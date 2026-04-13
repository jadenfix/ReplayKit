pub mod errors;
pub mod server;
pub mod views;

use std::collections::BTreeMap;
use std::sync::Arc;

use replaykit_collector::{
    ArtifactSpec, BeginRun, Collector, EdgeSpec, EndSpan, EventSpec, SnapshotSpec, SpanSpec,
};
use replaykit_core_model::{
    ArtifactId, ArtifactRecord, BranchPlan, BranchRecord, BranchRequest, ReplayJobId,
    ReplayJobRecord, RunDiffRecord, RunId, RunRecord, RunTreeNode, SpanEdgeRecord, SpanId,
    SpanRecord,
};
use replaykit_diff_engine::DiffEngine;
use replaykit_replay_engine::{BranchExecution, ExecutorRegistry, ReplayEngine};
use replaykit_storage::Storage;

pub use crate::errors::{ApiError, ApiErrorBody};

pub struct ReplayKitService<S: Storage, E: ExecutorRegistry> {
    storage: Arc<S>,
    collector: Collector<S>,
    replay: ReplayEngine<S, E>,
    diff: DiffEngine<S>,
}

impl<S: Storage, E: ExecutorRegistry> ReplayKitService<S, E> {
    pub fn new(storage: Arc<S>, executors: E) -> Self {
        Self {
            collector: Collector::new(storage.clone()),
            replay: ReplayEngine::new(storage.clone(), executors),
            diff: DiffEngine::new(storage.clone()),
            storage,
        }
    }

    // -----------------------------------------------------------------------
    // Run management (collector pass-through)
    // -----------------------------------------------------------------------

    pub fn begin_run(&self, request: BeginRun) -> Result<RunRecord, ApiError> {
        self.collector.begin_run(request).map_err(Into::into)
    }

    pub fn finish_run(
        &self,
        run_id: &RunId,
        ended_at: u64,
        status: replaykit_core_model::RunStatus,
        final_output_preview: Option<String>,
    ) -> Result<RunRecord, ApiError> {
        self.collector
            .finish_run(run_id, ended_at, status, final_output_preview)
            .map_err(Into::into)
    }

    // -----------------------------------------------------------------------
    // Span management (collector pass-through)
    // -----------------------------------------------------------------------

    pub fn start_span(
        &self,
        run_id: &RunId,
        trace_id: &replaykit_core_model::TraceId,
        spec: SpanSpec,
    ) -> Result<SpanRecord, ApiError> {
        self.collector
            .start_span(run_id, trace_id, spec)
            .map_err(Into::into)
    }

    pub fn end_span(
        &self,
        run_id: &RunId,
        span_id: &SpanId,
        spec: EndSpan,
    ) -> Result<SpanRecord, ApiError> {
        self.collector
            .end_span(run_id, span_id, spec)
            .map_err(Into::into)
    }

    // -----------------------------------------------------------------------
    // Data collection (collector pass-through)
    // -----------------------------------------------------------------------

    pub fn add_event(
        &self,
        run_id: &RunId,
        span_id: &SpanId,
        spec: EventSpec,
    ) -> Result<replaykit_core_model::EventRecord, ApiError> {
        self.collector
            .add_event(run_id, span_id, spec)
            .map_err(Into::into)
    }

    pub fn add_artifact(
        &self,
        run_id: &RunId,
        span_id: Option<&SpanId>,
        spec: ArtifactSpec,
    ) -> Result<replaykit_core_model::ArtifactRecord, ApiError> {
        self.collector
            .add_artifact(run_id, span_id, spec)
            .map_err(Into::into)
    }

    pub fn add_snapshot(
        &self,
        run_id: &RunId,
        span_id: Option<&SpanId>,
        spec: SnapshotSpec,
    ) -> Result<replaykit_core_model::SnapshotRecord, ApiError> {
        self.collector
            .add_snapshot(run_id, span_id, spec)
            .map_err(Into::into)
    }

