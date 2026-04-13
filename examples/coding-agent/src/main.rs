use std::sync::Arc;

use replaykit_api::ReplayKitService;
use replaykit_core_model::{ArtifactType, RunStatus, RunTreeNode};
use replaykit_replay_engine::executors::CompositeExecutorRegistry;
use replaykit_sdk_rust_tracing::{
    SemanticSession, file_read, file_write, model_call, planner_step, shell_command,
    summary_from_pairs,
};
use replaykit_storage::InMemoryStorage;

/// Build a realistic coding-agent run and return the session storage.
///
/// The run simulates:
///   1. A planner deciding what to do
///   2. Reading a test file
///   3. Asking an LLM for a fix
///   4. Writing the fix back
///   5. Running tests (which fail)
///
/// Dependency chain:
///   llm -> file_read, file_write -> llm, shell -> file_write
fn build_run(storage: Arc<InMemoryStorage>) -> replaykit_core_model::RunRecord {
    let session = SemanticSession::start(storage.clone(), "fix failing tests", "agent.main", 100)
        .expect("create session");

    // 1. Planner
    let planner = session
        .record_completed_span(
            planner_step("plan fix")
                .span_id("planner")
                .times(100, 110)
                .output(
                    ArtifactType::DebugLog,
                    summary_from_pairs(&[
                        ("plan", "read test, generate fix, apply, verify"),
                        ("goal", "fix failing tests in test_auth.rs"),
                    ]),
                )
                .output_fingerprint("plan-v1")
                .build(),
        )
        .expect("planner");

    // 2. File read
    let fread = session
        .record_completed_span(
            file_read("read test_auth.rs")
                .path("tests/test_auth.rs")
                .span_id("file-read")
                .parent(&planner.span_id)
                .times(111, 115)
                .output(
                    ArtifactType::FileBlob,
                    summary_from_pairs(&[("path", "tests/test_auth.rs"), ("lines", "42")]),
                )
                .output_fingerprint("file-sha256-abc123")
                .build(),
        )
        .expect("file-read");

    // 3. LLM call
    let llm = session
        .record_completed_span(
            model_call("generate fix")
                .provider("anthropic")
                .model("claude-sonnet-4-6")
                .model_request_json(
                    r#"{"messages":[{"role":"user","content":"fix the failing auth test"}]}"#,
                )
                .span_id("llm-call")
                .parent(&planner.span_id)
                .times(116, 130)
                .executor("claude-sonnet-4-6", "2025-05-14")
                .input(
                    ArtifactType::ModelRequest,
                    summary_from_pairs(&[
                        ("model", "claude-sonnet-4-6"),
                        ("prompt_tokens", "1200"),
                    ]),
                )
                .input_fingerprint("prompt-hash-def456")
                .output(
                    ArtifactType::ModelResponse,
                    summary_from_pairs(&[
                        ("response_tokens", "350"),
                        ("content_preview", "fn test_auth() { ... }"),
                    ]),
                )
                .output_fingerprint("response-hash-ghi789")
                .cost(1200, 350, 4500)
                .build(),
        )
        .expect("llm-call");

    session
        .add_dependency(llm.span_id.clone(), fread.span_id.clone())
        .expect("llm depends on file-read");

    // 4. File write
    let fwrite = session
        .record_completed_span(
            file_write("apply patch")
                .path("tests/test_auth.rs")
                .write_content("fn test_auth() { assert!(login(\"user\", \"pass\").is_ok()); }")
                .span_id("file-write")
                .parent(&planner.span_id)
                .times(131, 140)
                .input(
                    ArtifactType::FileDiff,
                    summary_from_pairs(&[("path", "tests/test_auth.rs"), ("hunks", "1")]),
                )
                .output(
                    ArtifactType::FileDiff,
                    summary_from_pairs(&[("path", "tests/test_auth.rs"), ("lines_changed", "5")]),
                )
                .output_fingerprint("diff-hash-jkl012")
                .build(),
        )
        .expect("file-write");

    session
        .add_dependency(fwrite.span_id.clone(), llm.span_id.clone())
        .expect("file-write depends on llm");

    // 5. Shell command (FAILS)
    let shell = session
        .record_completed_span(
            shell_command("cargo test")
                .command("cargo test --test auth")
                .cwd("/workspace/project")
                .timeout_ms(30_000)
                .span_id("shell-test")
                .parent(&planner.span_id)
                .times(141, 160)
                .executor("shell", "bash-5.2")
                .input_fingerprint("env-hash-mno345")
                .output(
                    ArtifactType::ShellStderr,
                    summary_from_pairs(&[
                        ("exit_code", "1"),
                        ("stderr", "test test_auth::test_login ... FAILED"),
                    ]),
                )
                .output_fingerprint("test-output-hash-pqr678")
                .failed("cargo test exited with code 1: 1 test failed")
                .build(),
        )
        .expect("shell");

    session
        .add_dependency(shell.span_id.clone(), fwrite.span_id.clone())
        .expect("shell depends on file-write");

    session.finish(161, RunStatus::Failed).expect("finish run")
}

