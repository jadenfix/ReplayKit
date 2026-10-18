//! Executor implementations for forked replay.
//!
//! Executors receive the *effective* span (patched and re-bound inputs, see
//! [`crate::inputs`]) and a [`ReplayExecutionContext`] carrying the hermetic
//! sandbox, the environment to add, and the span as recorded.
//!
//! | span kind | executor | what replay does |
//! |---|---|---|
//! | `ShellCommand` | [`ShellExecutor`] | runs `command` with `/bin/sh -c` in the sandbox (`cwd` mapped into it) with the span's env |
//! | `ToolCall` | [`ToolExecutor`] | runs the tool's recorded `command` the same way; stdout is the tool output |
//! | `FileRead` | [`FileReadExecutor`] | reads `path` from the sandbox |
//! | `FileWrite` | [`FileWriteExecutor`] | writes `content` to `path` in the sandbox and emits a unified diff |
//! | `LlmCall` | [`ModelExecutorMode`] | blocked by default; see below |
//! | `Run`, `PlannerStep`, `Subgraph` | [`PlannerStepExecutor`] | re-derives status from the replayed children |
//!
//! Output fingerprints are content hashes, so a recorder can compute the
//! same values and replays that reproduce a recorded output are recognised
//! as unchanged (downstream work is then reused, not re-run):
//!
//! - shell: `sha256("exit={code};stdout={sha256(stdout)};stderr={sha256(stderr)}")`
//! - tool, model response, file read: `sha256(output bytes)`
//! - file write: `sha256(content written)`
//!
//! ## Model executor modes
//!
//! `LlmCall` replays never reach a provider unless explicitly configured:
//!
//! - [`ModelExecutorMode::Blocked`] (default): the span blocks, and the
//!   planner reports it as blocked before anything runs.
//! - [`ModelExecutorMode::Recorded`]: deterministic. Returns the response
//!   from a fixtures table keyed by (model, exact prompt), or the span's own
//!   recorded response when the effective prompt and model are unchanged.
//!   Anything else blocks with the prompt's hash.
//! - [`ModelExecutorMode::Command`]: a pluggable provider. Runs a command
//!   with the JSON request on stdin (`{"model","provider","prompt",
//!   "span_id","span_name","attributes"}`) and takes stdout as the response.
//! - [`ModelExecutorMode::Fake`]: canned responses for tests.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use replaykit_core_model::{
    ArtifactType, CostMetrics, Document, SpanKind, SpanRecord, SpanStatus, Value, attrs,
};
use replaykit_storage::blob::sha256_hex;
use serde::Deserialize;

use crate::inputs::text_attr;
use crate::{
    ExecutionResult, ExecutorRegistry, ProducedArtifact, ReplayError, ReplayExecutionContext,
};

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn summary(pairs: &[(&str, &str)]) -> Document {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), Value::Text((*v).to_owned())))
        .collect()
}

/// Truncate a string to at most `max` characters (not bytes), appending "..."
/// if shortened. Safe for multi-byte UTF-8.
fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(3)).collect();
        format!("{truncated}...")
    }
}

fn text_artifact(
    artifact_type: ArtifactType,
    mime: &str,
    bytes: Vec<u8>,
    summary: Document,
) -> ProducedArtifact {
    ProducedArtifact {
        artifact_type,
        mime: mime.into(),
        sha256: sha256_hex(&bytes),
        byte_len: bytes.len(),
        blob_path: String::new(), // filled by the blob store
        content: Some(bytes),
        summary,
        redaction: Document::new(),
        created_at: now_epoch_secs(),
    }
}

/// Map a recorded path into the sandbox (or use it as-is outside a fork).
fn resolve_path(context: &ReplayExecutionContext, path: &str) -> Result<PathBuf, String> {
    match &context.workspace {
        Some(workspace) => workspace.resolve(path),
        None => Ok(PathBuf::from(path)),
    }
}

/// Working directory for a process-running span.
fn working_dir(
    span: &SpanRecord,
    context: &ReplayExecutionContext,
) -> Result<Option<PathBuf>, String> {
    match text_attr(span, attrs::CWD) {
        Some(cwd) => resolve_path(context, cwd).map(Some),
        None => Ok(context.workspace.as_ref().map(|ws| ws.sandbox.clone())),
    }
}

fn timeout_of(span: &SpanRecord) -> Option<Duration> {
    match span.attributes.get(attrs::TIMEOUT_MS) {
        Some(Value::Int(ms)) if *ms > 0 => Some(Duration::from_millis(*ms as u64)),
        Some(Value::Text(ms)) => ms.parse::<u64>().ok().map(Duration::from_millis),
        _ => None,
    }
}

struct ProcessOutput {
    code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
}

/// Run `command` with `/bin/sh -c`, feeding `stdin`, with an optional
/// timeout. Reads both pipes concurrently so large outputs cannot deadlock.
fn run_shell(
    command: &str,
    cwd: Option<&Path>,
    env: &BTreeMap<String, String>,
    context: &ReplayExecutionContext,
    stdin: Option<Vec<u8>>,
    timeout: Option<Duration>,
) -> std::io::Result<ProcessOutput> {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg(command);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    if context.workspace.is_some() {
        cmd.env("REPLAYKIT_REPLAY", "1");
    }
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    // Own process group, so a timeout can stop everything the command
    // started (not just the shell).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn()?;

    let writer = stdin.and_then(|bytes| {
        child.stdin.take().map(|mut pipe| {
            std::thread::spawn(move || {
                let _ = pipe.write_all(&bytes);
            })
        })
    });
    let mut stdout_pipe = child.stdout.take().expect("piped stdout");
    let mut stderr_pipe = child.stderr.take().expect("piped stderr");
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });

    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if timeout.is_some_and(|limit| started.elapsed() >= limit) {
            #[cfg(unix)]
            if let Ok(pgid) = libc::pid_t::try_from(child.id()) {
                // SAFETY: kill(2) with a negative pid signals the process
                // group we created for this child; it has no memory effects.
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
            }
            let _ = child.kill();
            timed_out = true;
            break child.wait()?;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    Ok(ProcessOutput {
        code: if timed_out {
            124
        } else {
            status.code().unwrap_or(-1)
        },
        stdout,
        stderr,
        timed_out,
    })
}

fn shell_fingerprint(code: i32, stdout: &[u8], stderr: &[u8]) -> String {
    sha256_hex(
        format!(
            "exit={code};stdout={};stderr={}",
            sha256_hex(stdout),
            sha256_hex(stderr)
        )
        .as_bytes(),
    )
}

fn failure_message(out: &ProcessOutput, label: &str) -> String {
    if out.timed_out {
        format!("{label} timed out")
    } else {
        format!(
            "{label} exited with code {}: {}",
            out.code,
            truncate_str(&String::from_utf8_lossy(&out.stderr), 200)
        )
    }
}

// ---------------------------------------------------------------------------
// ShellExecutor
// ---------------------------------------------------------------------------

/// Executes shell commands via `/bin/sh -c` and captures stdout, stderr and
/// the exit code. In a fork, the command runs inside the sandbox.
pub struct ShellExecutor;

impl ShellExecutor {
    pub fn check(&self, span: &SpanRecord, context: &ReplayExecutionContext) -> Result<(), String> {
        if let Some(reason) = validate_shell_contract(span).block {
            return Err(reason);
        }
        working_dir(span, context).map(|_| ())
    }

    pub fn execute_span(
        &self,
        span: &SpanRecord,
        context: &ReplayExecutionContext,
    ) -> Result<ExecutionResult, ReplayError> {
        self.check(span, context).map_err(ReplayError::Blocked)?;
        let cmd_str = text_attr(span, attrs::COMMAND).unwrap_or(&span.name);
        let cwd = working_dir(span, context).map_err(ReplayError::Blocked)?;
        let out = match run_shell(
            cmd_str,
            cwd.as_deref(),
            &context.env,
            context,
            None,
            timeout_of(span),
        ) {
            Ok(out) => out,
            Err(err) => {
                return Ok(failed_result(format!(
                    "failed to start command in {}: {err}",
                    cwd.as_deref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| ".".into())
                )));
            }
        };

