//! Span inputs on replay: patch parsing, input-binding templates and the
//! "effective span" handed to executors.
//!
//! A recorded span stores what it ran with in its attributes (`command`,
//! `cwd`, `env`, `path`, `content`, `prompt`, `model`, ...). On replay the
//! engine builds an *effective* copy of the span:
//!
//! 1. attributes listed in `input_bindings` are re-rendered from the replayed
//!    outputs of the upstream spans they reference, so a patched or
//!    re-executed upstream output reaches downstream executors;
//! 2. the patch is applied to the fork span (a new prompt, command, file
//!    content, model config or environment);
//!
//! and executors only ever read the effective span.

use std::collections::BTreeMap;

use replaykit_core_model::{Document, PatchType, SpanRecord, Value, attrs};

/// Attribute recording which patch was applied to a span.
pub const PATCHED_ATTR: &str = "patched";

/// Read a text attribute.
pub fn text_attr<'a>(span: &'a SpanRecord, key: &str) -> Option<&'a str> {
    match span.attributes.get(key) {
        Some(Value::Text(text)) => Some(text.as_str()),
        _ => None,
    }
}

fn json_to_value(json: &serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map(Value::Int)
            .unwrap_or_else(|| Value::Text(n.to_string())),
        serde_json::Value::String(s) => Value::Text(s.clone()),
        serde_json::Value::Array(items) => Value::Array(items.iter().map(json_to_value).collect()),
        serde_json::Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), json_to_value(v)))
                .collect(),
        ),
    }
}

/// Interpret a patch replacement as a key/value object: a `Value::Object`, a
/// JSON object in text form, or `KEY=VALUE` lines (blank lines and `#`
/// comments ignored).
pub fn parse_key_values(value: &Value) -> Result<BTreeMap<String, Value>, String> {
    match value {
        Value::Object(doc) => Ok(doc.clone()),
        Value::Text(text) => {
            let trimmed = text.trim();
            if trimmed.starts_with('{') {
                let json: serde_json::Value = serde_json::from_str(trimmed)
                    .map_err(|e| format!("replacement is not valid JSON: {e}"))?;
                match json_to_value(&json) {
                    Value::Object(doc) => Ok(doc),
                    _ => Err("replacement JSON must be an object".into()),
                }
            } else {
                let mut out = BTreeMap::new();
                for line in trimmed.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    let (key, val) = line
                        .split_once('=')
                        .ok_or_else(|| format!("expected KEY=VALUE, got {line:?}"))?;
                    let key = key.trim();
                    if key.is_empty() {
                        return Err(format!("empty key in {line:?}"));
                    }
                    out.insert(key.to_owned(), Value::Text(val.to_owned()));
                }
                Ok(out)
            }
        }
        Value::Null => Ok(BTreeMap::new()),
        other => Err(format!("expected an object or KEY=VALUE text, got {other}")),
    }
}

/// Parse an environment map (`env` attribute or env patch).
pub fn parse_env(value: &Value) -> Result<BTreeMap<String, String>, String> {
    parse_key_values(value).map(|map| {
        map.into_iter()
            .map(|(k, v)| {
                let v = match v {
                    Value::Text(t) => t,
                    other => other.to_string(),
                };
                (k, v)
            })
            .collect()
    })
}

/// The replacement as plain text (objects/arrays are rejected).
pub fn replacement_text(value: &Value) -> Result<String, String> {
    match value {
        Value::Text(text) => Ok(text.clone()),
        Value::Int(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Null => Ok(String::new()),
        other => Err(format!("expected text replacement, got {other}")),
    }
}

/// `input_bindings` of a span: attribute name -> template.
pub fn bindings_of(span: &SpanRecord) -> Result<BTreeMap<String, String>, String> {
    let Some(value) = span.attributes.get(attrs::INPUT_BINDINGS) else {
        return Ok(BTreeMap::new());
    };
    let map = parse_key_values(value).map_err(|e| format!("bad input_bindings: {e}"))?;
    map.into_iter()
        .map(|(k, v)| match v {
            Value::Text(template) => Ok((k, template)),
            other => Err(format!(
                "input binding {k:?} must be a template string, got {other}"
            )),
        })
        .collect()
}

/// One `{{span_id|filter}}` placeholder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placeholder {
    pub span_id: String,
    pub filter: Option<String>,
}

