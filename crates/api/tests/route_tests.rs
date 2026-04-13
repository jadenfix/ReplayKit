use std::sync::Arc;

use axum_test::TestServer;
use replaykit_api::ReplayKitService;
use replaykit_api::server::build_router;
use replaykit_collector::{ArtifactSpec, BeginRun, EndSpan, SpanSpec};
use replaykit_core_model::{
    ArtifactType, CostMetrics, Document, HostMetadata, ReplayPolicy, RunId, RunStatus, SpanId,
    SpanKind, SpanStatus,
};
use replaykit_replay_engine::{ExecutorRegistry, ReplayError, ReplayExecutionContext};
use replaykit_storage::{InMemoryStorage, Storage};

// ---------------------------------------------------------------------------
// Test executor that supports LlmCall spans
// ---------------------------------------------------------------------------

struct FakeExecutorRegistry;

impl ExecutorRegistry for FakeExecutorRegistry {
    fn supports(&self, span: &replaykit_core_model::SpanRecord) -> bool {
        span.kind == SpanKind::LlmCall
    }

    fn execute(
        &self,
        span: &replaykit_core_model::SpanRecord,
        _context: &ReplayExecutionContext,
    ) -> Result<replaykit_replay_engine::ExecutionResult, ReplayError> {
        Ok(replaykit_replay_engine::ExecutionResult {
            status: SpanStatus::Completed,
            output_artifacts: vec![replaykit_replay_engine::ProducedArtifact {
                artifact_type: ArtifactType::ModelResponse,
                mime: "application/json".into(),
                sha256: "replayed".into(),
                byte_len: 1,
                blob_path: "memory://replayed".into(),
                summary: Document::new(),
                redaction: Document::new(),
                created_at: 10,
            }],
            output_fingerprint: Some(format!("replayed:{}", span.span_id.0)),
            snapshot: None,
            error_summary: None,
            cost: CostMetrics::default(),
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn seed_run<S: Storage, E: ExecutorRegistry>(
    service: &ReplayKitService<S, E>,
) -> replaykit_core_model::RunRecord {
    let run = service
        .begin_run(BeginRun {
            title: "test run".into(),
            entrypoint: "agent.main".into(),
            adapter_name: "test".into(),
            adapter_version: "0.1.0".into(),
            started_at: 1,
            git_sha: None,
            environment_fingerprint: None,
            host: HostMetadata {
                os: "macos".into(),
                arch: "arm64".into(),
                hostname: None,
            },
            labels: Vec::new(),
        })
        .unwrap();

    let planner = service
        .start_span(
            &run.run_id,
            &run.trace_id,
            SpanSpec {
                span_id: Some(SpanId("planner".into())),
                parent_span_id: None,
                kind: SpanKind::PlannerStep,
                name: "planner".into(),
                started_at: 1,
                replay_policy: ReplayPolicy::RecordOnly,
                executor_kind: None,
                executor_version: None,
                input_artifact_ids: Vec::new(),
                input_fingerprint: None,
                environment_fingerprint: None,
                attributes: Document::new(),
            },
        )
        .unwrap();
    service
        .end_span(
            &run.run_id,
            &planner.span_id,
            EndSpan {
                ended_at: 2,
                status: SpanStatus::Completed,
                output_artifact_ids: Vec::new(),
                snapshot_id: None,
                output_fingerprint: Some("planner".into()),
                error_code: None,
                error_summary: None,
                cost: CostMetrics::default(),
            },
        )
        .unwrap();

    let tool = service
        .start_span(
            &run.run_id,
            &run.trace_id,
            SpanSpec {
                span_id: Some(SpanId("tool".into())),
                parent_span_id: Some(planner.span_id.clone()),
                kind: SpanKind::ToolCall,
                name: "tool".into(),
                started_at: 3,
                replay_policy: ReplayPolicy::RerunnableSupported,
                executor_kind: None,
                executor_version: None,
                input_artifact_ids: Vec::new(),
                input_fingerprint: None,
                environment_fingerprint: None,
                attributes: Document::new(),
            },
        )
        .unwrap();
    let tool_artifact = service
        .add_artifact(
            &run.run_id,
            Some(&tool.span_id),
            ArtifactSpec {
                artifact_type: ArtifactType::ToolOutput,
                mime: "application/json".into(),
                sha256: "tool-hash".into(),
                byte_len: 1,
                blob_path: "memory://tool-output".into(),
                summary: Document::new(),
                redaction: Document::new(),
                created_at: 4,
                content: None,
            },
        )
        .unwrap();
    service
        .end_span(
            &run.run_id,
            &tool.span_id,
            EndSpan {
                ended_at: 4,
                status: SpanStatus::Completed,
                output_artifact_ids: vec![tool_artifact.artifact_id],
                snapshot_id: None,
                output_fingerprint: Some("tool-out".into()),
                error_code: None,
                error_summary: None,
                cost: CostMetrics::default(),
            },
        )
        .unwrap();

    let answer = service
        .start_span(
            &run.run_id,
            &run.trace_id,
            SpanSpec {
                span_id: Some(SpanId("answer".into())),
                parent_span_id: Some(planner.span_id.clone()),
                kind: SpanKind::LlmCall,
                name: "answer".into(),
                started_at: 5,
                replay_policy: ReplayPolicy::RerunnableSupported,
                executor_kind: Some("fake-llm".into()),
                executor_version: Some("v1".into()),
                input_artifact_ids: Vec::new(),
                input_fingerprint: Some("answer-in".into()),
                environment_fingerprint: None,
                attributes: Document::new(),
            },
        )
        .unwrap();
    let answer_artifact = service
        .add_artifact(
            &run.run_id,
            Some(&answer.span_id),
            ArtifactSpec {
                artifact_type: ArtifactType::ModelResponse,
                mime: "application/json".into(),
                sha256: "answer-hash".into(),
                byte_len: 1,
                blob_path: "memory://answer-output".into(),
                summary: Document::new(),
                redaction: Document::new(),
                created_at: 6,
                content: None,
            },
        )
        .unwrap();
    service
        .end_span(
            &run.run_id,
            &answer.span_id,
            EndSpan {
                ended_at: 6,
                status: SpanStatus::Failed,
                output_artifact_ids: vec![answer_artifact.artifact_id],
                snapshot_id: None,
                output_fingerprint: Some("answer-out".into()),
                error_code: None,
                error_summary: Some("model failed".into()),
                cost: CostMetrics::default(),
            },
        )
        .unwrap();

    service
        .add_edge(
            &run.run_id,
            replaykit_collector::EdgeSpec {
                from_span_id: tool.span_id,
                to_span_id: answer.span_id,
                kind: replaykit_core_model::EdgeKind::DataDependsOn,
                attributes: Document::new(),
            },
        )
        .unwrap();

    service
        .finish_run(&run.run_id, 7, RunStatus::Failed, Some("failed".into()))
        .unwrap()
}

fn seeded_server() -> (TestServer, RunId) {
    let (server, run_id, _) = seeded_server_with_storage();
    (server, run_id)
}

fn seeded_server_with_storage() -> (TestServer, RunId, Arc<InMemoryStorage>) {
    let storage = Arc::new(InMemoryStorage::new());
    let service = Arc::new(ReplayKitService::new(storage.clone(), FakeExecutorRegistry));
    let run = seed_run(&service);
    let run_id = run.run_id.clone();
    let router = build_router(service);
    let server = TestServer::new(router).expect("test server");
    (server, run_id, storage)
}

// ---------------------------------------------------------------------------
// Route tests — Section 10.1
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_runs_returns_200_with_array() {
    let (server, run_id) = seeded_server();
    let resp = server.get("/api/v1/runs").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert!(body.is_array());
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["run_id"], run_id.0);
    assert_eq!(body[0]["status_label"], "failed");
}

#[tokio::test]
async fn get_run_returns_200() {
    let (server, run_id) = seeded_server();
    let resp = server.get(&format!("/api/v1/runs/{}", run_id.0)).await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["run_id"], run_id.0);
    assert_eq!(body["title"], "test run");
}

#[tokio::test]
async fn get_run_returns_404_for_missing_run() {
    let (server, _) = seeded_server();
    let resp = server.get("/api/v1/runs/nonexistent").await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["code"], "not_found");
}

#[tokio::test]
async fn run_tree_returns_200_with_nodes() {
    let (server, run_id) = seeded_server();
    let resp = server.get(&format!("/api/v1/runs/{}/tree", run_id.0)).await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["run_id"], run_id.0);
    assert!(body["nodes"].is_array());
    let nodes = body["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1); // root: planner
    assert_eq!(nodes[0]["name"], "planner");
    assert_eq!(nodes[0]["children"].as_array().unwrap().len(), 2); // tool + answer
}

#[tokio::test]
async fn run_edges_returns_200() {
    let (server, run_id) = seeded_server();
    let resp = server.get(&format!("/api/v1/runs/{}/edges", run_id.0)).await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert!(body.is_array());
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["kind"], "DataDependsOn");
}

