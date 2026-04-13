use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use replaykit_core_model::{
    ArtifactType, CostMetrics, Document, EdgeKind, HostMetadata, ReplayPolicy, RunId, RunStatus,
    SpanId, SpanKind, SpanStatus,
};
use replaykit_storage::{SqliteStorage, Storage};
use serde::{Deserialize, Serialize};

use crate::{
    ArtifactSpec, BeginRun, Collector, CollectorError, EdgeSpec, EndSpan, EventSpec, SnapshotSpec,
    SpanSpec,
};

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

pub struct CollectorServer {
    collector: Collector<SqliteStorage>,
}

type AppState = Arc<CollectorServer>;

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

impl IntoResponse for CollectorError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            CollectorError::InvalidInput(msg) => {
                if msg.contains("not found") {
                    (StatusCode::NOT_FOUND, msg.clone())
                } else {
                    (StatusCode::BAD_REQUEST, msg.clone())
                }
            }
            CollectorError::Storage(err) => {
                use replaykit_storage::StorageError;
                match err {
                    StorageError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
                    StorageError::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
                    StorageError::InvalidInput(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
                    StorageError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
                }
            }
        };
        (status, Json(ErrorResponse { error: message })).into_response()
    }
}

type ApiResult<T> = Result<Json<T>, CollectorError>;

// ---------------------------------------------------------------------------
// Request/Response DTOs
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct BeginRunRequest {
    pub title: String,
    pub entrypoint: String,
    pub adapter_name: String,
    pub adapter_version: String,
    pub started_at: u64,
    #[serde(default)]
    pub git_sha: Option<String>,
    #[serde(default)]
    pub environment_fingerprint: Option<String>,
    #[serde(default)]
    pub host: Option<HostMetadataDto>,
    #[serde(default)]
    pub labels: Vec<String>,
}

#[derive(Deserialize, Default)]
pub struct HostMetadataDto {
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub hostname: Option<String>,
}

#[derive(Deserialize)]
pub struct StartSpanRequest {
    #[serde(default)]
    pub span_id: Option<String>,
    #[serde(default)]
    pub parent_span_id: Option<String>,
    pub kind: SpanKind,
    pub name: String,
    pub started_at: u64,
    #[serde(default = "default_replay_policy")]
    pub replay_policy: ReplayPolicy,
    #[serde(default)]
    pub executor_kind: Option<String>,
    #[serde(default)]
    pub executor_version: Option<String>,
    #[serde(default)]
    pub input_artifact_ids: Vec<String>,
    #[serde(default)]
    pub input_fingerprint: Option<String>,
    #[serde(default)]
    pub environment_fingerprint: Option<String>,
    #[serde(default)]
    pub attributes: Document,
}

fn default_replay_policy() -> ReplayPolicy {
    ReplayPolicy::RecordOnly
}

#[derive(Deserialize)]
pub struct EndSpanRequest {
    pub ended_at: u64,
    pub status: SpanStatus,
    #[serde(default)]
    pub output_artifact_ids: Vec<String>,
    #[serde(default)]
    pub snapshot_id: Option<String>,
    #[serde(default)]
    pub output_fingerprint: Option<String>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub error_summary: Option<String>,
    #[serde(default)]
    pub cost: Option<CostMetricsDto>,
}

#[derive(Deserialize, Default)]
pub struct CostMetricsDto {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub estimated_cost_micros: u64,
}

#[derive(Deserialize)]
pub struct AddEventRequest {
    pub timestamp: u64,
    pub kind: String,
    #[serde(default)]
    pub payload: Document,
}

#[derive(Deserialize)]
pub struct AddArtifactRequest {
    pub artifact_type: ArtifactType,
    pub mime: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub byte_len: usize,
    #[serde(default)]
    pub blob_path: String,
    #[serde(default)]
    pub summary: Document,
    #[serde(default)]
    pub redaction: Document,
    pub created_at: u64,
    /// Optional base64 or inline content. When provided via the upload
    /// endpoint, this is populated from the raw body instead.
    #[serde(default)]
    pub content: Option<String>,
}

#[derive(Deserialize)]
pub struct AddSnapshotRequest {
    pub kind: String,
    pub artifact_id: String,
    #[serde(default)]
    pub summary: Document,
    pub created_at: u64,
}

#[derive(Deserialize)]
pub struct AddEdgeRequest {
    pub from_span_id: String,
    pub to_span_id: String,
    pub kind: EdgeKind,
    #[serde(default)]
    pub attributes: Document,
}