        let exit_code = out.code;
        let mut artifacts = Vec::new();
        if !out.stdout.is_empty() {
            let preview = truncate_str(&String::from_utf8_lossy(&out.stdout), 200);
            artifacts.push(text_artifact(
                ArtifactType::ShellStdout,
                "text/plain",
                out.stdout.clone(),
                summary(&[
                    ("exit_code", &exit_code.to_string()),
                    ("stdout_preview", &preview),
                ]),
            ));
        }
        if !out.stderr.is_empty() {
            let preview = truncate_str(&String::from_utf8_lossy(&out.stderr), 200);
            artifacts.push(text_artifact(
                ArtifactType::ShellStderr,
                "text/plain",
                out.stderr.clone(),
                summary(&[
                    ("exit_code", &exit_code.to_string()),
                    ("stderr_preview", &preview),
                ]),
            ));
        }
        // If no output at all, still produce a summary artifact.
        if artifacts.is_empty() {
            let content = format!("exit_code={exit_code}");
            artifacts.push(text_artifact(
                ArtifactType::ShellStdout,
                "text/plain",
                content.into_bytes(),
                summary(&[("exit_code", &exit_code.to_string())]),
            ));
        }

        let (status, error_summary) = if exit_code == 0 {
            (SpanStatus::Completed, None)
        } else {
            (SpanStatus::Failed, Some(failure_message(&out, "command")))
        };
        Ok(ExecutionResult {
            status,
            output_artifacts: artifacts,
            output_fingerprint: Some(shell_fingerprint(exit_code, &out.stdout, &out.stderr)),
            snapshot: None,
            error_summary,
            cost: CostMetrics::default(),
        })
    }
}

fn failed_result(error: String) -> ExecutionResult {
    ExecutionResult {
        status: SpanStatus::Failed,
        output_artifacts: Vec::new(),
        output_fingerprint: Some(sha256_hex(error.as_bytes())),
        snapshot: None,
        error_summary: Some(error),
        cost: CostMetrics::default(),
    }
}

// ---------------------------------------------------------------------------
// ToolExecutor
// ---------------------------------------------------------------------------

/// Re-runs a tool call whose recorded `command` attribute says how the tool
/// was invoked. stdout is the tool output.
pub struct ToolExecutor;

impl ToolExecutor {
    pub fn check(&self, span: &SpanRecord, context: &ReplayExecutionContext) -> Result<(), String> {
        if text_attr(span, attrs::COMMAND).is_none() {
            return Err(format!(
                "ToolCall {} has no 'command' attribute, so it cannot be re-run; record the \
                 tool's command or patch its output with tool_output_override",
                span.name
            ));
        }
        working_dir(span, context).map(|_| ())
    }

    pub fn execute_span(
        &self,
        span: &SpanRecord,
        context: &ReplayExecutionContext,
    ) -> Result<ExecutionResult, ReplayError> {
        self.check(span, context).map_err(ReplayError::Blocked)?;
        let command = text_attr(span, attrs::COMMAND).unwrap_or_default();
        let cwd = working_dir(span, context).map_err(ReplayError::Blocked)?;
        let out = match run_shell(
            command,
            cwd.as_deref(),
            &context.env,
            context,
            None,
            timeout_of(span),
        ) {
            Ok(out) => out,
            Err(err) => return Ok(failed_result(format!("failed to start tool: {err}"))),
        };
        let preview = truncate_str(&String::from_utf8_lossy(&out.stdout), 200);
        let mut artifacts = vec![text_artifact(
            ArtifactType::ToolOutput,
            "text/plain",
            out.stdout.clone(),
            summary(&[
                ("exit_code", &out.code.to_string()),
                ("output_preview", &preview),
            ]),
        )];
        if !out.stderr.is_empty() {
            let preview = truncate_str(&String::from_utf8_lossy(&out.stderr), 200);
            artifacts.push(text_artifact(
                ArtifactType::ShellStderr,
                "text/plain",
                out.stderr.clone(),
                summary(&[("stderr_preview", &preview)]),
            ));
        }
        let (status, error_summary) = if out.code == 0 {
            (SpanStatus::Completed, None)
        } else {
            (SpanStatus::Failed, Some(failure_message(&out, "tool")))
        };
        Ok(ExecutionResult {
            status,
            output_artifacts: artifacts,
            output_fingerprint: Some(sha256_hex(&out.stdout)),
            snapshot: None,
            error_summary,
            cost: CostMetrics::default(),
        })
    }
}

// ---------------------------------------------------------------------------
// FileReadExecutor
// ---------------------------------------------------------------------------

/// Reads a file. Expects `attributes["path"]` (or a `read <path>` name).
pub struct FileReadExecutor;

impl FileReadExecutor {
    pub fn check(&self, span: &SpanRecord, context: &ReplayExecutionContext) -> Result<(), String> {
        if let Some(reason) = validate_file_read_contract(span).block {
            return Err(reason);
        }
        resolve_path(context, &extract_file_path(span)).map(|_| ())
    }

    pub fn execute_span(
        &self,
        span: &SpanRecord,
        context: &ReplayExecutionContext,
    ) -> Result<ExecutionResult, ReplayError> {
        self.check(span, context).map_err(ReplayError::Blocked)?;
        let path = extract_file_path(span);
        let resolved = resolve_path(context, &path).map_err(ReplayError::Blocked)?;
        let content = match std::fs::read(&resolved) {
            Ok(content) => content,
            // A missing file is an outcome of this branch, not an inability
            // to replay.
            Err(err) => return Ok(failed_result(format!("failed to read file {path}: {err}"))),
        };
        let hash = sha256_hex(&content);
        let byte_len = content.len();
        let line_count = content.iter().filter(|&&b| b == b'\n').count();
        let artifact = text_artifact(
            ArtifactType::FileBlob,
            "application/octet-stream",
            content,
            summary(&[
                ("path", &path),
                ("lines", &line_count.to_string()),
                ("bytes", &byte_len.to_string()),
            ]),
        );
        Ok(ExecutionResult {
            status: SpanStatus::Completed,
            output_artifacts: vec![artifact],
            output_fingerprint: Some(hash),
            snapshot: None,
            error_summary: None,
            cost: CostMetrics::default(),
        })
    }
}

// ---------------------------------------------------------------------------
// FileWriteExecutor
// ---------------------------------------------------------------------------

/// Writes `content` to `path` inside the replay sandbox and emits a unified
/// diff of the change. Outside a fork (no sandbox) nothing is written and the
/// diff is computed against an empty file.
pub struct FileWriteExecutor;

impl FileWriteExecutor {
    pub fn check(&self, span: &SpanRecord, context: &ReplayExecutionContext) -> Result<(), String> {
        if let Some(reason) = validate_file_write_contract(span).block {
            return Err(reason);
        }
        resolve_path(context, &extract_file_path(span)).map(|_| ())
    }

    pub fn execute_span(
        &self,
        span: &SpanRecord,
        context: &ReplayExecutionContext,
    ) -> Result<ExecutionResult, ReplayError> {
        self.check(span, context).map_err(ReplayError::Blocked)?;
        let path = extract_file_path(span);
        let content = extract_write_content(span).ok_or_else(|| {
            ReplayError::Blocked("file write span is missing content attribute".into())
        })?;
        let previous = if context.workspace.is_some() {
            let resolved = resolve_path(context, &path).map_err(ReplayError::Blocked)?;
            let previous = std::fs::read_to_string(&resolved).unwrap_or_default();
            if let Some(parent) = resolved.parent()
                && let Err(err) = std::fs::create_dir_all(parent)
            {
                return Ok(failed_result(format!(
                    "failed to create {}: {err}",
                    parent.display()
                )));
            }
            if let Err(err) = std::fs::write(&resolved, content.as_bytes()) {
                return Ok(failed_result(format!("failed to write {path}: {err}")));
            }
            previous
        } else {
            String::new()
        };
        let diff = unified_diff(&path, &previous, &content);
        let artifact = text_artifact(
            ArtifactType::FileDiff,
            "text/x-diff",
            diff.into_bytes(),
            summary(&[("path", &path), ("bytes", &content.len().to_string())]),
        );
        Ok(ExecutionResult {
            status: SpanStatus::Completed,
            output_artifacts: vec![artifact],
            output_fingerprint: Some(sha256_hex(content.as_bytes())),
            snapshot: None,
            error_summary: None,
            cost: CostMetrics::default(),
        })
    }
}