#[tokio::test]
async fn span_detail_returns_200() {
    let (server, run_id) = seeded_server();
    let resp = server
        .get(&format!("/api/v1/runs/{}/spans/tool", run_id.0))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["span_id"], "tool");
    assert_eq!(body["kind"], "ToolCall");
    assert_eq!(body["status_label"], "completed");
    assert_eq!(body["replay_policy"], "rerunnable_supported");
}

#[tokio::test]
async fn span_artifacts_returns_200() {
    let (server, run_id) = seeded_server();
    let resp = server
        .get(&format!("/api/v1/runs/{}/spans/tool/artifacts", run_id.0))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert!(body.is_array());
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["artifact_type"], "ToolOutput");
}

#[tokio::test]
async fn span_dependencies_returns_200() {
    let (server, run_id) = seeded_server();
    let resp = server
        .get(&format!(
            "/api/v1/runs/{}/spans/tool/dependencies",
            run_id.0
        ))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert!(body.is_array());
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["kind"], "DataDependsOn");
}

#[tokio::test]
async fn create_branch_returns_201() {
    let (server, run_id) = seeded_server();
    let resp = server
        .post("/api/v1/branches")
        .json(&serde_json::json!({
            "source_run_id": run_id.0,
            "fork_span_id": "tool",
            "patch_type": "tool_output_override",
            "replacement": "patched output"
        }))
        .await;
    resp.assert_status(axum::http::StatusCode::CREATED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["source_run_id"], run_id.0);
    assert!(body["target_run_id"].is_string());
    assert!(body["branch_id"].is_string());
}