fn parse_placeholders(template: &str) -> Result<Vec<(usize, usize, Placeholder)>, String> {
    let mut out = Vec::new();
    let mut cursor = 0;
    while let Some(open_rel) = template[cursor..].find("{{") {
        let open = cursor + open_rel;
        let close = template[open + 2..]
            .find("}}")
            .map(|rel| open + 2 + rel)
            .ok_or_else(|| format!("unterminated placeholder in template {template:?}"))?;
        let inner = template[open + 2..close].trim();
        let (span_id, filter) = match inner.split_once('|') {
            Some((id, filter)) => (id.trim(), Some(filter.trim().to_owned())),
            None => (inner, None),
        };
        if span_id.is_empty() {
            return Err(format!("empty placeholder in template {template:?}"));
        }
        if let Some(filter) = &filter
            && !matches!(filter.as_str(), "trim" | "words" | "raw")
        {
            return Err(format!(
                "unknown template filter {filter:?} (expected trim, words or raw)"
            ));
        }
        out.push((
            open,
            close + 2,
            Placeholder {
                span_id: span_id.to_owned(),
                filter,
            },
        ));
        cursor = close + 2;
    }
    Ok(out)
}

/// Span ids referenced by a template.
pub fn template_references(template: &str) -> Result<Vec<String>, String> {
    Ok(parse_placeholders(template)?
        .into_iter()
        .map(|(_, _, p)| p.span_id)
        .collect())
}