#[derive(Deserialize)]
pub struct FinishRunRequest {
    pub ended_at: u64,
    #[serde(default = "default_completed_status")]
    pub status: RunStatus,
    #[serde(default)]
    pub final_output_preview: Option<String>,
}

fn default_completed_status() -> RunStatus {
    RunStatus::Completed
}

#[derive(Deserialize)]
pub struct AbortRunRequest {
    pub ended_at: u64,
    #[serde(default)]
    pub error: String,
}

#[derive(Serialize, Deserialize)]
pub struct ArtifactUploadResponse {
    pub artifact_id: String,
    pub sha256: String,
    pub byte_len: usize,
    pub blob_path: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn begin_run(
    State(state): State<AppState>,
    Json(req): Json<BeginRunRequest>,
) -> ApiResult<replaykit_core_model::RunRecord> {
    let host = req.host.unwrap_or_default();
    let run = state.collector.begin_run(BeginRun {
        title: req.title,
        entrypoint: req.entrypoint,
        adapter_name: req.adapter_name,
        adapter_version: req.adapter_version,
        started_at: req.started_at,
        git_sha: req.git_sha,
        environment_fingerprint: req.environment_fingerprint,
        host: HostMetadata {
            os: host.os,
            arch: host.arch,
            hostname: host.hostname,
        },
        labels: req.labels,
    })?;
    tracing::info!(run_id = %run.run_id.0, "run started");
    Ok(Json(run))
}

async fn start_span(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(req): Json<StartSpanRequest>,
) -> ApiResult<replaykit_core_model::SpanRecord> {
    let run_id = RunId(run_id);
    let run = state.collector.storage().get_run(&run_id)?;
    let span = state.collector.start_span(
        &run_id,
        &run.trace_id,
        SpanSpec {
            span_id: req.span_id.map(SpanId),
            parent_span_id: req.parent_span_id.map(SpanId),
            kind: req.kind,
            name: req.name,
            started_at: req.started_at,
            replay_policy: req.replay_policy,
            executor_kind: req.executor_kind,
            executor_version: req.executor_version,
            input_artifact_ids: req
                .input_artifact_ids
                .into_iter()
                .map(replaykit_core_model::ArtifactId)
                .collect(),
            input_fingerprint: req.input_fingerprint,
            environment_fingerprint: req.environment_fingerprint,
            attributes: req.attributes,
        },
    )?;
    tracing::debug!(run_id = %run_id.0, span_id = %span.span_id.0, "span started");
    Ok(Json(span))
}

async fn end_span(
    State(state): State<AppState>,
    Path((run_id, span_id)): Path<(String, String)>,
    Json(req): Json<EndSpanRequest>,
) -> ApiResult<replaykit_core_model::SpanRecord> {
    let run_id = RunId(run_id);
    let span_id = SpanId(span_id);
    let cost = req.cost.unwrap_or_default();
    let span = state.collector.end_span(
        &run_id,
        &span_id,
        EndSpan {
            ended_at: req.ended_at,
            status: req.status,
            output_artifact_ids: req
                .output_artifact_ids
                .into_iter()
                .map(replaykit_core_model::ArtifactId)
                .collect(),
            snapshot_id: req.snapshot_id.map(replaykit_core_model::SnapshotId),
            output_fingerprint: req.output_fingerprint,
            error_code: req.error_code,
            error_summary: req.error_summary,
            cost: CostMetrics {
                input_tokens: cost.input_tokens,
                output_tokens: cost.output_tokens,
                estimated_cost_micros: cost.estimated_cost_micros,
            },
        },
    )?;
    tracing::debug!(run_id = %run_id.0, span_id = %span.span_id.0, "span ended");
    Ok(Json(span))
}

async fn add_event(
    State(state): State<AppState>,
    Path((run_id, span_id)): Path<(String, String)>,
    Json(req): Json<AddEventRequest>,
) -> ApiResult<replaykit_core_model::EventRecord> {
    let run_id = RunId(run_id);
    let span_id = SpanId(span_id);
    let event = state.collector.add_event(
        &run_id,
        &span_id,
        EventSpec {
            timestamp: req.timestamp,
            kind: req.kind,
            payload: req.payload,
        },
    )?;
    Ok(Json(event))
}

async fn add_artifact(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(req): Json<AddArtifactRequest>,
) -> ApiResult<replaykit_core_model::ArtifactRecord> {
    let run_id = RunId(run_id);
    let artifact = state.collector.add_artifact(
        &run_id,
        None,
        ArtifactSpec {
            artifact_type: req.artifact_type,
            mime: req.mime,
            sha256: req.sha256,
            byte_len: req.byte_len,
            blob_path: req.blob_path,
            summary: req.summary,
            redaction: req.redaction,
            created_at: req.created_at,
            content: req.content.map(|s| s.into_bytes()),
        },
    )?;
    tracing::debug!(
        run_id = %run_id.0,
        artifact_id = %artifact.artifact_id.0,
        "artifact added"
    );
    Ok(Json(artifact))
}

async fn upload_artifact(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    body: Bytes,
) -> ApiResult<ArtifactUploadResponse> {
    let run_id = RunId(run_id);
    let artifact = state.collector.add_artifact(
        &run_id,
        None,
        ArtifactSpec {
            artifact_type: ArtifactType::FileBlob,
            mime: "application/octet-stream".into(),
            sha256: String::new(),
            byte_len: 0,
            blob_path: String::new(),
            summary: Document::new(),
            redaction: Document::new(),
            created_at: now_millis(),
            content: Some(body.to_vec()),
        },
    )?;
    tracing::debug!(
        run_id = %run_id.0,
        artifact_id = %artifact.artifact_id.0,
        byte_len = artifact.byte_len,
        "artifact uploaded"
    );
    Ok(Json(ArtifactUploadResponse {
        artifact_id: artifact.artifact_id.0,
        sha256: artifact.sha256,
        byte_len: artifact.byte_len,
        blob_path: artifact.blob_path,
    }))
}

async fn add_snapshot(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(req): Json<AddSnapshotRequest>,
) -> ApiResult<replaykit_core_model::SnapshotRecord> {
    let run_id = RunId(run_id);
    let snapshot = state.collector.add_snapshot(
        &run_id,
        None,
        SnapshotSpec {
            kind: req.kind,
            artifact_id: replaykit_core_model::ArtifactId(req.artifact_id),
            summary: req.summary,
            created_at: req.created_at,
        },
    )?;
    Ok(Json(snapshot))
}

async fn add_edge(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(req): Json<AddEdgeRequest>,
) -> ApiResult<replaykit_core_model::SpanEdgeRecord> {
    let run_id = RunId(run_id);
    let edge = state.collector.add_edge(
        &run_id,
        EdgeSpec {
            from_span_id: SpanId(req.from_span_id),
            to_span_id: SpanId(req.to_span_id),
            kind: req.kind,
            attributes: req.attributes,
        },
    )?;
    Ok(Json(edge))
}

async fn finish_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(req): Json<FinishRunRequest>,
) -> ApiResult<replaykit_core_model::RunRecord> {
    let run_id = RunId(run_id);
    let run =
        state
            .collector
            .finish_run(&run_id, req.ended_at, req.status, req.final_output_preview)?;
    tracing::info!(run_id = %run_id.0, status = ?run.status, "run finished");
    Ok(Json(run))
}

