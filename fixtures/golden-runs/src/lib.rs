use std::sync::Arc;

use replaykit_core_model::{
    ArtifactRecord, ArtifactType, EdgeKind, RunId, RunRecord, RunStatus, SnapshotRecord,
    SpanEdgeRecord, SpanRecord,
};
use replaykit_sdk_rust_tracing::{
    SemanticSession, file_read, file_write, human_input, model_call, planner_step, shell_command,
    summary_from_pairs,
};
use replaykit_storage::{InMemoryStorage, Storage};

// ---------------------------------------------------------------------------
// FixtureRun – all data from a single captured run
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct FixtureRun {
    pub run: RunRecord,
    pub spans: Vec<SpanRecord>,
    pub artifacts: Vec<ArtifactRecord>,
    pub edges: Vec<SpanEdgeRecord>,
    pub snapshots: Vec<SnapshotRecord>,
}

impl FixtureRun {
    pub fn span_by_id(&self, id: &str) -> &SpanRecord {
        self.spans
            .iter()
            .find(|s| s.span_id.0 == id)
            .unwrap_or_else(|| panic!("span {id} not found"))
    }

    pub fn data_deps_from(&self, span_id: &str) -> Vec<&SpanEdgeRecord> {
        self.edges
            .iter()
            .filter(|e| e.from_span_id.0 == span_id && e.kind == EdgeKind::DataDependsOn)
            .collect()
    }
}

fn extract(
    storage: &Arc<InMemoryStorage>,
    run_id: &RunId,
) -> (
    Vec<SpanRecord>,
    Vec<ArtifactRecord>,
    Vec<SpanEdgeRecord>,
    Vec<SnapshotRecord>,
) {
    (
        storage.list_spans(run_id).unwrap(),
        storage.list_artifacts(run_id).unwrap(),
        storage.list_edges(run_id).unwrap(),
        storage.list_snapshots(run_id).unwrap(),
    )
}

// ---------------------------------------------------------------------------
// 1. Failed coding-agent run
// ---------------------------------------------------------------------------

