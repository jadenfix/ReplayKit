use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;

use replaykit_core_model::{ArtifactId, BranchRequest, PatchManifest, RunId, SpanId, Value};
use replaykit_replay_engine::ExecutorRegistry;
use replaykit_storage::Storage;

use crate::ReplayKitService;
use crate::errors::{ApiError, ApiErrorBody};
use crate::views::{
    ArtifactPreviewView, BranchExecutionView, BranchPlanView, DependencyView, ReplayJobView,
    RunDiffSummaryView, RunSummaryView, RunTreeView, SpanDetailView,
};

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

type AppState<S, E> = Arc<ReplayKitService<S, E>>;

// ---------------------------------------------------------------------------
// Error response adapter
// ---------------------------------------------------------------------------

impl IntoResponse for ApiErrorBody {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(self)).into_response()
    }
}

fn err_response(err: ApiError) -> Response {
    ApiErrorBody::from(err).into_response()
}

// ---------------------------------------------------------------------------
// Shared request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct BranchCommandRequest {
    pub source_run_id: String,
    pub fork_span_id: String,
    pub patch_type: String,
    pub replacement: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub target_artifact_id: Option<String>,
    #[serde(default)]
    pub created_by: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ComputeDiffRequest {
    pub source_run_id: String,
    pub target_run_id: String,
}

pub fn parse_patch_type(s: &str) -> Result<replaykit_core_model::PatchType, ApiErrorBody> {
    use replaykit_core_model::PatchType;
    match s {
        "prompt_edit" => Ok(PatchType::PromptEdit),
        "tool_output_override" => Ok(PatchType::ToolOutputOverride),
        "env_var_override" => Ok(PatchType::EnvVarOverride),
        "model_config_edit" => Ok(PatchType::ModelConfigEdit),
        "retrieval_context_override" => Ok(PatchType::RetrievalContextOverride),
        "snapshot_override" => Ok(PatchType::SnapshotOverride),
        other => Err(ApiErrorBody::invalid_input(format!(
            "unknown patch_type: {other}"
        ))),
    }
}

pub fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ---------------------------------------------------------------------------
// Router builder
// ---------------------------------------------------------------------------

pub fn build_router<S: Storage + 'static, E: ExecutorRegistry + 'static>(
    service: Arc<ReplayKitService<S, E>>,
) -> Router {
    Router::new()
        // Query endpoints
        .route("/api/v1/runs", get(list_runs::<S, E>))
        .route("/api/v1/runs/{run_id}", get(get_run::<S, E>))
        .route("/api/v1/runs/{run_id}/tree", get(get_run_tree::<S, E>))
        .route(
            "/api/v1/runs/{run_id}/timeline",
            get(get_run_timeline::<S, E>),
        )
        .route(
            "/api/v1/runs/{run_id}/spans/{span_id}",
            get(get_span_detail::<S, E>),
        )
        .route(
            "/api/v1/runs/{run_id}/spans/{span_id}/artifacts",
            get(get_span_artifacts::<S, E>),
        )
        .route(
            "/api/v1/runs/{run_id}/spans/{span_id}/dependencies",
            get(get_span_dependencies::<S, E>),
        )
        .route(
            "/api/v1/runs/{source_run_id}/diff/{target_run_id}",
            get(get_diff::<S, E>),
        )
        .route("/api/v1/replay-jobs/{job_id}", get(get_replay_job::<S, E>))
        // Command endpoints
        .route("/api/v1/branches", post(create_branch::<S, E>))
        .route("/api/v1/branches/plan", post(plan_branch::<S, E>))
        .route("/api/v1/diffs", post(compute_diff::<S, E>))
        .with_state(service)
}

// ---------------------------------------------------------------------------
// Query handlers
// ---------------------------------------------------------------------------

async fn list_runs<S: Storage + 'static, E: ExecutorRegistry + 'static>(
    State(svc): State<AppState<S, E>>,
) -> Response {
    match svc.list_runs() {
        Ok(runs) => {
            let views: Vec<RunSummaryView> = runs.iter().map(RunSummaryView::from_record).collect();
            Json(views).into_response()
        }
        Err(e) => err_response(e),
    }
}

async fn get_run<S: Storage + 'static, E: ExecutorRegistry + 'static>(
    State(svc): State<AppState<S, E>>,
    Path(run_id): Path<String>,
) -> Response {
    match svc.get_run(&RunId(run_id)) {
        Ok(run) => Json(RunSummaryView::from_record(&run)).into_response(),
        Err(e) => err_response(e),
    }
}

async fn get_run_tree<S: Storage + 'static, E: ExecutorRegistry + 'static>(
    State(svc): State<AppState<S, E>>,
    Path(run_id): Path<String>,
) -> Response {
    let rid = RunId(run_id);
    let run = match svc.get_run(&rid) {
        Ok(r) => r,
        Err(e) => return err_response(e),
    };
    match svc.run_tree(&rid) {
        Ok(nodes) => Json(RunTreeView::from_parts(&run, &nodes)).into_response(),
        Err(e) => err_response(e),
    }
}

/// Stub: timeline is not yet supported. Returns 501 Not Implemented.
async fn get_run_timeline<S: Storage + 'static, E: ExecutorRegistry + 'static>(
    State(_svc): State<AppState<S, E>>,
    Path(_run_id): Path<String>,
) -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "code": "not_implemented",
            "message": "timeline endpoint is not yet implemented; use /tree for span hierarchy"
        })),
    )
        .into_response()
}

