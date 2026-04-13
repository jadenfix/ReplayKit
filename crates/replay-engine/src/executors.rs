//! Real executor implementations for replay.
//!
//! Each executor handles a specific span kind during forked replay:
//! - `ShellExecutor`: runs shell commands and captures stdout/stderr/exit code
//! - `FileReadExecutor`: reads a file and produces a FileBlob artifact
//! - `FileWriteExecutor`: writes a diff artifact describing what would be written
//! - `CompositeExecutorRegistry`: dispatches to the right executor by SpanKind
//!
//! LLM calls are intentionally NOT supported by CompositeExecutorRegistry.
//! The replay engine handles LLM reuse via fingerprint matching; if a rerun is
//! needed and no API key is configured, the span blocks explicitly.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use replaykit_core_model::{
    ArtifactType, CostMetrics, Document, SpanKind, SpanRecord, SpanStatus, Value,
};
use replaykit_storage::blob::sha256_hex;

use crate::{ExecutionResult, ExecutorRegistry, ProducedArtifact, ReplayError, ReplayExecutionContext};

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

// ---------------------------------------------------------------------------
// ShellExecutor
// ---------------------------------------------------------------------------

/// Executes shell commands via `sh -c` and captures stdout, stderr, exit code.
pub struct ShellExecutor;

impl ShellExecutor {
    pub fn execute_span(
        &self,
        span: &SpanRecord,
        _context: &ReplayExecutionContext,
    ) -> Result<ExecutionResult, ReplayError> {
        let cmd_str = &span.name;
        let now = now_epoch_secs();

        let output = Command::new("sh")
            .arg("-c")
            .arg(cmd_str)
            .output()
            .map_err(|e| ReplayError::Blocked(format!("failed to spawn shell: {e}")))?;

        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let stdout_hash = sha256_hex(&output.stdout);
        let stderr_hash = sha256_hex(&output.stderr);

        let mut artifacts = Vec::new();

        if !output.stdout.is_empty() {
            artifacts.push(ProducedArtifact {
                artifact_type: ArtifactType::ShellStdout,
                mime: "text/plain".into(),
                sha256: stdout_hash.clone(),
                byte_len: output.stdout.len(),
                blob_path: String::new(), // filled by persist_executor_artifacts
                summary: summary(&[
                    ("exit_code", &exit_code.to_string()),
                    ("stdout_preview", &truncate_str(&stdout, 200)),
                ]),
                redaction: Document::new(),
                created_at: now,
            });
        }

        if !output.stderr.is_empty() {
            artifacts.push(ProducedArtifact {
                artifact_type: ArtifactType::ShellStderr,
                mime: "text/plain".into(),
                sha256: stderr_hash.clone(),
                byte_len: output.stderr.len(),
                blob_path: String::new(),
                summary: summary(&[
                    ("exit_code", &exit_code.to_string()),
                    ("stderr_preview", &truncate_str(&stderr, 200)),
                ]),
                redaction: Document::new(),
                created_at: now,
            });
        }

        // If no output at all, still produce a summary artifact.
        if artifacts.is_empty() {
            let content = format!("exit_code={exit_code}");
            let hash = sha256_hex(content.as_bytes());
            artifacts.push(ProducedArtifact {
                artifact_type: ArtifactType::ShellStdout,
                mime: "text/plain".into(),
                sha256: hash,
                byte_len: content.len(),
                blob_path: String::new(),
                summary: summary(&[("exit_code", &exit_code.to_string())]),
                redaction: Document::new(),
                created_at: now,
            });
        }

        // Fingerprint includes content hashes, not lengths, so output changes
        // are always detectable even when sizes happen to match.
        let fingerprint_input = format!(
            "exit={exit_code};stdout={stdout_hash};stderr={stderr_hash}"
        );
        let output_fingerprint = sha256_hex(fingerprint_input.as_bytes());

        let (status, error_summary) = if exit_code == 0 {
            (SpanStatus::Completed, None)
        } else {
            (
                SpanStatus::Failed,
                Some(format!(
                    "command exited with code {exit_code}: {}",
                    truncate_str(&stderr, 200)
                )),
            )
        };

        Ok(ExecutionResult {
            status,
            output_artifacts: artifacts,
            output_fingerprint: Some(output_fingerprint),
            snapshot: None,
            error_summary,
            cost: CostMetrics::default(),
        })
    }
}

// ---------------------------------------------------------------------------
// FileReadExecutor
// ---------------------------------------------------------------------------

/// Reads a file from disk. Expects the span attributes["path"] or the span
/// name to contain the filesystem path.
pub struct FileReadExecutor;