/// A coding agent that reads a test file, asks an LLM for a fix, writes the
/// patch, and runs `cargo test` -- which fails.
///
/// Dependency graph:
/// ```text
///   planner
///   ├── file_read("test_auth.rs")
///   ├── llm("generate fix")        ── DataDependsOn ──> file_read
///   ├── file_write("apply patch")   ── DataDependsOn ──> llm
///   └── shell("cargo test") [FAIL]  ── DataDependsOn ──> file_write
/// ```
///
/// The `shell` span is the natural branch point: change the file_write
/// output and rerun.
pub fn generate_failed_coding_agent() -> FixtureRun {
    let storage = Arc::new(InMemoryStorage::new());
    let session =
        SemanticSession::start(storage.clone(), "fix failing tests", "agent.main", 100).unwrap();
    let run_id = session.run().run_id.clone();

    // -- 1. Planner --------------------------------------------------------
    let planner = session
        .record_completed_span(
            planner_step("plan fix")
                .span_id("planner-001")
                .times(100, 110)
                .output(
                    ArtifactType::DebugLog,
                    summary_from_pairs(&[("plan", "read test, generate fix, apply, verify")]),
                )
                .output_fingerprint("plan-v1")
                .build(),
        )
        .unwrap();

    // -- 2. File read ------------------------------------------------------
    let fread = session
        .record_completed_span(
            file_read("read test_auth.rs")
                .path("tests/test_auth.rs")
                .span_id("file-read-001")
                .parent(&planner.span_id)
                .times(111, 115)
                .output(
                    ArtifactType::FileBlob,
                    summary_from_pairs(&[("path", "tests/test_auth.rs"), ("lines", "42")]),
                )
                .output_fingerprint("file-sha256-abc123")
                .build(),
        )
        .unwrap();

    // -- 3. LLM call -------------------------------------------------------
    let llm = session
        .record_completed_span(
            model_call("generate fix")
                .provider("anthropic")
                .model("claude-sonnet-4-6")
                .model_request_json(
                    r#"{"messages":[{"role":"user","content":"fix the failing auth test"}]}"#,
                )
                .span_id("llm-001")
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
                        (
                            "content_preview",
                            "fn test_auth() { assert!(login(\"user\", \"pass\").is_ok()); }",
                        ),
                    ]),
                )
                .output_fingerprint("response-hash-ghi789")
                .cost(1200, 350, 4500)
                .build(),
        )
        .unwrap();

    // LLM depends on file read output.
    session
        .add_dependency(llm.span_id.clone(), fread.span_id.clone())
        .unwrap();

    // -- 4. File write -----------------------------------------------------
    let fwrite = session
        .record_completed_span(
            file_write("apply patch")
                .path("tests/test_auth.rs")
                .write_content("fn test_auth() { assert!(login(\"user\", \"pass\").is_ok()); }")
                .span_id("file-write-001")
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
        .unwrap();

    // File write depends on LLM output.
    session
        .add_dependency(fwrite.span_id.clone(), llm.span_id.clone())
        .unwrap();

    // -- 5. Shell command (FAILS) ------------------------------------------
    let shell = session
        .record_completed_span(
            shell_command("cargo test")
                .command("cargo test --test auth")
                .span_id("shell-001")
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
        .unwrap();

    // Shell depends on file write.
    session
        .add_dependency(shell.span_id.clone(), fwrite.span_id.clone())
        .unwrap();

    // -- Finish as Failed --------------------------------------------------
    let run = session.finish(161, RunStatus::Failed).unwrap();

    let (spans, artifacts, edges, snapshots) = extract(&storage, &run_id);
    FixtureRun {
        run,
        spans,
        artifacts,
        edges,
        snapshots,
    }
}

// ---------------------------------------------------------------------------
// 2. Successful coding-agent run (paired with the failed run)
// ---------------------------------------------------------------------------

/// Same structure as the failed run, but the shell command succeeds.
/// Useful for diff-engine testing (compare failed vs successful).
pub fn generate_success_coding_agent() -> FixtureRun {
    let storage = Arc::new(InMemoryStorage::new());
    let session = SemanticSession::start(
        storage.clone(),
        "fix failing tests (retry)",
        "agent.main",
        200,
    )
    .unwrap();
    let run_id = session.run().run_id.clone();

    let planner = session
        .record_completed_span(
            planner_step("plan fix")
                .span_id("planner-001")
                .times(200, 210)
                .output(
                    ArtifactType::DebugLog,
                    summary_from_pairs(&[("plan", "read test, generate fix, apply, verify")]),
                )
                .output_fingerprint("plan-v1")
                .build(),
        )
        .unwrap();

    let fread = session
        .record_completed_span(
            file_read("read test_auth.rs")
                .path("tests/test_auth.rs")
                .span_id("file-read-001")
                .parent(&planner.span_id)
                .times(211, 215)
                .output(
                    ArtifactType::FileBlob,
                    summary_from_pairs(&[("path", "tests/test_auth.rs"), ("lines", "42")]),
                )
                .output_fingerprint("file-sha256-abc123")
                .build(),
        )
        .unwrap();

    let llm = session
        .record_completed_span(
            model_call("generate fix")
                .provider("openai")
                .model("gpt-5.4")
                .model_request_json(
                    r#"{"messages":[{"role":"user","content":"fix the failing auth test"}]}"#,
                )
                .span_id("llm-001")
                .parent(&planner.span_id)
                .times(216, 230)
                .executor("gpt-5.4", "2025-06-01")
                .input(
                    ArtifactType::ModelRequest,
                    summary_from_pairs(&[
                        ("model", "gpt-5.4"),
                        ("prompt_tokens", "1250"),
                    ]),
                )
                .input_fingerprint("prompt-hash-def456-v2")
                .output(
                    ArtifactType::ModelResponse,
                    summary_from_pairs(&[
                        ("response_tokens", "400"),
                        ("content_preview", "fn test_auth() { let result = login(\"user\", \"pass\"); assert!(result.is_ok()); }"),
                    ]),
                )
                .output_fingerprint("response-hash-xyz999")
                .cost(1250, 400, 5000)
                .build(),
        )
        .unwrap();

    session
        .add_dependency(llm.span_id.clone(), fread.span_id.clone())
        .unwrap();

    let fwrite = session
        .record_completed_span(
            file_write("apply patch")
                .path("tests/test_auth.rs")
                .write_content("fn test_auth() { let result = login(\"user\", \"pass\"); assert!(result.is_ok()); }")
                .span_id("file-write-001")
                .parent(&planner.span_id)
                .times(231, 240)
                .input(
                    ArtifactType::FileDiff,
                    summary_from_pairs(&[("path", "tests/test_auth.rs"), ("hunks", "1")]),
                )
                .output(
                    ArtifactType::FileDiff,
                    summary_from_pairs(&[("path", "tests/test_auth.rs"), ("lines_changed", "7")]),
                )
                .output_fingerprint("diff-hash-jkl012-v2")
                .build(),
        )
        .unwrap();

    session
        .add_dependency(fwrite.span_id.clone(), llm.span_id.clone())
        .unwrap();

    // Shell succeeds this time.
    let shell = session
        .record_completed_span(
            shell_command("cargo test")
                .command("cargo test --test auth")
                .span_id("shell-001")
                .parent(&planner.span_id)
                .times(241, 260)
                .executor("shell", "bash-5.2")
                .input_fingerprint("env-hash-mno345")
                .output(
                    ArtifactType::ShellStdout,
                    summary_from_pairs(&[
                        ("exit_code", "0"),
                        (
                            "stdout",
                            "test test_auth::test_login ... ok\n\ntest result: ok. 1 passed",
                        ),
                    ]),
                )
                .output_fingerprint("test-output-hash-stu901")
                .build(),
        )
        .unwrap();

    session
        .add_dependency(shell.span_id.clone(), fwrite.span_id.clone())
        .unwrap();

    let run = session.finish(261, RunStatus::Completed).unwrap();

    let (spans, artifacts, edges, snapshots) = extract(&storage, &run_id);
    FixtureRun {
        run,
        spans,
        artifacts,
        edges,
        snapshots,
    }
}

// ---------------------------------------------------------------------------
// 3. Simple recorded-only run (no rerunnable spans)
// ---------------------------------------------------------------------------

/// Minimal run with a planner step and a human-input span.
/// All spans are `RecordOnly` — useful for UI testing and basic validation.
pub fn generate_simple_recorded() -> FixtureRun {
    let storage = Arc::new(InMemoryStorage::new());
    let session =
        SemanticSession::start(storage.clone(), "simple recorded session", "demo.main", 300)
            .unwrap();
    let run_id = session.run().run_id.clone();

    let planner = session
        .record_completed_span(
            planner_step("initial plan")
                .span_id("plan-simple")
                .times(300, 310)
                .output(
                    ArtifactType::DebugLog,
                    summary_from_pairs(&[("plan", "ask user for clarification")]),
                )
                .output_fingerprint("plan-simple-v1")
                .build(),
        )
        .unwrap();

    session
        .record_completed_span(
            human_input("user prompt")
                .span_id("human-001")
                .parent(&planner.span_id)
                .times(311, 320)
                .output(
                    ArtifactType::HumanMessage,
                    summary_from_pairs(&[("message", "please fix the auth module")]),
                )
                .output_fingerprint("human-msg-hash-001")
                .build(),
        )
        .unwrap();

    let run = session.finish(321, RunStatus::Completed).unwrap();

    let (spans, artifacts, edges, snapshots) = extract(&storage, &run_id);
    FixtureRun {
        run,
        spans,
        artifacts,
        edges,
        snapshots,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use replaykit_core_model::{SpanId, SpanStatus};

    use super::*;

    // -- Determinism -------------------------------------------------------

    #[test]
    fn failed_run_is_deterministic() {
        let a = generate_failed_coding_agent();
        let b = generate_failed_coding_agent();

        assert_eq!(a.run.run_id, b.run.run_id, "run_id differs");
        assert_eq!(a.spans.len(), b.spans.len(), "span count differs");
        for (sa, sb) in a.spans.iter().zip(&b.spans) {
            assert_eq!(sa.span_id, sb.span_id, "span_id mismatch");
            assert_eq!(sa.kind, sb.kind, "kind mismatch for {}", sa.span_id.0);
            assert_eq!(sa.started_at, sb.started_at);
            assert_eq!(sa.ended_at, sb.ended_at);
        }
        assert_eq!(a.artifacts.len(), b.artifacts.len());
        assert_eq!(a.edges.len(), b.edges.len());
    }

    #[test]
    fn success_run_is_deterministic() {
        let a = generate_success_coding_agent();
        let b = generate_success_coding_agent();

        assert_eq!(a.run.run_id, b.run.run_id);
        assert_eq!(a.spans.len(), b.spans.len());
    }

    #[test]
    fn simple_run_is_deterministic() {
        let a = generate_simple_recorded();
        let b = generate_simple_recorded();

        assert_eq!(a.run.run_id, b.run.run_id);
        assert_eq!(a.spans.len(), b.spans.len());
    }

    // -- Structural properties: failed run ---------------------------------

    #[test]
    fn failed_run_has_expected_shape() {
        let f = generate_failed_coding_agent();

        assert_eq!(f.run.status, RunStatus::Failed);
        assert_eq!(f.spans.len(), 5, "expected 5 spans");

        // All spans are children of planner (except planner itself).
        let planner = f.span_by_id("planner-001");
        assert_eq!(planner.parent_span_id, None);

        for id in &["file-read-001", "llm-001", "file-write-001", "shell-001"] {
            let span = f.span_by_id(id);
            assert_eq!(
                span.parent_span_id,
                Some(SpanId("planner-001".into())),
                "{id} should be child of planner"
            );
        }

        // Shell span is failed.
        let shell = f.span_by_id("shell-001");
        assert_eq!(shell.status, SpanStatus::Failed);
        assert!(shell.error_summary.is_some());

        // Shell span is branchable (RerunnableSupported).
        assert_eq!(
            shell.replay_policy,
            replaykit_core_model::ReplayPolicy::RerunnableSupported
        );
    }

    #[test]
    fn failed_run_has_dependency_chain() {
        let f = generate_failed_coding_agent();

        // LLM depends on file read.
        let llm_deps = f.data_deps_from("llm-001");
        assert_eq!(llm_deps.len(), 1);
        assert_eq!(llm_deps[0].to_span_id, SpanId("file-read-001".into()));

        // File write depends on LLM.
        let fw_deps = f.data_deps_from("file-write-001");
        assert_eq!(fw_deps.len(), 1);
        assert_eq!(fw_deps[0].to_span_id, SpanId("llm-001".into()));

        // Shell depends on file write.
        let sh_deps = f.data_deps_from("shell-001");
        assert_eq!(sh_deps.len(), 1);
        assert_eq!(sh_deps[0].to_span_id, SpanId("file-write-001".into()));
    }

    #[test]
    fn failed_run_has_artifacts() {
        let f = generate_failed_coding_agent();

        // 1 planner output + 1 file_read output + 2 llm (in+out) + 2 file_write (in+out) + 1 shell output = 7
        // Actually: planner output(1), file_read output(1), llm input(1)+output(1), file_write input(1)+output(1), shell output(1) = 7
        assert!(
            f.artifacts.len() >= 7,
            "expected >= 7 artifacts, got {}",
            f.artifacts.len()
        );
    }

    // -- Structural properties: success run --------------------------------

    #[test]
    fn success_run_completes() {
        let f = generate_success_coding_agent();

        assert_eq!(f.run.status, RunStatus::Completed);
        assert_eq!(f.spans.len(), 5);

        let shell = f.span_by_id("shell-001");
        assert_eq!(shell.status, SpanStatus::Completed);
        assert!(shell.error_summary.is_none());
    }

    // -- Structural properties: simple run ---------------------------------

    #[test]
    fn simple_run_is_minimal() {
        let f = generate_simple_recorded();

        assert_eq!(f.run.status, RunStatus::Completed);
        assert_eq!(f.spans.len(), 2);
        assert!(
            f.edges.is_empty(),
            "simple run should have no dependency edges"
        );

        let human = f.span_by_id("human-001");
        assert_eq!(human.kind, replaykit_core_model::SpanKind::HumanInput);
        assert_eq!(
            human.replay_policy,
            replaykit_core_model::ReplayPolicy::RecordOnly,
        );
    }

    // -- Cross-fixture: paired runs are diff-able --------------------------

    #[test]
    fn failed_and_success_have_matching_span_ids() {
        let failed = generate_failed_coding_agent();
        let success = generate_success_coding_agent();

        // Both runs share the same logical span IDs (set via span_id builder).
        let failed_ids: Vec<_> = failed.spans.iter().map(|s| &s.span_id).collect();
        let success_ids: Vec<_> = success.spans.iter().map(|s| &s.span_id).collect();
        assert_eq!(failed_ids, success_ids);
    }

    #[test]
    fn prompt_edit_with_fake_model_unblocks_llm_span() {
        use replaykit_core_model::{BranchRequest, PatchManifest, PatchType, Value};
        use replaykit_replay_engine::ReplayEngine;
        use replaykit_replay_engine::executors::{
            CompositeExecutorRegistry, FakeModelExecutor, ModelExecutorMode,
        };

        let fixture = generate_failed_coding_agent();
        let storage = Arc::new(InMemoryStorage::new());

        // Load fixture into storage
        storage.insert_run(fixture.run.clone()).unwrap();
        for span in &fixture.spans {
            storage.upsert_span(span.clone()).unwrap();
        }
        for artifact in &fixture.artifacts {
            storage.insert_artifact(artifact.clone()).unwrap();
        }
        for edge in &fixture.edges {
            storage.insert_edge(edge.clone()).unwrap();
        }

        // Advance ID counters past fixture-allocated IDs so execute_fork
        // doesn't generate colliding IDs.
        use replaykit_core_model::IdKind;
        for kind in [
            IdKind::Run,
            IdKind::Trace,
            IdKind::Branch,
            IdKind::ReplayJob,
            IdKind::Diff,
            IdKind::Snapshot,
            IdKind::Event,
        ] {
            let _ = storage.allocate_id(kind);
        }
        for _ in 0..fixture.artifacts.len() + 2 {
            let _ = storage.allocate_id(IdKind::Artifact);
        }
        for _ in 0..fixture.edges.len() + 2 {
            let _ = storage.allocate_id(IdKind::Edge);
        }

        let fake = FakeModelExecutor::new("fn test_auth() { assert!(true); }")
            .with_response("generate fix", "fn test_auth() { assert!(true); }");
        let registry =
            CompositeExecutorRegistry::new().with_model_mode(ModelExecutorMode::Fake(fake));
        let engine = ReplayEngine::new(storage.clone(), registry);

        let request = BranchRequest {
            source_run_id: fixture.run.run_id.clone(),
            fork_span_id: SpanId("llm-001".into()),
            patch_manifest: PatchManifest {
                patch_type: PatchType::PromptEdit,
                target_artifact_id: None,
                replacement: Value::Text("generate a better fix".into()),
                note: Some("testing prompt edit".into()),
                created_at: 500,
            },
            created_by: Some("test".into()),
        };

        let execution = engine.execute_fork(request).unwrap();

        // The LLM span itself should NOT be blocked (FakeModelExecutor handles it).
        // Downstream spans may fail/block in test environment (no real files/commands).
        let llm_span = storage
            .get_span(&execution.target_run.run_id, &SpanId("llm-001".into()))
            .unwrap();
        assert_eq!(
            llm_span.status,
            SpanStatus::Completed,
            "LLM span should be Completed via FakeModelExecutor, got: {:?} / {:?}",
            llm_span.status,
            llm_span.error_summary,
        );
        assert!(
            llm_span.output_fingerprint.is_some(),
            "LLM span should have a new output fingerprint"
        );
        assert_eq!(llm_span.output_artifact_ids.len(), 1);

        // The plan should show llm-001 as dirty (patched) and downstream spans too
        let dirty_ids: Vec<_> = execution
            .plan
            .dirty_spans
            .iter()
            .map(|d| d.span_id.0.as_str())
            .collect();
        assert!(dirty_ids.contains(&"llm-001"), "llm should be dirty");
    }

    #[test]
    fn failed_and_success_differ_at_shell_output() {
        let failed = generate_failed_coding_agent();
        let success = generate_success_coding_agent();

        let f_shell = failed.span_by_id("shell-001");
        let s_shell = success.span_by_id("shell-001");

        assert_ne!(f_shell.status, s_shell.status);
        assert_ne!(f_shell.output_fingerprint, s_shell.output_fingerprint);
    }
}