#[tokio::test]
async fn cached_diff_returns_200_after_branch() {
    let (server, run_id) = seeded_server();
    // Create branch first (which auto-diffs)
    let branch_resp = server
        .post("/api/v1/branches")
        .json(&serde_json::json!({
            "source_run_id": run_id.0,
            "fork_span_id": "tool",
            "patch_type": "tool_output_override",
            "replacement": "patched"
        }))
        .await;
    let branch: serde_json::Value = branch_resp.json();
    let target_run_id = branch["target_run_id"].as_str().unwrap();

    let resp = server
        .get(&format!("/api/v1/runs/{}/diff/{}", run_id.0, target_run_id))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["source_run_id"], run_id.0);
    assert_eq!(body["target_run_id"], target_run_id);
    assert!(body["changed_span_count"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn run_branches_returns_200_after_branch() {
    let (server, run_id) = seeded_server();
    let branch_resp = server
        .post("/api/v1/branches")
        .json(&serde_json::json!({
            "source_run_id": run_id.0,
            "fork_span_id": "tool",
            "patch_type": "tool_output_override",
            "replacement": "patched"
        }))
        .await;
    let branch: serde_json::Value = branch_resp.json();
    let target_run_id = branch["target_run_id"].as_str().unwrap();

    let resp = server
        .get(&format!("/api/v1/runs/{}/branches", run_id.0))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert!(body.is_array());
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["source_run_id"], run_id.0);
    assert_eq!(body[0]["target_run_id"], target_run_id);
    assert_eq!(body[0]["fork_span_id"], "tool");
}

#[tokio::test]
async fn artifact_content_returns_200_for_stored_blob() {
    let (server, run_id, storage) = seeded_server_with_storage();
    let artifact = replaykit_core_model::ArtifactRecord {
        artifact_id: replaykit_core_model::ArtifactId("blob-artifact".into()),
        run_id: run_id.clone(),
        span_id: None,
        artifact_type: ArtifactType::DebugLog,
        mime: "application/json".into(),
        sha256: "placeholder".into(),
        byte_len: 0,
        blob_path: "memory://placeholder".into(),
        summary: Document::new(),
        redaction: Document::new(),
        created_at: 9,
    };
    storage
        .store_artifact_with_content(artifact, br#"{"ok":true}"#)
        .unwrap();

    let resp = server
        .get(&format!(
            "/api/v1/runs/{}/artifacts/{}/content",
            run_id.0, "blob-artifact"
        ))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["artifact_id"], "blob-artifact");
    assert_eq!(body["content"], "{\"ok\":true}");
}

#[tokio::test]
async fn cached_diff_returns_404_when_not_computed() {
    let (server, run_id) = seeded_server();
    let resp = server
        .get(&format!("/api/v1/runs/{}/diff/nonexistent", run_id.0))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["code"], "not_found");
}