impl FileReadExecutor {
    pub fn execute_span(
        &self,
        span: &SpanRecord,
        _context: &ReplayExecutionContext,
    ) -> Result<ExecutionResult, ReplayError> {
        let path = extract_file_path(span);
        let now = now_epoch_secs();

        let content = std::fs::read(&path)
            .map_err(|e| ReplayError::Blocked(format!("failed to read file {path}: {e}")))?;

        let hash = sha256_hex(&content);
        let line_count = content.iter().filter(|&&b| b == b'\n').count();

        let artifact = ProducedArtifact {
            artifact_type: ArtifactType::FileBlob,
            mime: "application/octet-stream".into(),
            sha256: hash.clone(),
            byte_len: content.len(),
            blob_path: String::new(),
            summary: summary(&[
                ("path", &path),
                ("lines", &line_count.to_string()),
                ("bytes", &content.len().to_string()),
            ]),
            redaction: Document::new(),
            created_at: now,
        };

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

/// Produces a diff artifact describing a file write. Does NOT perform the
/// actual filesystem write -- that requires an explicit side-effect
/// declaration which is not yet implemented. The artifact records what
/// *would* be written so the diff engine can compare it.
pub struct FileWriteExecutor;

impl FileWriteExecutor {
    pub fn execute_span(
        &self,
        span: &SpanRecord,
        _context: &ReplayExecutionContext,
    ) -> Result<ExecutionResult, ReplayError> {
        let path = extract_file_path(span);
        let now = now_epoch_secs();

        let content = extract_write_content(span).unwrap_or_default();
        let hash = sha256_hex(content.as_bytes());

        let artifact = ProducedArtifact {
            artifact_type: ArtifactType::FileDiff,
            mime: "text/plain".into(),
            sha256: hash.clone(),
            byte_len: content.len(),
            blob_path: String::new(),
            summary: summary(&[
                ("path", &path),
                ("bytes", &content.len().to_string()),
            ]),
            redaction: Document::new(),
            created_at: now,
        };

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
// CompositeExecutorRegistry
// ---------------------------------------------------------------------------

/// Dispatches to the appropriate executor based on span kind.
///
/// Supported span kinds: `ShellCommand`, `FileRead`, `FileWrite`.
///
/// `LlmCall` is intentionally NOT supported here. The replay engine already
/// handles LLM reuse via fingerprint matching on the reusable/cacheable
/// path. If a dirty LLM span reaches execution, it means the fingerprint
/// changed and a real API call would be needed -- which should block
/// explicitly rather than making uncontrolled network requests.
pub struct CompositeExecutorRegistry {
    shell: ShellExecutor,
    file_read: FileReadExecutor,
    file_write: FileWriteExecutor,
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
            file_read: FileReadExecutor,
            file_write: FileWriteExecutor,
        }
    }
}

impl ExecutorRegistry for CompositeExecutorRegistry {
    fn supports(&self, span: &SpanRecord) -> bool {
        matches!(
            span.kind,
            SpanKind::ShellCommand | SpanKind::FileRead | SpanKind::FileWrite
        )
    }

    fn execute(
        &self,
        span: &SpanRecord,
        context: &ReplayExecutionContext,
    ) -> Result<ExecutionResult, ReplayError> {
        match span.kind {
            SpanKind::ShellCommand => self.shell.execute_span(span, context),
            SpanKind::FileRead => self.file_read.execute_span(span, context),
            SpanKind::FileWrite => self.file_write.execute_span(span, context),
            _ => Err(ReplayError::Blocked(format!(
                "no executor for span kind {:?}",
                span.kind
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract a file path from span attributes or name.
fn extract_file_path(span: &SpanRecord) -> String {
    if let Some(Value::Text(path)) = span.attributes.get("path") {
        return path.clone();
    }
    let name = &span.name;
    if let Some(rest) = name.strip_prefix("read ").or_else(|| name.strip_prefix("write ")) {
        return rest.to_string();
    }
    name.clone()
}

/// Extract content to write from span attributes.
fn extract_write_content(span: &SpanRecord) -> Option<String> {
    if let Some(Value::Text(content)) = span.attributes.get("content") {
        return Some(content.clone());
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use replaykit_core_model::{ReplayPolicy, SpanId};

    fn test_context() -> ReplayExecutionContext {
        ReplayExecutionContext {
            source_run_id: replaykit_core_model::RunId("src-run".into()),
            target_run_id: replaykit_core_model::RunId("tgt-run".into()),
            fork_span_id: SpanId("fork".into()),
        }
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

    // -- truncate_str ---------------------------------------------------------

    #[test]
    fn truncate_str_ascii() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello world!", 8), "hello...");
    }

    #[test]
    fn truncate_str_multibyte_safe() {
        // Must not panic on multi-byte characters.
        let s = "héllo wörld café";
        let result = truncate_str(s, 8);
        assert!(result.ends_with("..."));
        assert!(result.chars().count() <= 8);
    }

    // -- ShellExecutor --------------------------------------------------------

    #[test]
    fn shell_executor_success() {
        let executor = ShellExecutor;
        let span = make_span(SpanKind::ShellCommand, "echo hello");
        let result = executor.execute_span(&span, &test_context()).unwrap();
        assert_eq!(result.status, SpanStatus::Completed);
        assert!(!result.output_artifacts.is_empty());
        assert!(result.output_fingerprint.is_some());
        assert!(result.error_summary.is_none());
    }

    #[test]
    fn shell_executor_failure() {
        let executor = ShellExecutor;
        let span = make_span(SpanKind::ShellCommand, "exit 1");
        let result = executor.execute_span(&span, &test_context()).unwrap();
        assert_eq!(result.status, SpanStatus::Failed);
        assert!(result.error_summary.is_some());
    }

    #[test]
    fn shell_executor_captures_stdout_and_stderr() {
        let executor = ShellExecutor;
        let span = make_span(SpanKind::ShellCommand, "echo hello && echo err >&2");
        let result = executor.execute_span(&span, &test_context()).unwrap();
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
        let executor = ShellExecutor;
        let r1 = executor
            .execute_span(&make_span(SpanKind::ShellCommand, "echo aaa"), &test_context())
            .unwrap();
        let r2 = executor
            .execute_span(&make_span(SpanKind::ShellCommand, "echo bbb"), &test_context())
            .unwrap();
        // "aaa\n" and "bbb\n" have the same length (4 bytes) but different content.
        // Fingerprints must differ.
        assert_ne!(r1.output_fingerprint, r2.output_fingerprint);
    }

    // -- FileReadExecutor -----------------------------------------------------

    #[test]
    fn file_read_executor_reads_existing_file() {
        let dir = std::env::temp_dir().join("replaykit-test-fread");
        let _ = std::fs::create_dir_all(&dir);
        let file_path = dir.join("test.txt");
        std::fs::write(&file_path, "line1\nline2\n").unwrap();

        let span = make_span(SpanKind::FileRead, &file_path.to_string_lossy());

        let executor = FileReadExecutor;
        let result = executor.execute_span(&span, &test_context()).unwrap();
        assert_eq!(result.status, SpanStatus::Completed);
        assert_eq!(result.output_artifacts.len(), 1);
        assert_eq!(result.output_artifacts[0].artifact_type, ArtifactType::FileBlob);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_read_executor_blocks_on_missing_file() {
        let span = make_span(SpanKind::FileRead, "/nonexistent/path/file.txt");
        let result = FileReadExecutor.execute_span(&span, &test_context());
        assert!(matches!(result, Err(ReplayError::Blocked(_))));
    }

    // -- FileWriteExecutor ----------------------------------------------------

    #[test]
    fn file_write_executor_produces_diff_artifact_without_side_effect() {
        let span = make_span(SpanKind::FileWrite, "output.txt");
        let result = FileWriteExecutor
            .execute_span(&span, &test_context())
            .unwrap();
        assert_eq!(result.status, SpanStatus::Completed);
        assert_eq!(result.output_artifacts.len(), 1);
        assert_eq!(result.output_artifacts[0].artifact_type, ArtifactType::FileDiff);
        // No file should have been created on disk.
    }

    // -- CompositeExecutorRegistry --------------------------------------------

    #[test]
    fn composite_supports_shell_file_read_file_write() {
        let registry = CompositeExecutorRegistry::new();
        assert!(registry.supports(&make_span(SpanKind::ShellCommand, "")));
        assert!(registry.supports(&make_span(SpanKind::FileRead, "")));
        assert!(registry.supports(&make_span(SpanKind::FileWrite, "")));
    }

    #[test]
    fn composite_does_not_support_llm_or_planner() {
        let registry = CompositeExecutorRegistry::new();
        assert!(!registry.supports(&make_span(SpanKind::LlmCall, "")));
        assert!(!registry.supports(&make_span(SpanKind::PlannerStep, "")));
        assert!(!registry.supports(&make_span(SpanKind::ToolCall, "")));
        assert!(!registry.supports(&make_span(SpanKind::HumanInput, "")));
    }
}
