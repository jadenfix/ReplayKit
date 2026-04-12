use std::collections::BTreeMap;
use std::sync::Arc;

use replaykit_collector::{
    ArtifactSpec, BeginRun, Collector, CollectorError, EdgeSpec, EndSpan, EventSpec, SnapshotSpec,
    SpanSpec,
};
use replaykit_core_model::{
    BranchPlan, BranchRequest, RunDiffRecord, RunId, RunRecord, RunTreeNode, SpanId, SpanRecord,
};
use replaykit_diff_engine::{DiffEngine, DiffError};
use replaykit_replay_engine::{BranchExecution, ExecutorRegistry, ReplayEngine, ReplayError};
use replaykit_storage::{Storage, StorageError};

#[derive(Debug)]
pub enum ApiError {
    Storage(StorageError),
    Collector(CollectorError),
    Replay(ReplayError),
    Diff(DiffError),
}

impl From<StorageError> for ApiError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

impl From<CollectorError> for ApiError {
    fn from(value: CollectorError) -> Self {
        Self::Collector(value)
    }
}

impl From<ReplayError> for ApiError {
    fn from(value: ReplayError) -> Self {
        Self::Replay(value)
    }
}

impl From<DiffError> for ApiError {
    fn from(value: DiffError) -> Self {
        Self::Diff(value)
    }
}

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

    pub fn begin_run(&self, request: BeginRun) -> Result<RunRecord, ApiError> {
        self.collector.begin_run(request).map_err(Into::into)
    }

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

    pub fn end_span(
        &self,
        run_id: &RunId,
        span_id: &SpanId,
        spec: EndSpan,
    ) -> Result<SpanRecord, ApiError> {
        self.collector.end_span(run_id, span_id, spec).map_err(Into::into)
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

    pub fn list_runs(&self) -> Result<Vec<RunRecord>, ApiError> {
        self.storage.list_runs().map_err(Into::into)
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
        for spans in by_parent.values_mut() {
            spans.sort_by_key(|span| span.sequence_no);
        }
        Ok(build_tree(None, &by_parent))
    }

    pub fn plan_branch(&self, request: &BranchRequest) -> Result<BranchPlan, ApiError> {
        self.replay.plan_fork(request).map_err(Into::into)
    }

    pub fn create_branch(&self, request: BranchRequest) -> Result<BranchExecution, ApiError> {
        let execution = self.replay.execute_fork(request)?;
        let _ = self
            .diff
            .diff_runs(
                &execution.branch.source_run_id,
                &execution.branch.target_run_id,
                execution.branch.created_at,
            )
            .ok();
        Ok(execution)
    }

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