async fn abort_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(req): Json<AbortRunRequest>,
) -> ApiResult<replaykit_core_model::RunRecord> {
    let run_id = RunId(run_id);
    let run = state
        .collector
        .abort_run(&run_id, req.ended_at, req.error)?;
    tracing::info!(run_id = %run_id.0, "run aborted");
    Ok(Json(run))
}

// ---------------------------------------------------------------------------
// Router & server startup
// ---------------------------------------------------------------------------

pub fn build_router(collector: Collector<SqliteStorage>) -> Router {
    let state: AppState = Arc::new(CollectorServer { collector });

    Router::new()
        .route("/v1/runs", post(begin_run))
        .route("/v1/runs/{run_id}/spans", post(start_span))
        .route("/v1/runs/{run_id}/spans/{span_id}/end", post(end_span))
        .route("/v1/runs/{run_id}/spans/{span_id}/events", post(add_event))
        .route("/v1/runs/{run_id}/artifacts", post(add_artifact))
        .route("/v1/runs/{run_id}/artifacts/upload", post(upload_artifact))
        .route("/v1/runs/{run_id}/snapshots", post(add_snapshot))
        .route("/v1/runs/{run_id}/edges", post(add_edge))
        .route("/v1/runs/{run_id}/finish", post(finish_run))
        .route("/v1/runs/{run_id}/abort", post(abort_run))
        .with_state(state)
}