/// Minimal unified diff (3 lines of context) of `old` -> `new`.
pub fn unified_diff(path: &str, old: &str, new: &str) -> String {
    let a = old.lines().collect::<Vec<_>>();
    let b = new.lines().collect::<Vec<_>>();
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    if a == b {
        return out;
    }
    // ops: (tag, line) with tag ' ', '-', '+'
    let ops: Vec<(char, &str)> = if a.len().saturating_mul(b.len()) > 4_000_000 {
        a.iter()
            .map(|l| ('-', *l))
            .chain(b.iter().map(|l| ('+', *l)))
            .collect()
    } else {
        let (n, m) = (a.len(), b.len());
        let mut lcs = vec![vec![0u32; m + 1]; n + 1];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i][j] = if a[i] == b[j] {
                    lcs[i + 1][j + 1] + 1
                } else {
                    lcs[i + 1][j].max(lcs[i][j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        let mut ops = Vec::new();
        while i < n && j < m {
            if a[i] == b[j] {
                ops.push((' ', a[i]));
                i += 1;
                j += 1;
            } else if lcs[i + 1][j] >= lcs[i][j + 1] {
                ops.push(('-', a[i]));
                i += 1;
            } else {
                ops.push(('+', b[j]));
                j += 1;
            }
        }
        ops.extend(a[i..].iter().map(|l| ('-', *l)));
        ops.extend(b[j..].iter().map(|l| ('+', *l)));
        ops
    };
    // Group into hunks with 3 lines of context.
    const CONTEXT: usize = 3;
    let changed = ops
        .iter()
        .enumerate()
        .filter(|(_, (tag, _))| *tag != ' ')
        .map(|(idx, _)| idx)
        .collect::<Vec<_>>();
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    for idx in changed {
        let start = idx.saturating_sub(CONTEXT);
        let end = (idx + CONTEXT + 1).min(ops.len());
        match hunks.last_mut() {
            Some(last) if start <= last.1 => last.1 = end,
            _ => hunks.push((start, end)),
        }
    }
    for (start, end) in hunks {
        let old_start = ops[..start].iter().filter(|(t, _)| *t != '+').count();
        let new_start = ops[..start].iter().filter(|(t, _)| *t != '-').count();
        let old_len = ops[start..end].iter().filter(|(t, _)| *t != '+').count();
        let new_len = ops[start..end].iter().filter(|(t, _)| *t != '-').count();
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            old_start + usize::from(old_len > 0),
            old_len,
            new_start + usize::from(new_len > 0),
            new_len
        ));
        for (tag, line) in &ops[start..end] {
            out.push(*tag);
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

// ---------------------------------------------------------------------------
// PlannerStepExecutor
// ---------------------------------------------------------------------------

/// Executor for container spans (`PlannerStep`, `Run`, `Subgraph`).
///
/// Planning decisions are not re-made on replay: the container's recorded
/// output is reused verbatim. Its *status* is re-derived from its children
/// as they ended in the replay:
///
/// - any child blocked: blocked;
/// - no child changed status: the recorded status stands;
/// - a child that did not fail in the recording now fails: failed;
/// - the container failed in the recording and no child fails any more:
///   completed (its recorded failure came from its children);
/// - otherwise the recorded status stands.
pub struct PlannerStepExecutor;

impl PlannerStepExecutor {
    pub fn execute_span(
        &self,
        span: &SpanRecord,
        context: &ReplayExecutionContext,
    ) -> Result<ExecutionResult, ReplayError> {
        let recorded = context
            .recorded
            .as_ref()
            .map(|r| r.span.clone())
            .unwrap_or_else(|| span.clone());
        let children = &context.children;
        let (status, error_summary) = if let Some(blocked) = children
            .iter()
            .find(|c| c.replayed.status == SpanStatus::Blocked)
        {
            (
                SpanStatus::Blocked,
                Some(format!(
                    "replay blocked: child span {} is blocked",
                    blocked.replayed.span_id.0
                )),
            )
        } else if children
            .iter()
            .all(|c| c.recorded.status == c.replayed.status)
        {
            (recorded.status, recorded.error_summary.clone())
        } else if let Some(newly_failed) = children.iter().find(|c| {
            c.replayed.status == SpanStatus::Failed && c.recorded.status != SpanStatus::Failed
        }) {
            (
                SpanStatus::Failed,
                Some(format!(
                    "child span {} now fails: {}",
                    newly_failed.replayed.span_id.0,
                    newly_failed
                        .replayed
                        .error_summary
                        .as_deref()
                        .unwrap_or("failed")
                )),
            )
        } else if recorded.status == SpanStatus::Failed
            && !children
                .iter()
                .any(|c| c.replayed.status == SpanStatus::Failed)
        {
            (SpanStatus::Completed, None)
        } else {
            (recorded.status, recorded.error_summary.clone())
        };
        Ok(ExecutionResult {
            status,
            output_artifacts: Vec::new(),
            output_fingerprint: recorded.output_fingerprint.clone(),
            snapshot: None,
            error_summary,
            cost: CostMetrics::default(),
        })
    }
}

// ---------------------------------------------------------------------------
// Model executors
// ---------------------------------------------------------------------------

fn model_of(span: &SpanRecord) -> String {
    text_attr(span, attrs::MODEL)
        .map(str::to_owned)
        .or_else(|| span.executor_kind.clone())
        .unwrap_or_else(|| "unknown".into())
}

/// The prompt sent to the model: the `prompt` attribute, else the recorded
/// request JSON.
fn prompt_of(span: &SpanRecord) -> Option<&str> {
    text_attr(span, attrs::PROMPT).or_else(|| text_attr(span, attrs::MODEL_REQUEST_JSON))
}

fn model_response(response: &str, extra: &[(&str, &str)]) -> ExecutionResult {
    let mut pairs = vec![("response_preview", truncate_str(response, 200))];
    pairs.extend(extra.iter().map(|(k, v)| (*k, (*v).to_owned())));
    let doc = pairs
        .into_iter()
        .map(|(k, v)| (k.to_owned(), Value::Text(v)))
        .collect();
    ExecutionResult {
        status: SpanStatus::Completed,
        output_artifacts: vec![text_artifact(
            ArtifactType::ModelResponse,
            "text/plain",
            response.as_bytes().to_vec(),
            doc,
        )],
        output_fingerprint: Some(sha256_hex(response.as_bytes())),
        snapshot: None,
        error_summary: None,
        cost: CostMetrics::default(),
    }
}

/// Default safe mode for LlmCall. Blocks with a descriptive message.
pub struct BlockedModelExecutor;

impl BlockedModelExecutor {
    fn reason(span: &SpanRecord) -> String {
        format!(
            "LlmCall to model '{}' requires an explicit model executor (model mode is \
             'blocked'); use model mode 'recorded' with a response fixture or 'command' with a \
             provider command",
            model_of(span)
        )
    }

    pub fn execute_span(
        &self,
        span: &SpanRecord,
        _context: &ReplayExecutionContext,
    ) -> Result<ExecutionResult, ReplayError> {
        if let Some(reason) = validate_llm_contract(span).block {
            return Err(ReplayError::Blocked(reason));
        }
        Err(ReplayError::Blocked(Self::reason(span)))
    }

    pub fn why_not(&self, span: &SpanRecord) -> Option<String> {
        let diag = validate_llm_contract(span);
        if let Some(reason) = diag.block {
            return Some(reason);
        }
        Some(Self::reason(span))
    }
}

/// Deterministic model executor for tests. Returns configured responses.
pub struct FakeModelExecutor {
    responses: BTreeMap<String, String>,
    default_response: String,
}

impl FakeModelExecutor {
    pub fn new(default_response: impl Into<String>) -> Self {
        Self {
            responses: BTreeMap::new(),
            default_response: default_response.into(),
        }
    }

    pub fn with_response(
        mut self,
        span_name: impl Into<String>,
        response: impl Into<String>,
    ) -> Self {
        self.responses.insert(span_name.into(), response.into());
        self
    }

    pub fn execute_span(
        &self,
        span: &SpanRecord,
        _context: &ReplayExecutionContext,
    ) -> Result<ExecutionResult, ReplayError> {
        if let Some(reason) = validate_llm_contract(span).block {
            return Err(ReplayError::Blocked(reason));
        }
        let response = self
            .responses
            .get(&span.name)
            .unwrap_or(&self.default_response);
        Ok(model_response(response, &[("fake", "true")]))
    }
}

/// One recorded (model, prompt) -> response mapping.
#[derive(Clone, Debug, Deserialize)]
pub struct ModelFixture {
    /// Restrict the fixture to one model; `None` matches any model.
    #[serde(default)]
    pub model: Option<String>,
    pub prompt: String,
    pub response: String,
}

#[derive(Deserialize)]
struct FixtureFile {
    responses: Vec<ModelFixture>,
}

/// Deterministic replay of recorded model responses.
#[derive(Clone, Debug, Default)]
pub struct RecordedModelExecutor {
    fixtures: Vec<ModelFixture>,
}

impl RecordedModelExecutor {
    pub fn new(fixtures: Vec<ModelFixture>) -> Self {
        Self { fixtures }
    }

    /// Load `{"responses": [{"model"?, "prompt", "response"}, ...]}`.
    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read model fixtures {}: {e}", path.display()))?;
        let file: FixtureFile = serde_json::from_str(&text)
            .map_err(|e| format!("bad model fixtures {}: {e}", path.display()))?;
        Ok(Self::new(file.responses))
    }

    fn lookup(
        &self,
        span: &SpanRecord,
        context: &ReplayExecutionContext,
    ) -> Result<String, String> {
        if let Some(reason) = validate_llm_contract(span).block {
            return Err(reason);
        }
        let model = model_of(span);
        let prompt = prompt_of(span).ok_or_else(|| {
            format!(
                "LlmCall {} has no 'prompt' or 'model_request_json' attribute to match a \
                 recorded response",
                span.name
            )
        })?;
        if let Some(fixture) = self
            .fixtures
            .iter()
            .find(|f| f.prompt == prompt && f.model.as_deref().is_none_or(|m| m == model))
        {
            return Ok(fixture.response.clone());
        }
        if let Some(recorded) = &context.recorded
            && prompt_of(&recorded.span) == Some(prompt)
            && model_of(&recorded.span) == model
            && let Some(content) = recorded
                .outputs
                .iter()
                .find(|o| o.artifact.artifact_type == ArtifactType::ModelResponse)
                .and_then(|o| o.content.as_ref())
        {
            return Ok(String::from_utf8_lossy(content).into_owned());
        }
        Err(format!(
            "no recorded response for model '{model}' and this prompt (sha256 {}); add one to \
             the model fixtures or use model mode 'command'",
            &sha256_hex(prompt.as_bytes())[..12]
        ))
    }

    pub fn check(&self, span: &SpanRecord, context: &ReplayExecutionContext) -> Result<(), String> {
        self.lookup(span, context).map(|_| ())
    }

    pub fn execute_span(
        &self,
        span: &SpanRecord,
        context: &ReplayExecutionContext,
    ) -> Result<ExecutionResult, ReplayError> {
        let response = self.lookup(span, context).map_err(ReplayError::Blocked)?;
        Ok(model_response(&response, &[("model_mode", "recorded")]))
    }
}

/// Pluggable provider: runs `command` (via `/bin/sh -c`, in the sandbox)
/// with the JSON model request on stdin and uses stdout as the response.
#[derive(Clone, Debug)]
pub struct CommandModelExecutor {
    command: String,
    timeout: Option<Duration>,
}

impl CommandModelExecutor {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            timeout: Some(Duration::from_secs(600)),
        }
    }

    /// The JSON request sent on stdin.
    pub fn request_json(span: &SpanRecord) -> String {
        let attributes = span
            .attributes
            .iter()
            .filter_map(|(k, v)| match v {
                Value::Text(t) => Some((k.clone(), serde_json::Value::String(t.clone()))),
                Value::Int(n) => Some((k.clone(), serde_json::Value::from(*n))),
                Value::Bool(b) => Some((k.clone(), serde_json::Value::Bool(*b))),
                _ => None,
            })
            .collect::<serde_json::Map<_, _>>();
        serde_json::json!({
            "model": model_of(span),
            "provider": text_attr(span, attrs::PROVIDER),
            "prompt": prompt_of(span),
            "span_id": span.span_id.0,
            "span_name": span.name,
            "attributes": attributes,
        })
        .to_string()
    }

    pub fn execute_span(
        &self,
        span: &SpanRecord,
        context: &ReplayExecutionContext,
    ) -> Result<ExecutionResult, ReplayError> {
        if let Some(reason) = validate_llm_contract(span).block {
            return Err(ReplayError::Blocked(reason));
        }
        let cwd = context.workspace.as_ref().map(|ws| ws.sandbox.clone());
        let mut env = context.env.clone();
        env.insert("REPLAYKIT_MODEL".into(), model_of(span));
        let out = match run_shell(
            &self.command,
            cwd.as_deref(),
            &env,
            context,
            Some(Self::request_json(span).into_bytes()),
            self.timeout,
        ) {
            Ok(out) => out,
            Err(err) => {
                return Ok(failed_result(format!(
                    "failed to start model command: {err}"
                )));
            }
        };
        if out.code != 0 {
            return Ok(failed_result(failure_message(&out, "model command")));
        }
        let response = String::from_utf8_lossy(&out.stdout).into_owned();
        Ok(model_response(&response, &[("model_mode", "command")]))
    }
}