    pub fn add_edge(&self, run_id: &RunId, spec: EdgeSpec) -> Result<(), ApiError> {
        self.collector.add_edge(run_id, spec)?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Query endpoints
    // -----------------------------------------------------------------------

    pub fn list_runs(&self) -> Result<Vec<RunRecord>, ApiError> {
        self.storage.list_runs().map_err(Into::into)
    }

    pub fn get_run(&self, run_id: &RunId) -> Result<RunRecord, ApiError> {
        self.storage.get_run(run_id).map_err(Into::into)
    }

    pub fn get_span(&self, run_id: &RunId, span_id: &SpanId) -> Result<SpanRecord, ApiError> {
        self.storage.get_span(run_id, span_id).map_err(Into::into)
    }

    pub fn run_tree(&self, run_id: &RunId) -> Result<Vec<RunTreeNode>, ApiError> {
        let spans = self.storage.list_spans(run_id)?;
        let mut by_parent = BTreeMap::<Option<SpanId>, Vec<SpanRecord>>::new();
        for span in spans {
            by_parent
                .entry(span.parent_span_id.clone())
                .or_default()
                .push(span);
        }
        for siblings in by_parent.values_mut() {
            siblings.sort_by_key(|span| span.sequence_no);
        }
        Ok(build_tree(None, &by_parent))
    }

    pub fn span_artifacts(
        &self,
        run_id: &RunId,
        span_id: &SpanId,
    ) -> Result<Vec<ArtifactRecord>, ApiError> {
        let span = self.storage.get_span(run_id, span_id)?;
        let all_artifacts = self.storage.list_artifacts(run_id)?;
        let span_artifact_ids: std::collections::BTreeSet<_> = span
            .input_artifact_ids
            .iter()
            .chain(span.output_artifact_ids.iter())
            .collect();
        Ok(all_artifacts
            .into_iter()
            .filter(|a| {
                span_artifact_ids.contains(&a.artifact_id) || a.span_id.as_ref() == Some(span_id)
            })
            .collect())
    }

    pub fn span_dependencies(
        &self,
        run_id: &RunId,
        span_id: &SpanId,
    ) -> Result<Vec<SpanEdgeRecord>, ApiError> {
        let edges = self.storage.list_edges(run_id)?;
        Ok(edges
            .into_iter()
            .filter(|e| e.from_span_id == *span_id || e.to_span_id == *span_id)
            .collect())
    }

    pub fn list_edges(&self, run_id: &RunId) -> Result<Vec<SpanEdgeRecord>, ApiError> {
        self.storage.list_edges(run_id).map_err(Into::into)
    }

    pub fn list_run_branches(&self, run_id: &RunId) -> Result<Vec<BranchRecord>, ApiError> {
        Ok(self
            .storage
            .list_branches()?
            .into_iter()
            .filter(|branch| branch.source_run_id == *run_id || branch.target_run_id == *run_id)
            .collect())
    }

    pub fn get_artifact(
        &self,
        run_id: &RunId,
        artifact_id: &ArtifactId,
    ) -> Result<ArtifactRecord, ApiError> {
        self.storage
            .get_artifact(run_id, artifact_id)
            .map_err(Into::into)
    }

    pub fn read_artifact_content(
        &self,
        run_id: &RunId,
        artifact_id: &ArtifactId,
    ) -> Result<Vec<u8>, ApiError> {
        self.storage
            .read_artifact_content(run_id, artifact_id)
            .map_err(Into::into)
    }

    pub fn get_replay_job(&self, job_id: &ReplayJobId) -> Result<ReplayJobRecord, ApiError> {
        self.storage.get_replay_job(job_id).map_err(Into::into)
    }

    // -----------------------------------------------------------------------
    // Branch and replay
    // -----------------------------------------------------------------------

    pub fn plan_branch(&self, request: &BranchRequest) -> Result<BranchPlan, ApiError> {
        self.replay.plan_fork(request).map_err(Into::into)
    }

    pub fn create_branch(&self, request: BranchRequest) -> Result<BranchExecution, ApiError> {
        let execution = self.replay.execute_fork(request)?;
        self.diff.diff_runs(
            &execution.branch.source_run_id,
            &execution.branch.target_run_id,
            execution.branch.created_at,
        )?;
        Ok(execution)
    }

    // -----------------------------------------------------------------------
    // Diff
    // -----------------------------------------------------------------------

    pub fn diff_runs(
        &self,
        source_run_id: &RunId,
        target_run_id: &RunId,
        created_at: u64,
    ) -> Result<RunDiffRecord, ApiError> {
        self.diff
            .diff_runs(source_run_id, target_run_id, created_at)
            .map_err(Into::into)
    }

    pub fn cached_diff(
        &self,
        source_run_id: &RunId,
        target_run_id: &RunId,
    ) -> Result<RunDiffRecord, ApiError> {
        self.diff
            .get_cached_diff(source_run_id, target_run_id)
            .map_err(Into::into)
    }
}

fn build_tree(
    parent: Option<SpanId>,
    by_parent: &BTreeMap<Option<SpanId>, Vec<SpanRecord>>,
) -> Vec<RunTreeNode> {
    by_parent
        .get(&parent)
        .into_iter()
        .flatten()
        .map(|span| RunTreeNode {
            span: span.clone(),
            children: build_tree(Some(span.span_id.clone()), by_parent),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use replaykit_collector::{ArtifactSpec, BeginRun, EndSpan, SpanSpec};
    use replaykit_core_model::{
        ArtifactType, BranchRequest, CostMetrics, Document, HostMetadata, PatchManifest, PatchType,
        ReplayPolicy, RunStatus, SpanId, SpanKind, SpanStatus, Value,
    };
    use replaykit_replay_engine::{ExecutionResult, ProducedArtifact, ReplayExecutionContext};
    use replaykit_storage::{InMemoryStorage, SqliteStorage, Storage, StorageError};

    use super::*;

    struct FakeExecutorRegistry;

    impl ExecutorRegistry for FakeExecutorRegistry {
        fn supports(&self, span: &SpanRecord) -> bool {
            span.kind == SpanKind::LlmCall
        }

        fn execute(
            &self,
            span: &SpanRecord,
            _context: &ReplayExecutionContext,
        ) -> Result<ExecutionResult, replaykit_replay_engine::ReplayError> {
            Ok(ExecutionResult {
                status: SpanStatus::Completed,
                output_artifacts: vec![ProducedArtifact {
                    artifact_type: ArtifactType::ModelResponse,
                    mime: "application/json".into(),
                    sha256: "patched-answer".into(),
                    byte_len: 1,
                    blob_path: "memory://patched-answer".into(),
                    content: None,
                    summary: summary_from_pairs(&[("answer", "patched answer")]),
                    redaction: Document::new(),
                    created_at: 10,
                }],
                output_fingerprint: Some(format!("replayed:{}", span.span_id.0)),
                snapshot: None,
                error_summary: None,
                cost: CostMetrics {
                    input_tokens: 1,
                    output_tokens: 2,
                    estimated_cost_micros: 3,
                },
            })
        }
    }

    #[test]
    fn create_branch_persists_diff_and_replays_downstream() {
        let storage = Arc::new(InMemoryStorage::new());
        let service = ReplayKitService::new(storage.clone(), FakeExecutorRegistry);
        let run = seed_run(&service);

        let execution = service
            .create_branch(BranchRequest {
                source_run_id: run.run_id.clone(),
                fork_span_id: SpanId("tool".into()),
                patch_manifest: PatchManifest {
                    patch_type: PatchType::ToolOutputOverride,
                    target_artifact_id: None,
                    replacement: Value::Text("patched tool result".into()),
                    note: None,
                    created_at: 20,
                },
                created_by: Some("test".into()),
            })
            .unwrap();

        assert_eq!(execution.target_run.status, RunStatus::Completed);
        let diff = service
            .cached_diff(
                &execution.branch.source_run_id,
                &execution.branch.target_run_id,
            )
            .unwrap();
        assert!(diff.changed_span_count >= 2);

        let answer = storage
            .get_span(&execution.target_run.run_id, &SpanId("answer".into()))
            .unwrap();
        assert_eq!(answer.status, SpanStatus::Completed);
        assert_eq!(
            answer.output_fingerprint.as_deref(),
            Some("replayed:answer")
        );
        assert_eq!(answer.output_artifact_ids.len(), 1);
    }

    #[test]
    fn shared_storage_allocates_unique_run_ids_across_services() {
        let storage = Arc::new(InMemoryStorage::new());
        let first = ReplayKitService::new(storage.clone(), FakeExecutorRegistry);
        let second = ReplayKitService::new(storage, FakeExecutorRegistry);

        let first_run = first.begin_run(sample_begin_run("one")).unwrap();
        let second_run = second.begin_run(sample_begin_run("two")).unwrap();

        assert_ne!(first_run.run_id, second_run.run_id);
        assert_ne!(first_run.trace_id, second_run.trace_id);
    }

    #[test]
    fn sqlite_branch_keeps_source_artifacts_intact() {
        let db_path = unique_db_path("api-branch");
        let storage = Arc::new(SqliteStorage::open(&db_path).unwrap());
        let service = ReplayKitService::new(storage.clone(), FakeExecutorRegistry);
        let run = seed_run(&service);
        let source_artifacts = storage.list_artifacts(&run.run_id).unwrap();

        let execution = service
            .create_branch(BranchRequest {
                source_run_id: run.run_id.clone(),
                fork_span_id: SpanId("tool".into()),
                patch_manifest: PatchManifest {
                    patch_type: PatchType::ToolOutputOverride,
                    target_artifact_id: None,
                    replacement: Value::Text("patched tool result".into()),
                    note: None,
                    created_at: 20,
                },
                created_by: Some("test".into()),
            })
            .unwrap();

        for artifact in &source_artifacts {
            assert_eq!(
                storage
                    .get_artifact(&run.run_id, &artifact.artifact_id)
                    .unwrap()
                    .run_id,
                run.run_id
            );
            assert_eq!(
                storage
                    .get_artifact(&execution.target_run.run_id, &artifact.artifact_id)
                    .unwrap()
                    .run_id,
                execution.target_run.run_id
            );
        }

        let _ = fs::remove_file(db_path);
    }

    #[test]
    fn get_run_returns_not_found_for_missing_run() {
        let storage = Arc::new(InMemoryStorage::new());
        let service = ReplayKitService::new(storage, FakeExecutorRegistry);
        let result = service.get_run(&RunId("nonexistent".into()));
        assert!(result.is_err());
    }

    #[test]
    fn span_artifacts_returns_attached_artifacts() {
        let storage = Arc::new(InMemoryStorage::new());
        let service = ReplayKitService::new(storage, FakeExecutorRegistry);
        let run = seed_run(&service);
        let artifacts = service
            .span_artifacts(&run.run_id, &SpanId("tool".into()))
            .unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].artifact_type, ArtifactType::ToolOutput);
    }

    #[test]
    fn span_dependencies_returns_edges_for_span() {
        let storage = Arc::new(InMemoryStorage::new());
        let service = ReplayKitService::new(storage, FakeExecutorRegistry);
        let run = seed_run(&service);
        let deps = service
            .span_dependencies(&run.run_id, &SpanId("tool".into()))
            .unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].to_span_id, SpanId("answer".into()));
    }

    #[test]
    fn view_models_serialize_correctly() {
        let storage = Arc::new(InMemoryStorage::new());
        let service = ReplayKitService::new(storage, FakeExecutorRegistry);
        let run = seed_run(&service);

        let view = views::RunSummaryView::from_record(&run);
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["status"], "Failed");
        assert_eq!(json["status_label"], "failed");
        assert_eq!(json["is_branch"], false);

        let span = service
            .get_span(&run.run_id, &SpanId("tool".into()))
            .unwrap();
        let span_view = views::SpanDetailView::from_record(&span);
        let span_json = serde_json::to_value(&span_view).unwrap();
        assert_eq!(span_json["kind"], "ToolCall");
        assert_eq!(span_json["status_label"], "completed");
    }

    #[test]
    fn error_body_serializes_with_correct_code() {
        let err = ApiError::Storage(StorageError::NotFound("run not found".into()));
        let body: ApiErrorBody = err.into();
        assert_eq!(body.http_status(), 404);
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["code"], "not_found");
        assert_eq!(json["message"], "run not found");
    }

    #[test]
    fn error_replay_blocked_maps_to_422() {
        let err = ApiError::Replay(replaykit_replay_engine::ReplayError::Blocked(
            "no executor".into(),
        ));
        let body: ApiErrorBody = err.into();
        assert_eq!(body.http_status(), 422);
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["code"], "replay_blocked");
    }

    #[test]
    fn error_invalid_patch_maps_to_400() {
        let err = ApiError::Replay(replaykit_replay_engine::ReplayError::InvalidPatch(
            "bad patch".into(),
        ));
        let body: ApiErrorBody = err.into();
        assert_eq!(body.http_status(), 400);
        assert_eq!(body.code, crate::errors::ErrorCode::InvalidPatch);
    }

    #[test]
    fn error_storage_internal_maps_to_500() {
        let err = ApiError::Storage(StorageError::Internal("db gone".into()));
        let body: ApiErrorBody = err.into();
        assert_eq!(body.http_status(), 500);
        assert_eq!(body.code, crate::errors::ErrorCode::Internal);
    }

    #[test]
    fn run_summary_view_golden_json_shape() {
        let storage = Arc::new(InMemoryStorage::new());
        let service = ReplayKitService::new(storage, FakeExecutorRegistry);
        let run = seed_run(&service);
        let view = views::RunSummaryView::from_record(&run);
        let json = serde_json::to_value(&view).unwrap();
        // Verify all expected top-level keys are present
        for key in &[
            "run_id",
            "title",
            "status",
            "status_label",
            "started_at",
            "ended_at",
            "span_count",
            "error_count",
            "token_count",
            "estimated_cost_micros",
            "failure_class",
            "is_branch",
            "source_run_id",
        ] {
            assert!(
                json.get(key).is_some(),
                "missing key '{key}' in RunSummaryView"
            );
        }
    }

    #[test]
    fn span_detail_view_replay_policy_is_stable_label() {
        let storage = Arc::new(InMemoryStorage::new());
        let service = ReplayKitService::new(storage, FakeExecutorRegistry);
        let run = seed_run(&service);
        let span = service
            .get_span(&run.run_id, &SpanId("tool".into()))
            .unwrap();
        let view = views::SpanDetailView::from_record(&span);
        // Should be a stable snake_case label, not Rust Debug format
        assert_eq!(view.replay_policy, "rerunnable_supported");
    }

    fn seed_run<S: Storage>(service: &ReplayKitService<S, FakeExecutorRegistry>) -> RunRecord {
        let run = service.begin_run(sample_begin_run("demo")).unwrap();

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
                    sha256: "tool-output".into(),
                    byte_len: 1,
                    blob_path: "memory://tool-output".into(),
                    summary: summary_from_pairs(&[("tool", "initial tool output")]),
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
                    sha256: "answer-output".into(),
                    byte_len: 1,
                    blob_path: "memory://answer-output".into(),
                    summary: summary_from_pairs(&[("answer", "failed answer")]),
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
                    error_summary: Some("failed".into()),
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

    fn sample_begin_run(title: &str) -> BeginRun {
        BeginRun {
            title: title.into(),
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
        }
    }

    fn summary_from_pairs(pairs: &[(&str, &str)]) -> Document {
        let mut summary = Document::new();
        for (key, value) in pairs {
            summary.insert((*key).into(), Value::Text((*value).into()));
        }
        summary
    }

    fn unique_db_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("replaykit-{label}-{nonce}.db"))
    }
}