async fn get_span_detail<S: Storage + 'static, E: ExecutorRegistry + 'static>(
    State(svc): State<AppState<S, E>>,
    Path((run_id, span_id)): Path<(String, String)>,
) -> Response {
    match svc.get_span(&RunId(run_id), &SpanId(span_id)) {
        Ok(span) => Json(SpanDetailView::from_record(&span)).into_response(),
        Err(e) => err_response(e),
    }
}

async fn get_span_artifacts<S: Storage + 'static, E: ExecutorRegistry + 'static>(
    State(svc): State<AppState<S, E>>,
    Path((run_id, span_id)): Path<(String, String)>,
) -> Response {
    match svc.span_artifacts(&RunId(run_id), &SpanId(span_id)) {
        Ok(artifacts) => {
            let views: Vec<ArtifactPreviewView> = artifacts
                .iter()
                .map(ArtifactPreviewView::from_record)
                .collect();
            Json(views).into_response()
        }
        Err(e) => err_response(e),
    }
}

async fn get_span_dependencies<S: Storage + 'static, E: ExecutorRegistry + 'static>(
    State(svc): State<AppState<S, E>>,
    Path((run_id, span_id)): Path<(String, String)>,
) -> Response {
    match svc.span_dependencies(&RunId(run_id), &SpanId(span_id)) {
        Ok(edges) => {
            let views: Vec<DependencyView> =
                edges.iter().map(DependencyView::from_record).collect();
            Json(views).into_response()
        }
        Err(e) => err_response(e),
    }
}

async fn get_diff<S: Storage + 'static, E: ExecutorRegistry + 'static>(
    State(svc): State<AppState<S, E>>,
    Path((source_run_id, target_run_id)): Path<(String, String)>,
) -> Response {
    match svc.cached_diff(&RunId(source_run_id), &RunId(target_run_id)) {
        Ok(diff) => Json(RunDiffSummaryView::from_record(&diff)).into_response(),
        Err(e) => err_response(e),
    }
}

async fn get_replay_job<S: Storage + 'static, E: ExecutorRegistry + 'static>(
    State(svc): State<AppState<S, E>>,
    Path(job_id): Path<String>,
) -> Response {
    match svc.get_replay_job(&replaykit_core_model::ReplayJobId(job_id)) {
        Ok(job) => Json(ReplayJobView::from_record(&job)).into_response(),
        Err(e) => err_response(e),
    }
}

// ---------------------------------------------------------------------------
// Command handlers
// ---------------------------------------------------------------------------

async fn create_branch<S: Storage + 'static, E: ExecutorRegistry + 'static>(
    State(svc): State<AppState<S, E>>,
    Json(body): Json<BranchCommandRequest>,
) -> Response {
    let patch_type = match parse_patch_type(&body.patch_type) {
        Ok(pt) => pt,
        Err(e) => return e.into_response(),
    };

    let request = BranchRequest {
        source_run_id: RunId(body.source_run_id),
        fork_span_id: SpanId(body.fork_span_id),
        patch_manifest: PatchManifest {
            patch_type,
            target_artifact_id: body.target_artifact_id.map(ArtifactId),
            replacement: Value::Text(body.replacement),
            note: body.note,
            created_at: now_epoch_secs(),
        },
        created_by: body.created_by,
    };

    match svc.create_branch(request) {
        Ok(exec) => {
            let view = BranchExecutionView::from_parts(
                &exec.branch,
                &exec.target_run,
                &exec.replay_job,
                &exec.plan,
            );
            (StatusCode::CREATED, Json(view)).into_response()
        }
        Err(e) => err_response(e),
    }
}

async fn plan_branch<S: Storage + 'static, E: ExecutorRegistry + 'static>(
    State(svc): State<AppState<S, E>>,
    Json(body): Json<BranchCommandRequest>,
) -> Response {
    let patch_type = match parse_patch_type(&body.patch_type) {
        Ok(pt) => pt,
        Err(e) => return e.into_response(),
    };

    let request = BranchRequest {
        source_run_id: RunId(body.source_run_id),
        fork_span_id: SpanId(body.fork_span_id),
        patch_manifest: PatchManifest {
            patch_type,
            target_artifact_id: body.target_artifact_id.map(ArtifactId),
            replacement: Value::Text(body.replacement),
            note: body.note,
            created_at: now_epoch_secs(),
        },
        created_by: body.created_by,
    };

    match svc.plan_branch(&request) {
        Ok(plan) => Json(BranchPlanView::from_plan(&plan)).into_response(),
        Err(e) => err_response(e),
    }
}

async fn compute_diff<S: Storage + 'static, E: ExecutorRegistry + 'static>(
    State(svc): State<AppState<S, E>>,
    Json(body): Json<ComputeDiffRequest>,
) -> Response {
    match svc.diff_runs(
        &RunId(body.source_run_id),
        &RunId(body.target_run_id),
        now_epoch_secs(),
    ) {
        Ok(diff) => (
            StatusCode::CREATED,
            Json(RunDiffSummaryView::from_record(&diff)),
        )
            .into_response(),
        Err(e) => err_response(e),
    }
}