fn main() {
    let storage = Arc::new(InMemoryStorage::new());
    let run = build_run(storage.clone());

    println!("Run: {} ({})", run.title, run.run_id.0);
    println!("Status: {:?}", run.status);
    println!(
        "Summary: {} spans, {} artifacts, {} errors",
        run.summary.span_count, run.summary.artifact_count, run.summary.error_count
    );
    println!(
        "Cost: {} input + {} output tokens (${:.4})",
        run.summary.token_count,
        run.summary.estimated_cost_micros as f64 / 1_000_000.0,
        run.summary.estimated_cost_micros as f64 / 1_000_000.0,
    );

    // Demonstrate that the API service can consume the captured data.
    let service = ReplayKitService::new(storage, CompositeExecutorRegistry::new());
    let runs = service.list_runs().expect("list runs");
    println!("\nStored {} run(s) in memory", runs.len());

    let tree = service.run_tree(&run.run_id).expect("run tree");
    println!("\nRun tree:");
    for node in &tree {
        print_tree(node, 0);
    }

    println!("\nThe shell span (shell-test) is RerunnableSupported and Failed.");
    println!(
        "All replayable spans carry explicit contract attributes (command, cwd, path, content, model)."
    );
    println!("This is the natural branch point for a replay-and-fix workflow.");
}

fn print_tree(node: &RunTreeNode, depth: usize) {
    let indent = "  ".repeat(depth);
    let status = match node.span.status {
        replaykit_core_model::SpanStatus::Failed => " [FAILED]",
        _ => "",
    };
    println!(
        "{}{} ({:?}){}",
        indent, node.span.name, node.span.kind, status
    );
    for child in &node.children {
        print_tree(child, depth + 1);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use replaykit_core_model::{ReplayPolicy, SpanId, SpanKind, SpanStatus};
    use replaykit_storage::Storage;

    #[test]
    fn example_produces_expected_run_shape() {
        let storage = Arc::new(InMemoryStorage::new());
        let run = build_run(storage.clone());

        assert_eq!(run.status, RunStatus::Failed);
        assert_eq!(run.summary.span_count, 5);
        assert!(run.summary.error_count >= 1);
    }

    #[test]
    fn failure_path_exists() {
        let storage = Arc::new(InMemoryStorage::new());
        let run = build_run(storage.clone());

        let spans = storage.list_spans(&run.run_id).unwrap();
        let failed: Vec<_> = spans
            .iter()
            .filter(|s| s.status == SpanStatus::Failed)
            .collect();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].span_id, SpanId("shell-test".into()));
        assert_eq!(failed[0].kind, SpanKind::ShellCommand);
    }

    #[test]
    fn branchable_span_exists() {
        let storage = Arc::new(InMemoryStorage::new());
        let run = build_run(storage.clone());

        let spans = storage.list_spans(&run.run_id).unwrap();
        let shell = spans
            .iter()
            .find(|s| s.span_id == SpanId("shell-test".into()))
            .expect("shell span");

        // Branchable = failed + RerunnableSupported
        assert_eq!(shell.status, SpanStatus::Failed);
        assert_eq!(shell.replay_policy, ReplayPolicy::RerunnableSupported);
    }

    #[test]
    fn dependency_edges_present() {
        let storage = Arc::new(InMemoryStorage::new());
        let run = build_run(storage.clone());

        let edges = storage.list_edges(&run.run_id).unwrap();
        assert_eq!(edges.len(), 3, "expected 3 dependency edges");

        let edge_pairs: Vec<_> = edges
            .iter()
            .map(|e| (e.from_span_id.0.as_str(), e.to_span_id.0.as_str()))
            .collect();
        assert!(edge_pairs.contains(&("llm-call", "file-read")));
        assert!(edge_pairs.contains(&("file-write", "llm-call")));
        assert!(edge_pairs.contains(&("shell-test", "file-write")));
    }

    #[test]
    fn run_tree_structure() {
        let storage = Arc::new(InMemoryStorage::new());
        let run = build_run(storage.clone());

        let service = ReplayKitService::new(storage, CompositeExecutorRegistry::new());
        let tree = service.run_tree(&run.run_id).unwrap();

        // Root should be the planner.
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].span.name, "plan fix");
        assert_eq!(tree[0].children.len(), 4); // file-read, llm, file-write, shell
    }
}