/// Render a template, resolving each placeholder's span output through
/// `lookup` (which returns the text of that span's primary output).
pub fn render_template(
    template: &str,
    lookup: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Result<String, String> {
    let placeholders = parse_placeholders(template)?;
    let mut out = String::with_capacity(template.len());
    let mut cursor = 0;
    for (start, end, placeholder) in placeholders {
        out.push_str(&template[cursor..start]);
        let raw = lookup(&placeholder.span_id)?;
        let rendered = match placeholder.filter.as_deref() {
            Some("trim") => raw.trim().to_owned(),
            Some("words") => raw.split_whitespace().collect::<Vec<_>>().join(" "),
            _ => raw,
        };
        out.push_str(&rendered);
        cursor = end;
    }
    out.push_str(&template[cursor..]);
    Ok(out)
}

/// Patch-provided inputs for the fork span, applied on top of the rendered
/// bindings.
#[derive(Clone, Debug, Default)]
pub struct PatchInputs {
    /// Attributes to overwrite (rendered if they contain placeholders).
    pub attributes: Document,
    /// Environment overrides for this span and its descendants.
    pub env: BTreeMap<String, String>,
}

/// Turn a patch on the fork span into attribute/env overrides. Patch types
/// that replace the span's *output* (tool output, retrieval context, a file
/// read's content) are handled by the engine and yield no input overrides.
pub fn patch_inputs(
    patch_type: PatchType,
    replacement: &Value,
    span: &SpanRecord,
) -> Result<PatchInputs, String> {
    use replaykit_core_model::SpanKind;
    let mut inputs = PatchInputs::default();
    match patch_type {
        PatchType::PromptEdit => {
            inputs.attributes.insert(
                attrs::PROMPT.into(),
                Value::Text(replacement_text(replacement)?),
            );
        }
        PatchType::ModelConfigEdit => {
            for (key, value) in parse_key_values(replacement)? {
                inputs.attributes.insert(key, value);
            }
        }
        PatchType::EnvVarOverride => {
            inputs.env = parse_env(replacement)?;
        }
        PatchType::CommandOverride => {
            let looks_like_object = matches!(replacement, Value::Object(_))
                || matches!(replacement, Value::Text(t) if t.trim_start().starts_with('{'));
            if looks_like_object {
                for (key, value) in parse_key_values(replacement)? {
                    match key.as_str() {
                        "command" | "cwd" => {
                            inputs.attributes.insert(key, value);
                        }
                        "env" => inputs.env = parse_env(&value)?,
                        other => {
                            return Err(format!(
                                "command_override accepts command, cwd and env, not {other:?}"
                            ));
                        }
                    }
                }
            } else {
                inputs.attributes.insert(
                    attrs::COMMAND.into(),
                    Value::Text(replacement_text(replacement)?),
                );
            }
        }
        PatchType::FileContentOverride => {
            if span.kind == SpanKind::FileWrite {
                inputs.attributes.insert(
                    attrs::CONTENT.into(),
                    Value::Text(replacement_text(replacement)?),
                );
            }
        }
        PatchType::SnapshotOverride
        | PatchType::ToolOutputOverride
        | PatchType::RetrievalContextOverride => {}
    }
    Ok(inputs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_placeholders_with_filters() {
        let mut lookup = |id: &str| -> Result<String, String> {
            match id {
                "tests" => Ok("a.py\nb.py\n".into()),
                "fix" => Ok("  body \n".into()),
                other => Err(format!("unknown {other}")),
            }
        };
        assert_eq!(
            render_template("run {{ tests|words }} then {{fix|trim}}!", &mut lookup).unwrap(),
            "run a.py b.py then body!"
        );
        assert_eq!(
            render_template("{{fix}}", &mut lookup).unwrap(),
            "  body \n"
        );
        assert!(render_template("{{missing}}", &mut lookup).is_err());
        assert!(render_template("{{tests|shout}}", &mut lookup).is_err());
        assert!(render_template("{{tests", &mut lookup).is_err());
        assert_eq!(
            template_references("x {{a}} {{b|words}}").unwrap(),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn parses_env_from_lines_and_json() {
        let env = parse_env(&Value::Text("A=1\n# c\nB = two words\n".into())).unwrap();
        assert_eq!(env.get("A").map(String::as_str), Some("1"));
        assert_eq!(env.get("B").map(String::as_str), Some(" two words"));
        let env = parse_env(&Value::Text(r#"{"X": "y", "N": 3}"#.into())).unwrap();
        assert_eq!(env.get("X").map(String::as_str), Some("y"));
        assert_eq!(env.get("N").map(String::as_str), Some("3"));
        assert!(parse_env(&Value::Text("novalue".into())).is_err());
    }

    #[test]
    fn command_override_accepts_text_or_object() {
        let span = SpanRecord {
            run_id: replaykit_core_model::RunId("r".into()),
            span_id: replaykit_core_model::SpanId("s".into()),
            trace_id: replaykit_core_model::TraceId("t".into()),
            parent_span_id: None,
            sequence_no: 1,
            kind: replaykit_core_model::SpanKind::ShellCommand,
            name: "sh".into(),
            status: replaykit_core_model::SpanStatus::Completed,
            started_at: 0,
            ended_at: Some(1),
            replay_policy: replaykit_core_model::ReplayPolicy::RerunnableSupported,
            executor_kind: None,
            executor_version: None,
            input_artifact_ids: vec![],
            output_artifact_ids: vec![],
            snapshot_id: None,
            input_fingerprint: None,
            output_fingerprint: None,
            environment_fingerprint: None,
            attributes: Document::new(),
            error_code: None,
            error_summary: None,
            cost: Default::default(),
        };
        let text = patch_inputs(
            PatchType::CommandOverride,
            &Value::Text("echo hi".into()),
            &span,
        )
        .unwrap();
        assert_eq!(
            text.attributes.get("command"),
            Some(&Value::Text("echo hi".into()))
        );
        let obj = patch_inputs(
            PatchType::CommandOverride,
            &Value::Text(r#"{"command": "make", "cwd": "sub", "env": {"K": "V"}}"#.into()),
            &span,
        )
        .unwrap();
        assert_eq!(obj.attributes.get("cwd"), Some(&Value::Text("sub".into())));
        assert_eq!(obj.env.get("K").map(String::as_str), Some("V"));
        assert!(
            patch_inputs(
                PatchType::CommandOverride,
                &Value::Text(r#"{"shell": "zsh"}"#.into()),
                &span
            )
            .is_err()
        );
    }
}
