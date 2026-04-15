use std::sync::Arc;

use axum::http::{Method, StatusCode, header};
use axum_test::TestServer;
use replaykit_api::ReplayKitService;
use replaykit_api::server::build_router;
use replaykit_collector::{ArtifactSpec, BeginRun, EndSpan, SpanSpec};
use replaykit_core_model::{
    ArtifactType, CostMetrics, Document, HostMetadata, ReplayPolicy, RunId, RunStatus, SpanId,
    SpanKind, SpanStatus,
};
use replaykit_replay_engine::{ExecutorRegistry, ReplayError, ReplayExecutionContext};
use replaykit_storage::{InMemoryStorage, SqliteStorage, Storage, StorageError};

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
                content: None,
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
                from_span_id: answer.span_id,
                to_span_id: tool.span_id,
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

fn seeded_server_with_failed_dependency() -> (TestServer, RunId) {
    let storage = Arc::new(InMemoryStorage::new());
    let service = Arc::new(ReplayKitService::new(storage, FakeExecutorRegistry));
    let run = service
        .begin_run(BeginRun {
            title: "failed dependency run".into(),
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
    service
        .end_span(
            &run.run_id,
            &tool.span_id,
            EndSpan {
                ended_at: 4,
                status: SpanStatus::Failed,
                output_artifact_ids: Vec::new(),
                snapshot_id: None,
                output_fingerprint: Some("tool-out".into()),
                error_code: None,
                error_summary: Some("tool failed".into()),
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
    service
        .end_span(
            &run.run_id,
            &answer.span_id,
            EndSpan {
                ended_at: 6,
                status: SpanStatus::Failed,
                output_artifact_ids: Vec::new(),
                snapshot_id: None,
                output_fingerprint: Some("answer-out".into()),
                error_code: None,
                error_summary: Some("answer failed".into()),
                cost: CostMetrics::default(),
            },
        )
        .unwrap();

    service
        .add_edge(
            &run.run_id,
            replaykit_collector::EdgeSpec {
                from_span_id: answer.span_id,
                to_span_id: tool.span_id,
                kind: replaykit_core_model::EdgeKind::DataDependsOn,
                attributes: Document::new(),
            },
        )
        .unwrap();

    service
        .finish_run(&run.run_id, 7, RunStatus::Failed, Some("failed".into()))
        .unwrap();

    let run_id = run.run_id.clone();
    let router = build_router(service);
    let server = TestServer::new(router).expect("test server");
    (server, run_id)
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
async fn healthz_returns_200() {
    let (server, _) = seeded_server();
    let resp = server.get("/healthz").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn options_preflight_returns_cors_headers() {
    let (server, _) = seeded_server();
    let resp = server
        .method(Method::OPTIONS, "/api/v1/runs")
        .add_header(header::ORIGIN, "http://127.0.0.1:5173")
        .add_header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
        .await;
    assert!(
        resp.status_code().is_success(),
        "preflight should succeed, got {}",
        resp.status_code()
    );
    assert_eq!(
        resp.header(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .to_str()
            .unwrap(),
        "http://127.0.0.1:5173"
    );
}

#[tokio::test]
async fn get_routes_include_cors_headers_for_allowed_origin() {
    let (server, _) = seeded_server();
    let resp = server
        .get("/api/v1/runs")
        .add_header(header::ORIGIN, "http://localhost:5173")
        .await;
    resp.assert_status(StatusCode::OK);
    assert_eq!(
        resp.header(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .to_str()
            .unwrap(),
        "http://localhost:5173"
    );
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
    let resp = server
        .get(&format!("/api/v1/runs/{}/edges", run_id.0))
        .await;
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
    assert_eq!(body["content_encoding"], "utf-8");
    assert_eq!(body["content"].as_str().unwrap(), r#"{"ok":true}"#);
}

#[tokio::test]
async fn artifact_content_binary_round_trips_via_base64() {
    let (server, run_id, storage) = seeded_server_with_storage();
    let binary_bytes: &[u8] = b"\xff\xfe\x00\x01binary content";
    let artifact = replaykit_core_model::ArtifactRecord {
        artifact_id: replaykit_core_model::ArtifactId("binary-artifact".into()),
        run_id: run_id.clone(),
        span_id: None,
        artifact_type: ArtifactType::Screenshot,
        mime: "image/png".into(),
        sha256: "placeholder".into(),
        byte_len: 0,
        blob_path: "memory://placeholder".into(),
        summary: Document::new(),
        redaction: Document::new(),
        created_at: 10,
    };
    storage
        .store_artifact_with_content(artifact, binary_bytes)
        .unwrap();

    let resp = server
        .get(&format!(
            "/api/v1/runs/{}/artifacts/{}/content",
            run_id.0, "binary-artifact"
        ))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["artifact_id"], "binary-artifact");
    assert_eq!(body["content_encoding"], "base64");

    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(body["content"].as_str().unwrap())
        .unwrap();
    assert_eq!(decoded, binary_bytes);
}

#[tokio::test]
async fn artifact_content_forces_base64_for_binary_mime_even_if_utf8_valid() {
    // Regression: UTF-8 validity alone must not decide encoding. Bytes that
    // happen to be valid UTF-8 (e.g. pure ASCII) under a binary mime like
    // application/octet-stream must still come back base64, so the web
    // ArtifactViewer routes them through the binary metadata + download UX
    // instead of dumping them as text.
    let (server, run_id, storage) = seeded_server_with_storage();
    let ascii_bytes: &[u8] = b"\x00\x01\x02\x03hello world"; // valid UTF-8
    assert!(std::str::from_utf8(ascii_bytes).is_ok());
    let artifact = replaykit_core_model::ArtifactRecord {
        artifact_id: replaykit_core_model::ArtifactId("octet-ascii".into()),
        run_id: run_id.clone(),
        span_id: None,
        artifact_type: ArtifactType::ToolOutput,
        mime: "application/octet-stream".into(),
        sha256: "placeholder".into(),
        byte_len: 0,
        blob_path: "memory://placeholder".into(),
        summary: Document::new(),
        redaction: Document::new(),
        created_at: 11,
    };
    storage
        .store_artifact_with_content(artifact, ascii_bytes)
        .unwrap();

    let resp = server
        .get(&format!(
            "/api/v1/runs/{}/artifacts/{}/content",
            run_id.0, "octet-ascii"
        ))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["content_encoding"], "base64");
    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(body["content"].as_str().unwrap())
        .unwrap();
    assert_eq!(decoded, ascii_bytes);
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
async fn timeline_returns_200_with_entries() {
    let (server, run_id) = seeded_server();
    let resp = server
        .get(&format!("/api/v1/runs/{}/timeline", run_id.0))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["run_id"], run_id.0);
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3); // planner, tool, answer
    // Verify sorted by started_at
    let started_ats: Vec<u64> = entries
        .iter()
        .map(|e| e["started_at"].as_u64().unwrap())
        .collect();
    assert!(started_ats.windows(2).all(|w| w[0] <= w[1]));
    // Verify depth: planner=0, tool=1, answer=1
    assert_eq!(entries[0]["depth"], 0);
    assert_eq!(entries[1]["depth"], 1);
    assert_eq!(entries[2]["depth"], 1);
}

#[tokio::test]
async fn timeline_golden_shape() {
    let (server, run_id) = seeded_server();
    let resp = server
        .get(&format!("/api/v1/runs/{}/timeline", run_id.0))
        .await;
    let body: serde_json::Value = resp.json();
    for key in &[
        "run_id",
        "title",
        "status",
        "total_started_at",
        "total_ended_at",
        "entries",
    ] {
        assert!(
            body.get(key).is_some(),
            "missing key '{key}' in TimelineView"
        );
    }
    let entry = &body["entries"][0];
    for key in &[
        "span_id",
        "name",
        "kind",
        "status",
        "status_label",
        "started_at",
        "ended_at",
        "depth",
        "parent_span_id",
        "error_summary",
    ] {
        assert!(
            entry.get(key).is_some(),
            "missing key '{key}' in TimelineEntryView"
        );
    }
}

#[tokio::test]
async fn timeline_returns_404_for_missing_run() {
    let (server, _) = seeded_server();
    let resp = server.get("/api/v1/runs/nonexistent/timeline").await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn forensics_returns_200_with_failure_analysis() {
    let (server, run_id) = seeded_server();
    let resp = server
        .get(&format!("/api/v1/runs/{}/forensics", run_id.0))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["run_id"], run_id.0);
    assert_eq!(body["has_failure"], true);
    assert!(body["first_failed_span_id"].is_string());
    assert!(body["deepest_failed_span_id"].is_string());
    assert!(body["failure_path"].is_array());
    assert!(body["blocked_spans"].is_array());
    assert!(body["deepest_failing_dependency_id"].is_null());
}

#[tokio::test]
async fn forensics_reports_failed_dependency_chain() {
    let (server, run_id) = seeded_server_with_failed_dependency();
    let resp = server
        .get(&format!("/api/v1/runs/{}/forensics", run_id.0))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["deepest_failed_span_id"], "answer");
    assert_eq!(body["deepest_failing_dependency_id"], "tool");
    assert_eq!(
        body["failure_path"],
        serde_json::json!(["planner", "answer"])
    );
}

#[tokio::test]
async fn forensics_golden_shape() {
    let (server, run_id) = seeded_server();
    let resp = server
        .get(&format!("/api/v1/runs/{}/forensics", run_id.0))
        .await;
    let body: serde_json::Value = resp.json();
    for key in &[
        "run_id",
        "has_failure",
        "first_failed_span_id",
        "deepest_failed_span_id",
        "deepest_failing_dependency_id",
        "failure_path",
        "blocked_spans",
        "retry_groups",
    ] {
        assert!(
            body.get(key).is_some(),
            "missing key '{key}' in FailureForensicsView"
        );
    }
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
        "span_diffs",
        "latency_ms_delta",
        "token_delta",
        "final_output_changed",
        "summary",
    ] {
        assert!(
            body.get(key).is_some(),
            "missing key '{key}' in RunDiffSummaryView"
        );
    }

    // Verify span_diffs is populated
    let span_diffs = body["span_diffs"].as_array().unwrap();
    assert!(!span_diffs.is_empty(), "span_diffs should not be empty");
    let sd = &span_diffs[0];
    for key in &["span_id_source", "span_id_target", "name", "output_changed"] {
        assert!(sd.get(key).is_some(), "missing key '{key}' in SpanDiffView");
    }
}

// ---------------------------------------------------------------------------
// SQLite-backed integration test: diff insert failure triggers branch cleanup.
// ---------------------------------------------------------------------------

struct SqliteDiffFail {
    inner: Arc<SqliteStorage>,
}

impl SqliteDiffFail {
    fn new(inner: Arc<SqliteStorage>) -> Self {
        Self { inner }
    }
}

impl Storage for SqliteDiffFail {
    fn allocate_id(&self, kind: replaykit_core_model::IdKind) -> Result<String, StorageError> {
        self.inner.allocate_id(kind)
    }
    fn next_sequence(&self, run_id: &RunId) -> Result<u64, StorageError> {
        self.inner.next_sequence(run_id)
    }
    fn insert_run(&self, run: replaykit_core_model::RunRecord) -> Result<(), StorageError> {
        self.inner.insert_run(run)
    }
    fn update_run(&self, run: replaykit_core_model::RunRecord) -> Result<(), StorageError> {
        self.inner.update_run(run)
    }
    fn get_run(&self, run_id: &RunId) -> Result<replaykit_core_model::RunRecord, StorageError> {
        self.inner.get_run(run_id)
    }
    fn list_runs(&self) -> Result<Vec<replaykit_core_model::RunRecord>, StorageError> {
        self.inner.list_runs()
    }
    fn upsert_span(&self, span: replaykit_core_model::SpanRecord) -> Result<(), StorageError> {
        self.inner.upsert_span(span)
    }
    fn get_span(
        &self,
        run_id: &RunId,
        span_id: &SpanId,
    ) -> Result<replaykit_core_model::SpanRecord, StorageError> {
        self.inner.get_span(run_id, span_id)
    }
    fn list_spans(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<replaykit_core_model::SpanRecord>, StorageError> {
        self.inner.list_spans(run_id)
    }
    fn insert_event(&self, event: replaykit_core_model::EventRecord) -> Result<(), StorageError> {
        self.inner.insert_event(event)
    }
    fn list_events(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<replaykit_core_model::EventRecord>, StorageError> {
        self.inner.list_events(run_id)
    }
    fn insert_artifact(
        &self,
        artifact: replaykit_core_model::ArtifactRecord,
    ) -> Result<(), StorageError> {
        self.inner.insert_artifact(artifact)
    }
    fn get_artifact(
        &self,
        run_id: &RunId,
        artifact_id: &replaykit_core_model::ArtifactId,
    ) -> Result<replaykit_core_model::ArtifactRecord, StorageError> {
        self.inner.get_artifact(run_id, artifact_id)
    }
    fn list_artifacts(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<replaykit_core_model::ArtifactRecord>, StorageError> {
        self.inner.list_artifacts(run_id)
    }
    fn insert_snapshot(
        &self,
        snapshot: replaykit_core_model::SnapshotRecord,
    ) -> Result<(), StorageError> {
        self.inner.insert_snapshot(snapshot)
    }
    fn get_snapshot(
        &self,
        run_id: &RunId,
        snapshot_id: &replaykit_core_model::SnapshotId,
    ) -> Result<replaykit_core_model::SnapshotRecord, StorageError> {
        self.inner.get_snapshot(run_id, snapshot_id)
    }
    fn list_snapshots(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<replaykit_core_model::SnapshotRecord>, StorageError> {
        self.inner.list_snapshots(run_id)
    }
    fn insert_edge(&self, edge: replaykit_core_model::SpanEdgeRecord) -> Result<(), StorageError> {
        self.inner.insert_edge(edge)
    }
    fn list_edges(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<replaykit_core_model::SpanEdgeRecord>, StorageError> {
        self.inner.list_edges(run_id)
    }
    fn insert_branch(
        &self,
        branch: replaykit_core_model::BranchRecord,
    ) -> Result<(), StorageError> {
        self.inner.insert_branch(branch)
    }
    fn get_branch(
        &self,
        branch_id: &replaykit_core_model::BranchId,
    ) -> Result<replaykit_core_model::BranchRecord, StorageError> {
        self.inner.get_branch(branch_id)
    }
    fn list_branches(&self) -> Result<Vec<replaykit_core_model::BranchRecord>, StorageError> {
        self.inner.list_branches()
    }
    fn insert_replay_job(
        &self,
        job: replaykit_core_model::ReplayJobRecord,
    ) -> Result<(), StorageError> {
        self.inner.insert_replay_job(job)
    }
    fn update_replay_job(
        &self,
        job: replaykit_core_model::ReplayJobRecord,
    ) -> Result<(), StorageError> {
        self.inner.update_replay_job(job)
    }
    fn get_replay_job(
        &self,
        replay_job_id: &replaykit_core_model::ReplayJobId,
    ) -> Result<replaykit_core_model::ReplayJobRecord, StorageError> {
        self.inner.get_replay_job(replay_job_id)
    }
    fn insert_diff(&self, _diff: replaykit_core_model::RunDiffRecord) -> Result<(), StorageError> {
        Err(StorageError::Internal(
            "synthetic sqlite diff storage failure".into(),
        ))
    }
    fn get_diff(
        &self,
        source: &RunId,
        target: &RunId,
    ) -> Result<replaykit_core_model::RunDiffRecord, StorageError> {
        self.inner.get_diff(source, target)
    }
    fn store_artifact_with_content(
        &self,
        artifact: replaykit_core_model::ArtifactRecord,
        content: &[u8],
    ) -> Result<replaykit_core_model::ArtifactRecord, StorageError> {
        self.inner.store_artifact_with_content(artifact, content)
    }
    fn read_artifact_content(
        &self,
        run_id: &RunId,
        artifact_id: &replaykit_core_model::ArtifactId,
    ) -> Result<Vec<u8>, StorageError> {
        self.inner.read_artifact_content(run_id, artifact_id)
    }
    fn verify_artifact_integrity(
        &self,
        run_id: &RunId,
        artifact_id: &replaykit_core_model::ArtifactId,
    ) -> Result<replaykit_storage::BlobIntegrity, StorageError> {
        self.inner.verify_artifact_integrity(run_id, artifact_id)
    }
    fn cleanup_branch_execution(
        &self,
        target_run_id: &RunId,
        branch_id: &replaykit_core_model::BranchId,
        replay_job_id: &replaykit_core_model::ReplayJobId,
    ) -> Result<(), StorageError> {
        self.inner
            .cleanup_branch_execution(target_run_id, branch_id, replay_job_id)
    }
}

#[tokio::test]
async fn sqlite_branch_creation_cleans_up_after_diff_failure() {
    let data_root = std::env::temp_dir().join(format!(
        "replaykit-route-branch-cleanup-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let sqlite = Arc::new(SqliteStorage::open_with_data_root(&data_root).unwrap());
    let wrapped = Arc::new(SqliteDiffFail::new(sqlite.clone()));
    let service = Arc::new(ReplayKitService::new(wrapped, FakeExecutorRegistry));
    let run = seed_run(&service);
    let source_run_id = run.run_id.clone();

    let runs_before = sqlite.list_runs().unwrap().len();
    let branches_before = sqlite.list_branches().unwrap().len();

    let err = service
        .create_branch(replaykit_core_model::BranchRequest {
            source_run_id: source_run_id.clone(),
            fork_span_id: SpanId("tool".into()),
            patch_manifest: replaykit_core_model::PatchManifest {
                patch_type: replaykit_core_model::PatchType::ToolOutputOverride,
                target_artifact_id: None,
                replacement: replaykit_core_model::Value::Text("patched tool result".into()),
                note: None,
                created_at: 20,
            },
            created_by: Some("test".into()),
        })
        .unwrap_err();

    let message = format!("{err}");
    assert!(
        message.contains("synthetic sqlite diff storage failure"),
        "unexpected error: {message}"
    );

    // Cleanup ran against persistent SQLite storage.
    assert_eq!(sqlite.list_runs().unwrap().len(), runs_before);
    assert_eq!(sqlite.list_branches().unwrap().len(), branches_before);
    let source_artifacts = sqlite.list_artifacts(&source_run_id).unwrap();
    assert!(!source_artifacts.is_empty(), "source run must survive");

    let _ = std::fs::remove_dir_all(&data_root);
}