/// Selects which model executor to use for LlmCall spans.
pub enum ModelExecutorMode {
    Blocked,
    Fake(FakeModelExecutor),
    Recorded(RecordedModelExecutor),
    Command(CommandModelExecutor),
}

// ---------------------------------------------------------------------------
// CompositeExecutorRegistry
// ---------------------------------------------------------------------------

/// Dispatches to the appropriate executor based on span kind.
///
/// Supported span kinds: `ShellCommand`, `ToolCall`, `FileRead`,
/// `FileWrite`, `LlmCall`, and the containers `Run`, `PlannerStep`,
/// `Subgraph`. LlmCall is dispatched to a configurable
/// [`ModelExecutorMode`]; the default `Blocked` mode never calls a model.
pub struct CompositeExecutorRegistry {
    shell: ShellExecutor,
    tool: ToolExecutor,
    file_read: FileReadExecutor,
    file_write: FileWriteExecutor,
    planner: PlannerStepExecutor,
    model_mode: ModelExecutorMode,
}

impl Default for CompositeExecutorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CompositeExecutorRegistry {
    pub fn new() -> Self {
        Self {
            shell: ShellExecutor,
            tool: ToolExecutor,
            file_read: FileReadExecutor,
            file_write: FileWriteExecutor,
            planner: PlannerStepExecutor,
            model_mode: ModelExecutorMode::Blocked,
        }
    }

    /// Configure the model executor mode for LlmCall spans.
    pub fn with_model_mode(mut self, mode: ModelExecutorMode) -> Self {
        self.model_mode = mode;
        self
    }

    pub fn model_mode_label(&self) -> &'static str {
        match self.model_mode {
            ModelExecutorMode::Blocked => "blocked",
            ModelExecutorMode::Fake(_) => "fake",
            ModelExecutorMode::Recorded(_) => "recorded",
            ModelExecutorMode::Command(_) => "command",
        }
    }
}