#[tokio::test]
async fn timeline_returns_501_stub() {
    let (server, run_id) = seeded_server();
    let resp = server
        .get(&format!("/api/v1/runs/{}/timeline", run_id.0))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["code"], "not_implemented");
}

#[tokio::test]
async fn plan_branch_returns_200_with_plan() {
    let (server, run_id) = seeded_server();
    let resp = server
        .post("/api/v1/branches/plan")
        .json(&serde_json::json!({
            "source_run_id": run_id.0,
            "fork_span_id": "tool",
            "patch_type": "tool_output_override",
            "replacement": "patched"
        }))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["source_run_id"], run_id.0);
    assert_eq!(body["fork_span_id"], "tool");
    assert!(body["dirty_spans"].is_array());
}

#[tokio::test]
async fn compute_diff_returns_201() {
    let (server, run_id) = seeded_server();
    // Create a branch first to get a second run
    let branch_resp = server
        .post("/api/v1/branches")
        .json(&serde_json::json!({
            "source_run_id": run_id.0,
            "fork_span_id": "tool",
            "patch_type": "tool_output_override",
            "replacement": "x"
        }))
        .await;
    let branch: serde_json::Value = branch_resp.json();
    let target = branch["target_run_id"].as_str().unwrap();

    // Now compute diff via POST (even though one already exists from branch creation)
    let resp = server
        .post("/api/v1/diffs")
        .json(&serde_json::json!({
            "source_run_id": run_id.0,
            "target_run_id": target
        }))
        .await;
    resp.assert_status(axum::http::StatusCode::CREATED);
    let body: serde_json::Value = resp.json();
    assert!(body["diff_id"].is_string());
}

#[tokio::test]
async fn invalid_patch_type_returns_400() {
    let (server, run_id) = seeded_server();
    let resp = server
        .post("/api/v1/branches")
        .json(&serde_json::json!({
            "source_run_id": run_id.0,
            "fork_span_id": "tool",
            "patch_type": "totally_bogus",
            "replacement": "x"
        }))
        .await;
    resp.assert_status(axum::http::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["code"], "invalid_input");
}

// ---------------------------------------------------------------------------
// Golden JSON shape tests — Section 10.4
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_tree_golden_shape() {
    let (server, run_id) = seeded_server();
    let resp = server.get(&format!("/api/v1/runs/{}/tree", run_id.0)).await;
    let body: serde_json::Value = resp.json();

    // Top-level keys
    for key in &["run_id", "title", "status", "nodes"] {
        assert!(
            body.get(key).is_some(),
            "missing key '{key}' in RunTreeView"
        );
    }

    // Node shape
    let node = &body["nodes"][0];
    for key in &[
        "span_id",
        "name",
        "kind",
        "status",
        "status_label",
        "replay_policy",
        "started_at",
        "ended_at",
        "error_summary",
        "child_count",
        "children",
    ] {
        assert!(
            node.get(key).is_some(),
            "missing key '{key}' in TreeNodeView"
        );
    }
}

#[tokio::test]
async fn diff_golden_shape() {
    let (server, run_id) = seeded_server();
    // Create branch to get a diff
    let branch_resp = server
        .post("/api/v1/branches")
        .json(&serde_json::json!({
            "source_run_id": run_id.0,
            "fork_span_id": "tool",
            "patch_type": "tool_output_override",
            "replacement": "y"
        }))
        .await;
    let branch: serde_json::Value = branch_resp.json();
    let target = branch["target_run_id"].as_str().unwrap();

    let resp = server
        .get(&format!("/api/v1/runs/{}/diff/{}", run_id.0, target))
        .await;
    let body: serde_json::Value = resp.json();

    for key in &[
        "diff_id",
        "source_run_id",
        "target_run_id",
        "source_status",
        "target_status",
        "changed_span_count",
        "changed_artifact_count",
        "first_divergent_span_id",
        "summary",
    ] {
        assert!(
            body.get(key).is_some(),
            "missing key '{key}' in RunDiffSummaryView"
        );
    }
}