/// Start the collector HTTP server.
pub async fn serve(
    addr: SocketAddr,
    data_root: PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let storage = SqliteStorage::open_with_data_root(&data_root)?;

    // Recovery: mark interrupted runs.
    let recovered = storage.recover_interrupted_runs()?;
    if !recovered.is_empty() {
        tracing::warn!(
            count = recovered.len(),
            "marked running runs as interrupted during startup recovery"
        );
    }

    // Optional integrity scan (controlled by env var).
    if std::env::var("REPLAYKIT_INTEGRITY_SCAN").unwrap_or_default() == "1" {
        tracing::info!("running artifact integrity scan...");
        let reports = storage.scan_artifact_integrity()?;
        if reports.is_empty() {
            tracing::info!("integrity scan: all artifacts valid");
        } else {
            tracing::warn!(failures = reports.len(), "integrity scan found issues");
        }
    }

    let collector = Collector::new(Arc::new(storage));
    let app = build_router(collector);

    tracing::info!(%addr, data_root = %data_root.display(), "collector server starting");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{self, Request};
    use tower::ServiceExt;

    fn setup_test_app() -> (Router, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let storage = SqliteStorage::open_with_data_root(tmp.path()).unwrap();
        let collector = Collector::new(Arc::new(storage));
        let app = build_router(collector);
        (app, tmp)
    }

    async fn post_json(
        app: &Router,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let request = Request::builder()
            .method(http::Method::POST)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();

        let status = response.status();
        let body_bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        (status, json)
    }

    #[tokio::test]
    async fn full_flow_begin_span_artifact_end_finish() {
        let (app, _tmp) = setup_test_app();

        // 1. Begin run.
        let (status, run) = post_json(
            &app,
            "/v1/runs",
            serde_json::json!({
                "title": "test run",
                "entrypoint": "test.main",
                "adapter_name": "test",
                "adapter_version": "0.1.0",
                "started_at": 1
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let run_id = run["run_id"].as_str().unwrap();

        // 2. Start span.
        let (status, span) = post_json(
            &app,
            &format!("/v1/runs/{run_id}/spans"),
            serde_json::json!({
                "kind": "ToolCall",
                "name": "my-tool",
                "started_at": 2
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let span_id = span["span_id"].as_str().unwrap();

        // 3. Add artifact with content.
        let (status, artifact) = post_json(
            &app,
            &format!("/v1/runs/{run_id}/artifacts"),
            serde_json::json!({
                "artifact_type": "ToolOutput",
                "mime": "text/plain",
                "created_at": 3,
                "content": "hello world"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!artifact["sha256"].as_str().unwrap().is_empty());
        assert_eq!(artifact["byte_len"].as_u64().unwrap(), 11);

        // 4. End span.
        let (status, _) = post_json(
            &app,
            &format!("/v1/runs/{run_id}/spans/{span_id}/end"),
            serde_json::json!({
                "ended_at": 4,
                "status": "Completed"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // 5. Finish run.
        let (status, finished) = post_json(
            &app,
            &format!("/v1/runs/{run_id}/finish"),
            serde_json::json!({
                "ended_at": 5,
                "status": "Completed"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(finished["status"].as_str().unwrap(), "Completed");
    }

    #[tokio::test]
    async fn invalid_run_id_returns_not_found() {
        let (app, _tmp) = setup_test_app();

        let (status, body) = post_json(
            &app,
            "/v1/runs/nonexistent/spans",
            serde_json::json!({
                "kind": "ToolCall",
                "name": "x",
                "started_at": 1
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body["error"].as_str().unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn abort_run_returns_interrupted() {
        let (app, _tmp) = setup_test_app();

        let (_, run) = post_json(
            &app,
            "/v1/runs",
            serde_json::json!({
                "title": "abort test",
                "entrypoint": "test.main",
                "adapter_name": "test",
                "adapter_version": "0.1.0",
                "started_at": 1
            }),
        )
        .await;
        let run_id = run["run_id"].as_str().unwrap();

        let (status, aborted) = post_json(
            &app,
            &format!("/v1/runs/{run_id}/abort"),
            serde_json::json!({
                "ended_at": 2,
                "error": "user canceled"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(aborted["status"].as_str().unwrap(), "Interrupted");
    }

    #[tokio::test]
    async fn upload_artifact_raw_body() {
        let (app, _tmp) = setup_test_app();

        let (_, run) = post_json(
            &app,
            "/v1/runs",
            serde_json::json!({
                "title": "upload test",
                "entrypoint": "test.main",
                "adapter_name": "test",
                "adapter_version": "0.1.0",
                "started_at": 1
            }),
        )
        .await;
        let run_id = run["run_id"].as_str().unwrap();

        let request = Request::builder()
            .method(http::Method::POST)
            .uri(format!("/v1/runs/{run_id}/artifacts/upload"))
            .header("content-type", "application/octet-stream")
            .body(Body::from(b"binary content here".to_vec()))
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let result: ArtifactUploadResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(result.byte_len, 19);
        assert!(!result.sha256.is_empty());
    }
}