impl ExecutorRegistry for CompositeExecutorRegistry {
    fn supports(&self, span: &SpanRecord) -> bool {
        matches!(
            span.kind,
            SpanKind::ShellCommand
                | SpanKind::ToolCall
                | SpanKind::FileRead
                | SpanKind::FileWrite
                | SpanKind::LlmCall
                | SpanKind::PlannerStep
                | SpanKind::Run
                | SpanKind::Subgraph
        )
    }

    fn check(&self, span: &SpanRecord, context: &ReplayExecutionContext) -> Result<(), String> {
        match span.kind {
            SpanKind::ShellCommand => self.shell.check(span, context),
            SpanKind::ToolCall => self.tool.check(span, context),
            SpanKind::FileRead => self.file_read.check(span, context),
            SpanKind::FileWrite => self.file_write.check(span, context),
            SpanKind::LlmCall => {
                if let Some(reason) = validate_llm_contract(span).block {
                    return Err(reason);
                }
                match &self.model_mode {
                    ModelExecutorMode::Blocked => Err(BlockedModelExecutor::reason(span)),
                    ModelExecutorMode::Recorded(recorded) => recorded.check(span, context),
                    ModelExecutorMode::Fake(_) | ModelExecutorMode::Command(_) => Ok(()),
                }
            }
            SpanKind::PlannerStep | SpanKind::Run | SpanKind::Subgraph => Ok(()),
            other => Err(format!("no executor for span kind {other:?}")),
        }
    }

    fn execute(
        &self,
        span: &SpanRecord,
        context: &ReplayExecutionContext,
    ) -> Result<ExecutionResult, ReplayError> {
        match span.kind {
            SpanKind::ShellCommand => self.shell.execute_span(span, context),
            SpanKind::ToolCall => self.tool.execute_span(span, context),
            SpanKind::FileRead => self.file_read.execute_span(span, context),
            SpanKind::FileWrite => self.file_write.execute_span(span, context),
            SpanKind::PlannerStep | SpanKind::Run | SpanKind::Subgraph => {
                self.planner.execute_span(span, context)
            }
            SpanKind::LlmCall => match &self.model_mode {
                ModelExecutorMode::Blocked => BlockedModelExecutor.execute_span(span, context),
                ModelExecutorMode::Fake(fake) => fake.execute_span(span, context),
                ModelExecutorMode::Recorded(recorded) => recorded.execute_span(span, context),
                ModelExecutorMode::Command(command) => command.execute_span(span, context),
            },
            _ => Err(ReplayError::Blocked(format!(
                "no executor for span kind {:?}",
                span.kind
            ))),
        }
    }

    fn why_not(&self, span: &SpanRecord) -> Option<String> {
        match span.kind {
            SpanKind::ShellCommand => {
                let diag = validate_shell_contract(span);
                diag.block.clone().or_else(|| diag.warnings_summary())
            }
            SpanKind::ToolCall => text_attr(span, attrs::COMMAND)
                .is_none()
                .then(|| format!("ToolCall {} has no 'command' attribute", span.name)),
            SpanKind::FileRead => {
                let diag = validate_file_read_contract(span);
                diag.block.clone().or_else(|| diag.warnings_summary())
            }
            SpanKind::FileWrite => {
                let diag = validate_file_write_contract(span);
                diag.block.clone().or_else(|| diag.warnings_summary())
            }
            SpanKind::LlmCall => {
                let diag = validate_llm_contract(span);
                if diag.block.is_some() {
                    return diag.block;
                }
                match &self.model_mode {
                    ModelExecutorMode::Blocked => BlockedModelExecutor.why_not(span),
                    _ => diag.warnings_summary(),
                }
            }
            SpanKind::PlannerStep | SpanKind::Run | SpanKind::Subgraph => None,
            _ => Some(format!("no executor for span kind {:?}", span.kind)),
        }
    }
}

// ---------------------------------------------------------------------------
// Contract Diagnostics
// ---------------------------------------------------------------------------

/// Result of contract validation for a span kind. Separates hard blocks
/// (span cannot be replayed at all) from warnings (replay proceeds but
/// with degraded fidelity, e.g. falling back to span name as command).
#[derive(Clone, Debug, Default)]
pub struct ContractDiagnostic {
    /// If set, replay must stop — the span is missing a required attribute.
    pub block: Option<String>,
    /// Non-fatal issues — replay proceeds but semantics are weaker.
    pub warnings: Vec<String>,
}

impl ContractDiagnostic {
    fn blocked(reason: impl Into<String>) -> Self {
        Self {
            block: Some(reason.into()),
            warnings: Vec::new(),
        }
    }

    fn ok() -> Self {
        Self::default()
    }

    fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }

    /// Format warnings into a single description for `why_not()`.
    pub fn warnings_summary(&self) -> Option<String> {
        if self.warnings.is_empty() {
            None
        } else {
            Some(self.warnings.join("; "))
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract a file path from span attributes or name.
fn extract_file_path(span: &SpanRecord) -> String {
    if let Some(Value::Text(path)) = span.attributes.get(attrs::PATH) {
        return path.clone();
    }
    // Backwards compat: old spans encode path in name
    let name = &span.name;
    if let Some(rest) = name
        .strip_prefix("read ")
        .or_else(|| name.strip_prefix("write "))
    {
        return rest.to_string();
    }
    name.clone()
}

/// Extract content to write from span attributes.
fn extract_write_content(span: &SpanRecord) -> Option<String> {
    if let Some(Value::Text(content)) = span.attributes.get(attrs::CONTENT) {
        return Some(content.clone());
    }
    None
}

fn validate_shell_contract(span: &SpanRecord) -> ContractDiagnostic {
    let has_command = matches!(span.attributes.get(attrs::COMMAND), Some(Value::Text(_)));
    if !has_command && span.name.is_empty() {
        return ContractDiagnostic::blocked(
            "ShellCommand span has no 'command' attribute and empty name",
        );
    }
    let mut diag = ContractDiagnostic::ok();
    if !has_command {
        diag.warn("missing 'command' attribute — falling back to span name as command");
    }
    if !matches!(span.attributes.get(attrs::CWD), Some(Value::Text(_))) {
        diag.warn("missing 'cwd' attribute — shell will use the sandbox root");
    }
    diag
}

fn validate_file_read_contract(span: &SpanRecord) -> ContractDiagnostic {
    let has_path = matches!(span.attributes.get(attrs::PATH), Some(Value::Text(_)));
    if !has_path && span.name.is_empty() {
        return ContractDiagnostic::blocked("FileRead span has no 'path' attribute and empty name");
    }
    let mut diag = ContractDiagnostic::ok();
    if !has_path {
        diag.warn("missing 'path' attribute — falling back to span name as path");
    }
    diag
}

fn validate_file_write_contract(span: &SpanRecord) -> ContractDiagnostic {
    let has_path = matches!(span.attributes.get(attrs::PATH), Some(Value::Text(_)));
    let has_content = matches!(span.attributes.get(attrs::CONTENT), Some(Value::Text(_)));

    if !has_path && span.name.is_empty() {
        return ContractDiagnostic::blocked(
            "FileWrite span has no 'path' attribute and empty name",
        );
    }
    if !has_content {
        return ContractDiagnostic::blocked(
            "FileWrite span has no 'content' attribute — cannot replay without write content",
        );
    }
    let mut diag = ContractDiagnostic::ok();
    if !has_path {
        diag.warn("missing 'path' attribute — falling back to span name as path");
    }
    diag
}

fn validate_llm_contract(span: &SpanRecord) -> ContractDiagnostic {
    let has_model = matches!(span.attributes.get(attrs::MODEL), Some(Value::Text(_)))
        || span
            .executor_kind
            .as_deref()
            .is_some_and(|value| !value.is_empty());
    if !has_model {
        return ContractDiagnostic::blocked(
            "LlmCall span has no 'model' attribute or executor kind",
        );
    }
    let mut diag = ContractDiagnostic::ok();
    if !matches!(span.attributes.get(attrs::PROVIDER), Some(Value::Text(_))) {
        diag.warn("missing 'provider' attribute — needed for live model dispatch");
    }
    if prompt_of(span).is_none() {
        diag.warn("missing 'prompt'/'model_request_json' attribute — replay fidelity reduced");
    }
    diag
}

#[cfg(test)]
pub(crate) mod tests_support {
    pub(crate) fn shell_fp(code: i32, stdout: &str, stderr: &str) -> String {
        super::shell_fingerprint(code, stdout.as_bytes(), stderr.as_bytes())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChildOutcome, RecordedOutput, RecordedSpan, ReplayWorkspace};
    use replaykit_core_model::{ArtifactId, ArtifactRecord, ReplayPolicy, SpanId};

    fn test_context() -> ReplayExecutionContext {
        ReplayExecutionContext::new(
            replaykit_core_model::RunId("src-run".into()),
            replaykit_core_model::RunId("tgt-run".into()),
            SpanId("fork".into()),
        )
    }

    fn sandbox_context(name: &str) -> (ReplayExecutionContext, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "rk-exec-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = test_context();
        ctx.workspace = Some(ReplayWorkspace {
            sandbox: dir.clone(),
            recorded_root: Some("/recorded/repo".into()),
        });
        (ctx, dir)
    }

    fn make_span(kind: SpanKind, name: &str) -> SpanRecord {
        SpanRecord {
            run_id: replaykit_core_model::RunId("test-run".into()),
            span_id: SpanId("span-1".into()),
            trace_id: replaykit_core_model::TraceId("trace-1".into()),
            parent_span_id: None,
            sequence_no: 1,
            kind,
            name: name.into(),
            status: SpanStatus::Running,
            started_at: 1,
            ended_at: None,
            replay_policy: ReplayPolicy::RerunnableSupported,
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
            cost: CostMetrics::default(),
        }
    }

    fn with_attr(mut span: SpanRecord, key: &str, value: &str) -> SpanRecord {
        span.attributes
            .insert(key.into(), Value::Text(value.into()));
        span
    }

    fn stdout_of(result: &ExecutionResult) -> String {
        String::from_utf8(result.output_artifacts[0].content.clone().unwrap()).unwrap()
    }

    #[test]
    fn truncate_str_ascii() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello world!", 8), "hello...");
    }

    #[test]
    fn truncate_str_multibyte_safe() {
        let s = "héllo wörld café";
        let result = truncate_str(s, 8);
        assert!(result.ends_with("..."));
        assert!(result.chars().count() <= 8);
    }

    // -- ShellExecutor --------------------------------------------------------

    #[test]
    fn shell_executor_success() {
        let span = make_span(SpanKind::ShellCommand, "echo hello");
        let result = ShellExecutor.execute_span(&span, &test_context()).unwrap();
        assert_eq!(result.status, SpanStatus::Completed);
        assert!(!result.output_artifacts.is_empty());
        assert!(result.output_fingerprint.is_some());
        assert!(result.error_summary.is_none());
    }

    #[test]
    fn shell_executor_failure() {
        let span = make_span(SpanKind::ShellCommand, "exit 1");
        let result = ShellExecutor.execute_span(&span, &test_context()).unwrap();
        assert_eq!(result.status, SpanStatus::Failed);
        assert!(result.error_summary.is_some());
    }

    #[test]
    fn shell_executor_captures_stdout_and_stderr() {
        let span = make_span(SpanKind::ShellCommand, "echo hello && echo err >&2");
        let result = ShellExecutor.execute_span(&span, &test_context()).unwrap();
        assert_eq!(result.status, SpanStatus::Completed);
        let types: Vec<_> = result
            .output_artifacts
            .iter()
            .map(|a| a.artifact_type)
            .collect();
        assert!(types.contains(&ArtifactType::ShellStdout));
        assert!(types.contains(&ArtifactType::ShellStderr));
    }

    #[test]
    fn shell_fingerprint_changes_with_output_content() {
        let r1 = ShellExecutor
            .execute_span(
                &make_span(SpanKind::ShellCommand, "echo aaa"),
                &test_context(),
            )
            .unwrap();
        let r2 = ShellExecutor
            .execute_span(
                &make_span(SpanKind::ShellCommand, "echo bbb"),
                &test_context(),
            )
            .unwrap();
        assert_ne!(r1.output_fingerprint, r2.output_fingerprint);
        assert_eq!(
            r1.output_fingerprint.as_deref(),
            Some(shell_fingerprint(0, b"aaa\n", b"").as_str())
        );
    }

    #[test]
    fn shell_executor_receives_env_overrides() {
        let span = with_attr(
            make_span(SpanKind::ShellCommand, "greet"),
            attrs::COMMAND,
            "printf '%s' \"$GREETING\"",
        );
        let mut ctx = test_context();
        ctx.env.insert("GREETING".into(), "patched hello".into());
        let result = ShellExecutor.execute_span(&span, &ctx).unwrap();
        assert_eq!(stdout_of(&result), "patched hello");
    }

    #[test]
    fn shell_executor_runs_inside_the_sandbox() {
        let (ctx, dir) = sandbox_context("shell-cwd");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/cwd.txt"), "cwd-data\n").unwrap();
        let span = with_attr(
            with_attr(
                make_span(SpanKind::ShellCommand, "cat"),
                attrs::COMMAND,
                "cat cwd.txt; pwd",
            ),
            attrs::CWD,
            "/recorded/repo/sub",
        );
        let result = ShellExecutor.execute_span(&span, &ctx).unwrap();
        assert_eq!(result.status, SpanStatus::Completed);
        let out = stdout_of(&result);
        assert!(out.starts_with("cwd-data\n"), "{out}");
        assert!(out.trim_end().ends_with("/sub"), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shell_executor_blocks_cwd_outside_the_recorded_workspace() {
        let (ctx, dir) = sandbox_context("shell-escape");
        let span = with_attr(make_span(SpanKind::ShellCommand, "ls"), attrs::CWD, "/etc");
        match ShellExecutor.execute_span(&span, &ctx) {
            Err(ReplayError::Blocked(msg)) => assert!(msg.contains("outside"), "{msg}"),
            other => panic!("expected Blocked, got {other:?}"),
        }
        assert!(ShellExecutor.check(&span, &ctx).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shell_executor_honors_timeout() {
        let mut span = make_span(SpanKind::ShellCommand, "sleep 5; echo never");
        span.attributes
            .insert(attrs::TIMEOUT_MS.into(), Value::Int(100));
        let started = Instant::now();
        let result = ShellExecutor.execute_span(&span, &test_context()).unwrap();
        assert_eq!(result.status, SpanStatus::Failed);
        assert!(result.error_summary.unwrap().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn shell_executor_prefers_command_attribute() {
        let span = with_attr(
            make_span(SpanKind::ShellCommand, "old name"),
            attrs::COMMAND,
            "echo from_attr",
        );
        let result = ShellExecutor.execute_span(&span, &test_context()).unwrap();
        assert_eq!(result.status, SpanStatus::Completed);
        let preview = result.output_artifacts[0]
            .summary
            .get("stdout_preview")
            .unwrap()
            .to_string();
        assert!(
            preview.contains("from_attr"),
            "expected from_attr in: {preview}"
        );
    }

    #[test]
    fn shell_blocks_with_empty_name_and_no_command() {
        let span = make_span(SpanKind::ShellCommand, "");
        match ShellExecutor.execute_span(&span, &test_context()) {
            Err(ReplayError::Blocked(msg)) => assert!(msg.contains("command"), "{msg}"),
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    // -- ToolExecutor ---------------------------------------------------------

    #[test]
    fn tool_executor_reruns_recorded_command() {
        let (ctx, dir) = sandbox_context("tool");
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        std::fs::write(dir.join("b.txt"), "y").unwrap();
        let span = with_attr(make_span(SpanKind::ToolCall, "list"), attrs::COMMAND, "ls");
        let result = ToolExecutor.execute_span(&span, &ctx).unwrap();
        assert_eq!(result.status, SpanStatus::Completed);
        assert_eq!(
            result.output_artifacts[0].artifact_type,
            ArtifactType::ToolOutput
        );
        assert_eq!(stdout_of(&result), "a.txt\nb.txt\n");
        assert_eq!(
            result.output_fingerprint.as_deref(),
            Some(sha256_hex(b"a.txt\nb.txt\n").as_str())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tool_without_command_is_not_executable() {
        let span = make_span(SpanKind::ToolCall, "opaque");
        let err = CompositeExecutorRegistry::new()
            .check(&span, &test_context())
            .unwrap_err();
        assert!(err.contains("tool_output_override"), "{err}");
    }

    // -- FileReadExecutor -----------------------------------------------------

    #[test]
    fn file_read_executor_reads_existing_file() {
        let dir = std::env::temp_dir().join("replaykit-test-fread");
        let _ = std::fs::create_dir_all(&dir);
        let file_path = dir.join("test.txt");
        std::fs::write(&file_path, "line1\nline2\n").unwrap();
        let span = make_span(SpanKind::FileRead, &file_path.to_string_lossy());
        let result = FileReadExecutor
            .execute_span(&span, &test_context())
            .unwrap();
        assert_eq!(result.status, SpanStatus::Completed);
        assert_eq!(result.output_artifacts.len(), 1);
        assert_eq!(
            result.output_artifacts[0].artifact_type,
            ArtifactType::FileBlob
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_read_of_missing_file_fails_instead_of_blocking() {
        let span = make_span(SpanKind::FileRead, "/nonexistent/path/file.txt");
        let result = FileReadExecutor
            .execute_span(&span, &test_context())
            .unwrap();
        assert_eq!(result.status, SpanStatus::Failed);
        assert!(result.error_summary.unwrap().contains("failed to read"));
    }

    #[test]
    fn file_read_resolves_into_the_sandbox() {
        let (ctx, dir) = sandbox_context("fread");
        std::fs::write(dir.join("in.txt"), "sandboxed").unwrap();
        let span = with_attr(make_span(SpanKind::FileRead, "read"), attrs::PATH, "in.txt");
        let result = FileReadExecutor.execute_span(&span, &ctx).unwrap();
        assert_eq!(stdout_of(&result), "sandboxed");
        let abs = with_attr(
            make_span(SpanKind::FileRead, "read"),
            attrs::PATH,
            "/recorded/repo/in.txt",
        );
        assert_eq!(
            stdout_of(&FileReadExecutor.execute_span(&abs, &ctx).unwrap()),
            "sandboxed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- FileWriteExecutor ----------------------------------------------------

    #[test]
    fn file_write_executor_produces_diff_artifact_without_side_effect() {
        let mut span = make_span(SpanKind::FileWrite, "output-rk-never-written.txt");
        span.attributes
            .insert("content".into(), Value::Text("patched line".into()));
        let result = FileWriteExecutor
            .execute_span(&span, &test_context())
            .unwrap();
        assert_eq!(result.status, SpanStatus::Completed);
        assert_eq!(result.output_artifacts.len(), 1);
        assert_eq!(
            result.output_artifacts[0].artifact_type,
            ArtifactType::FileDiff
        );
        assert!(!Path::new("output-rk-never-written.txt").exists());
    }

    #[test]
    fn file_write_executor_writes_into_the_sandbox() {
        let (ctx, dir) = sandbox_context("fwrite");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.py"), "x = 1\ny = 2\n").unwrap();
        let span = with_attr(
            with_attr(
                make_span(SpanKind::FileWrite, "write"),
                attrs::PATH,
                "src/a.py",
            ),
            attrs::CONTENT,
            "x = 1\ny = 3\n",
        );
        let result = FileWriteExecutor.execute_span(&span, &ctx).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("src/a.py")).unwrap(),
            "x = 1\ny = 3\n"
        );
        let diff = stdout_of(&result);
        assert!(diff.contains("-y = 2\n+y = 3\n"), "{diff}");
        assert_eq!(
            result.output_fingerprint.as_deref(),
            Some(sha256_hex(b"x = 1\ny = 3\n").as_str())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_write_executor_blocks_without_content() {
        let span = make_span(SpanKind::FileWrite, "output.txt");
        let result = FileWriteExecutor.execute_span(&span, &test_context());
        assert!(matches!(result, Err(ReplayError::Blocked(_))));
    }

    #[test]
    fn unified_diff_has_hunk_headers() {
        let diff = unified_diff("f", "a\nb\nc\n", "a\nB\nc\nd\n");
        assert_eq!(
            diff,
            "--- a/f\n+++ b/f\n@@ -1,3 +1,4 @@\n a\n-b\n+B\n c\n+d\n"
        );
        assert_eq!(unified_diff("f", "same\n", "same\n"), "--- a/f\n+++ b/f\n");
    }

    #[test]
    fn file_read_uses_path_attribute() {
        let dir = std::env::temp_dir().join("rk-test-contract-read");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("contract.txt");
        std::fs::write(&file, "data\n").unwrap();
        let span = with_attr(
            make_span(SpanKind::FileRead, "wrong name"),
            attrs::PATH,
            &file.to_string_lossy(),
        );
        let result = FileReadExecutor
            .execute_span(&span, &test_context())
            .unwrap();
        assert_eq!(result.status, SpanStatus::Completed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- PlannerStepExecutor --------------------------------------------------

    fn child(id: &str, recorded: SpanStatus, replayed: SpanStatus) -> ChildOutcome {
        let mut a = make_span(SpanKind::ShellCommand, id);
        a.span_id = SpanId(id.into());
        a.status = recorded;
        let mut b = a.clone();
        b.status = replayed;
        if replayed == SpanStatus::Failed {
            b.error_summary = Some("boom".into());
        }
        ChildOutcome {
            recorded: a,
            replayed: b,
        }
    }

    fn planner_status(recorded: SpanStatus, children: Vec<ChildOutcome>) -> SpanStatus {
        let mut span = make_span(SpanKind::PlannerStep, "plan");
        span.status = recorded;
        let mut ctx = test_context();
        ctx.recorded = Some(RecordedSpan {
            span: span.clone(),
            outputs: Vec::new(),
        });
        ctx.children = children;
        PlannerStepExecutor
            .execute_span(&span, &ctx)
            .unwrap()
            .status
    }

    #[test]
    fn planner_step_rolls_up_children() {
        use SpanStatus::*;
        // Unchanged children keep the recorded status (even an expected failure).
        assert_eq!(
            planner_status(Completed, vec![child("t", Failed, Failed)]),
            Completed
        );
        // The failing child now passes: the step completes.
        assert_eq!(
            planner_status(
                Failed,
                vec![
                    child("t", Failed, Completed),
                    child("d", Completed, Completed)
                ]
            ),
            Completed
        );
        // A child that used to pass now fails.
        assert_eq!(
            planner_status(Completed, vec![child("t", Completed, Failed)]),
            Failed
        );
        // Any blocked child blocks the step.
        assert_eq!(
            planner_status(Completed, vec![child("t", Completed, Blocked)]),
            Blocked
        );
    }

    // -- Model executors ------------------------------------------------------

    fn llm_span(prompt: &str) -> SpanRecord {
        with_attr(
            with_attr(
                make_span(SpanKind::LlmCall, "Generate fix"),
                attrs::MODEL,
                "m1",
            ),
            attrs::PROMPT,
            prompt,
        )
    }

    #[test]
    fn blocked_model_executor_reports_model_name() {
        let span = with_attr(
            make_span(SpanKind::LlmCall, "test"),
            attrs::MODEL,
            "claude-sonnet-4-6",
        );
        match BlockedModelExecutor.execute_span(&span, &test_context()) {
            Err(ReplayError::Blocked(msg)) => assert!(msg.contains("claude-sonnet-4-6"), "{msg}"),
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    #[test]
    fn fake_model_returns_completed_with_artifact() {
        let fake = FakeModelExecutor::new("default answer");
        let span = with_attr(
            make_span(SpanKind::LlmCall, "test call"),
            attrs::MODEL,
            "gpt-5.4",
        );
        let result = fake.execute_span(&span, &test_context()).unwrap();
        assert_eq!(result.status, SpanStatus::Completed);
        assert_eq!(
            result.output_artifacts[0].artifact_type,
            ArtifactType::ModelResponse
        );
        assert_eq!(result.output_artifacts[0].mime, "text/plain");
        assert_eq!(
            result.output_artifacts[0].summary.get("fake"),
            Some(&Value::Text("true".into()))
        );
    }

    #[test]
    fn fake_model_per_span_override() {
        let fake = FakeModelExecutor::new("default").with_response("generate fix", "custom fix");
        let span1 = with_attr(
            make_span(SpanKind::LlmCall, "generate fix"),
            attrs::MODEL,
            "m",
        );
        let span2 = with_attr(
            make_span(SpanKind::LlmCall, "other call"),
            attrs::MODEL,
            "m",
        );
        let r1 = fake.execute_span(&span1, &test_context()).unwrap();
        let r2 = fake.execute_span(&span2, &test_context()).unwrap();
        assert_ne!(r1.output_fingerprint, r2.output_fingerprint);
    }

    #[test]
    fn fake_model_blocks_without_minimum_contract() {
        let fake = FakeModelExecutor::new("default");
        let span = make_span(SpanKind::LlmCall, "test call");
        match fake.execute_span(&span, &test_context()) {
            Err(ReplayError::Blocked(msg)) => assert!(msg.contains("model"), "{msg}"),
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    #[test]
    fn recorded_model_replays_fixtures_and_unchanged_prompts() {
        let executor = RecordedModelExecutor::new(vec![ModelFixture {
            model: Some("m1".into()),
            prompt: "edited".into(),
            response: "fixture answer".into(),
        }]);
        let ctx = test_context();
        let edited = executor.execute_span(&llm_span("edited"), &ctx).unwrap();
        assert_eq!(stdout_of(&edited), "fixture answer");

        // Unknown prompt without a recording: blocked, and check() agrees.
        let unknown = llm_span("something else");
        assert!(executor.check(&unknown, &ctx).is_err());
        assert!(matches!(
            executor.execute_span(&unknown, &ctx),
            Err(ReplayError::Blocked(msg)) if msg.contains("no recorded response")
        ));

        // The span's own recorded response is replayed when the prompt is unchanged.
        let original = llm_span("original");
        let mut ctx = test_context();
        ctx.recorded = Some(RecordedSpan {
            span: original.clone(),
            outputs: vec![RecordedOutput {
                artifact: ArtifactRecord {
                    artifact_id: ArtifactId("a".into()),
                    run_id: original.run_id.clone(),
                    span_id: Some(original.span_id.clone()),
                    artifact_type: ArtifactType::ModelResponse,
                    mime: "text/plain".into(),
                    sha256: String::new(),
                    byte_len: 8,
                    blob_path: String::new(),
                    summary: Document::new(),
                    redaction: Document::new(),
                    created_at: 0,
                },
                content: Some(b"recorded".to_vec()),
            }],
        });
        assert_eq!(
            stdout_of(&executor.execute_span(&original, &ctx).unwrap()),
            "recorded"
        );
        assert!(
            executor
                .check(&llm_span("original but edited"), &ctx)
                .is_err()
        );
    }

    #[test]
    fn command_model_executor_pipes_request_json() {
        let executor = CommandModelExecutor::new(
            "python3 -c 'import json,sys; r=json.load(sys.stdin); print(r[\"model\"], r[\"prompt\"].upper(), end=\"\")'",
        );
        let result = executor
            .execute_span(&llm_span("fix it"), &test_context())
            .unwrap();
        assert_eq!(result.status, SpanStatus::Completed);
        assert_eq!(stdout_of(&result), "m1 FIX IT");

        let failing = CommandModelExecutor::new("echo nope >&2; exit 3");
        let result = failing
            .execute_span(&llm_span("x"), &test_context())
            .unwrap();
        assert_eq!(result.status, SpanStatus::Failed);
        assert!(result.error_summary.unwrap().contains("nope"));
    }

    // -- CompositeExecutorRegistry --------------------------------------------

    #[test]
    fn composite_supports_executable_kinds_and_containers() {
        let registry = CompositeExecutorRegistry::new();
        for kind in [
            SpanKind::ShellCommand,
            SpanKind::ToolCall,
            SpanKind::FileRead,
            SpanKind::FileWrite,
            SpanKind::LlmCall,
            SpanKind::PlannerStep,
            SpanKind::Run,
        ] {
            assert!(registry.supports(&make_span(kind, "")), "{kind:?}");
        }
        assert!(!registry.supports(&make_span(SpanKind::HumanInput, "")));
        assert!(
            registry
                .check(&make_span(SpanKind::HumanInput, ""), &test_context())
                .is_err()
        );
    }

    #[test]
    fn composite_default_blocks_llm_with_explanation_in_check_and_execute() {
        let registry = CompositeExecutorRegistry::new();
        let span = with_attr(
            make_span(SpanKind::LlmCall, "test"),
            attrs::MODEL,
            "test-model",
        );
        let checked = registry.check(&span, &test_context()).unwrap_err();
        assert!(checked.contains("test-model"), "{checked}");
        match registry.execute(&span, &test_context()) {
            Err(ReplayError::Blocked(msg)) => assert_eq!(msg, checked),
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    #[test]
    fn composite_with_fake_model_executes_llm() {
        let registry = CompositeExecutorRegistry::new().with_model_mode(ModelExecutorMode::Fake(
            FakeModelExecutor::new("test response"),
        ));
        let span = with_attr(
            make_span(SpanKind::LlmCall, "test call"),
            attrs::MODEL,
            "gpt-5.4",
        );
        assert!(registry.check(&span, &test_context()).is_ok());
        let result = registry.execute(&span, &test_context()).unwrap();
        assert_eq!(result.status, SpanStatus::Completed);
        assert_eq!(
            result.output_artifacts[0].artifact_type,
            ArtifactType::ModelResponse
        );
    }

    // -- Contract diagnostics -------------------------------------------------

    #[test]
    fn shell_warns_on_name_fallback() {
        let diag = validate_shell_contract(&make_span(SpanKind::ShellCommand, "echo hello"));
        assert!(diag.block.is_none());
        assert!(diag.warnings.iter().any(|w| w.contains("command")));
    }

    #[test]
    fn shell_warns_on_missing_cwd() {
        let span = with_attr(
            make_span(SpanKind::ShellCommand, "x"),
            attrs::COMMAND,
            "echo hello",
        );
        let diag = validate_shell_contract(&span);
        assert!(diag.block.is_none());
        assert!(diag.warnings.iter().any(|w| w.contains("cwd")));
    }

    #[test]
    fn shell_no_warnings_when_fully_specified() {
        let span = with_attr(
            with_attr(
                make_span(SpanKind::ShellCommand, "x"),
                attrs::COMMAND,
                "echo hello",
            ),
            attrs::CWD,
            "/tmp",
        );
        assert!(validate_shell_contract(&span).warnings.is_empty());
    }

    #[test]
    fn file_write_blocks_on_missing_content_at_contract() {
        let span = with_attr(
            make_span(SpanKind::FileWrite, "output.txt"),
            attrs::PATH,
            "output.txt",
        );
        let diag = validate_file_write_contract(&span);
        assert!(diag.block.as_ref().unwrap().contains("content"));
    }

    #[test]
    fn llm_warns_on_missing_provider_and_prompt() {
        let span = with_attr(
            make_span(SpanKind::LlmCall, "test"),
            attrs::MODEL,
            "gpt-5.4",
        );
        let diag = validate_llm_contract(&span);
        assert!(diag.block.is_none());
        assert!(diag.warnings.iter().any(|w| w.contains("provider")));
        assert!(
            diag.warnings
                .iter()
                .any(|w| w.contains("model_request_json"))
        );
    }
}
