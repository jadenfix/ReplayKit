pub mod executors;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::sync::Arc;

use replaykit_core_model::{
    ArtifactId, ArtifactRecord, ArtifactType, BranchId, BranchPlan, BranchRecord, BranchRequest,
    CostMetrics, DirtyReason, DirtySpanRecord, Document, EdgeKind, IdKind, PatchType, ReplayJobId,
    ReplayJobRecord, ReplayJobStatus, ReplayMode, RunId, RunRecord, RunStatus, RunSummary,
    SnapshotId, SnapshotRecord, SpanEdgeRecord, SpanId, SpanRecord, SpanStatus, Value,
};
use replaykit_storage::{Storage, StorageError};

#[derive(Debug)]
pub enum ReplayError {
    Storage(StorageError),
    InvalidPatch(String),
    Blocked(String),
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplayError::Storage(err) => write!(f, "{err}"),
            ReplayError::InvalidPatch(message) | ReplayError::Blocked(message) => {
                write!(f, "{message}")
            }
        }
    }
}

impl From<StorageError> for ReplayError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

#[derive(Clone, Debug)]
pub struct ReplayExecutionContext {
    pub source_run_id: RunId,
    pub target_run_id: RunId,
    pub fork_span_id: SpanId,
}

#[derive(Clone, Debug)]
pub struct ExecutionResult {
    pub status: SpanStatus,
    pub output_artifacts: Vec<ProducedArtifact>,
    pub output_fingerprint: Option<String>,
    pub snapshot: Option<ProducedSnapshot>,
    pub error_summary: Option<String>,
    pub cost: CostMetrics,
}

#[derive(Clone, Debug)]
pub struct ProducedArtifact {
    pub artifact_type: ArtifactType,
    pub mime: String,
    pub sha256: String,
    pub byte_len: usize,
    pub blob_path: String,
    pub summary: Document,
    pub redaction: Document,
    pub created_at: u64,
}

#[derive(Clone, Debug)]
pub struct ProducedSnapshot {
    pub kind: String,
    pub artifact: ProducedArtifact,
    pub summary: Document,
    pub created_at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PatchDisposition {
    SatisfiesSpan,
    RequiresExecution,
}

pub trait ExecutorRegistry: Send + Sync {
    fn supports(&self, span: &SpanRecord) -> bool;

    fn execute(
        &self,
        span: &SpanRecord,
        _context: &ReplayExecutionContext,
    ) -> Result<ExecutionResult, ReplayError>;
}

#[derive(Default)]
pub struct NoopExecutorRegistry;

impl ExecutorRegistry for NoopExecutorRegistry {
    fn supports(&self, _span: &SpanRecord) -> bool {
        false
    }

    fn execute(
        &self,
        _span: &SpanRecord,
        _context: &ReplayExecutionContext,
    ) -> Result<ExecutionResult, ReplayError> {
        Err(ReplayError::Blocked(
            "no executor registered for span".into(),
        ))
    }
}

#[derive(Clone, Debug)]
pub struct BranchExecution {
    pub branch: BranchRecord,
    pub replay_job: ReplayJobRecord,
    pub target_run: RunRecord,
    pub plan: BranchPlan,
}

pub struct ReplayEngine<S: Storage, E: ExecutorRegistry> {
    storage: Arc<S>,
    executors: E,
}

impl<S: Storage, E: ExecutorRegistry> ReplayEngine<S, E> {
    pub fn new(storage: Arc<S>, executors: E) -> Self {
        Self { storage, executors }
    }

    pub fn plan_fork(&self, request: &BranchRequest) -> Result<BranchPlan, ReplayError> {
        let spans = self.storage.list_spans(&request.source_run_id)?;
        if spans.is_empty() {
            return Err(ReplayError::InvalidPatch(format!(
                "source run {:?} has no spans",
                request.source_run_id.0
            )));
        }

        let span_map = spans
            .iter()
            .cloned()
            .map(|span| (span.span_id.clone(), span))
            .collect::<BTreeMap<_, _>>();
        let fork_span = span_map.get(&request.fork_span_id).ok_or_else(|| {
            ReplayError::InvalidPatch(format!(
                "fork span {:?} was not found in run {:?}",
                request.fork_span_id.0, request.source_run_id.0
            ))
        })?;
        self.validate_patch_target(fork_span, request)?;

        let edges = self.storage.list_edges(&request.source_run_id)?;
        let dirty_map = compute_dirty_map(&request.fork_span_id, &spans, &edges);
        let mut dirty_spans = dirty_map.into_values().collect::<Vec<_>>();
        dirty_spans.sort_by_key(|record| {
            span_map
                .get(&record.span_id)
                .map(|span| span.sequence_no)
                .unwrap_or(u64::MAX)
        });

        let dirty_ids = dirty_spans
            .iter()
            .map(|record| record.span_id.clone())
            .collect::<BTreeSet<_>>();

        let reusable_spans = spans
            .iter()
            .filter(|span| !dirty_ids.contains(&span.span_id))
            .map(|span| span.span_id.clone())
            .collect::<Vec<_>>();

        let patch_disposition = patch_disposition(request.patch_manifest.patch_type);

        let blocked_spans = spans
            .iter()
            .filter(|span| dirty_ids.contains(&span.span_id))
            .filter(|span| {
                !(span.span_id == request.fork_span_id
                    && patch_disposition == PatchDisposition::SatisfiesSpan)
            })
            .filter(|span| !self.executors.supports(span))
            .map(|span| span.span_id.clone())
            .collect::<Vec<_>>();

        Ok(BranchPlan {
            source_run_id: request.source_run_id.clone(),
            fork_span_id: request.fork_span_id.clone(),
            dirty_spans,
            blocked_spans,
            reusable_spans,
        })
    }

    pub fn execute_fork(&self, request: BranchRequest) -> Result<BranchExecution, ReplayError> {
        let plan = self.plan_fork(&request)?;
        let source_run = self.storage.get_run(&request.source_run_id)?;
        let source_spans = self.storage.list_spans(&request.source_run_id)?;
        let source_artifacts = self.storage.list_artifacts(&request.source_run_id)?;
        let source_snapshots = self.storage.list_snapshots(&request.source_run_id)?;
        let source_edges = self.storage.list_edges(&request.source_run_id)?;
        let source_events = self.storage.list_events(&request.source_run_id)?;

        let target_run_id = RunId(self.storage.allocate_id(IdKind::Run)?);
        let branch_id = BranchId(self.storage.allocate_id(IdKind::Branch)?);
        let replay_job_id = ReplayJobId(self.storage.allocate_id(IdKind::ReplayJob)?);
        let now = request.patch_manifest.created_at;

        let mut target_run = source_run.clone();
        target_run.run_id = target_run_id.clone();
        target_run.source_run_id = Some(source_run.run_id.clone());
        target_run.branch_id = Some(branch_id.clone());
        target_run.started_at = now;
        target_run.ended_at = None;
        target_run.status = RunStatus::Running;
        if !target_run.labels.iter().any(|label| label == "branch") {
            target_run.labels.push("branch".into());
        }
        self.storage.insert_run(target_run.clone())?;

        let replay_job = ReplayJobRecord {
            replay_job_id: replay_job_id.clone(),
            source_run_id: request.source_run_id.clone(),
            target_run_id: Some(target_run_id.clone()),
            mode: ReplayMode::Forked,
            status: ReplayJobStatus::Running,
            created_at: now,
            started_at: Some(now),
            ended_at: None,
            progress: Document::new(),
            error_summary: None,
        };
        self.storage.insert_replay_job(replay_job.clone())?;

        copy_run_data(
            &*self.storage,
            &target_run_id,
            &source_spans,
            &source_artifacts,
            &source_snapshots,
            &source_edges,
            &source_events,
        )?;

        let patch_artifact = ArtifactRecord {
            artifact_id: ArtifactId(self.storage.allocate_id(IdKind::Artifact)?),
            run_id: target_run_id.clone(),
            span_id: Some(request.fork_span_id.clone()),
            artifact_type: ArtifactType::PatchManifest,
            mime: "application/replaykit-patch".into(),
            sha256: format!(
                "patch:{}:{}:{}",
                request.source_run_id.0, request.fork_span_id.0, request.patch_manifest.created_at
            ),
            byte_len: 1,
            blob_path: format!("memory://patch/{}", request.fork_span_id.0),
            summary: patch_summary(&request),
            redaction: Document::new(),
            created_at: now,
        };
        self.storage.insert_artifact(patch_artifact.clone())?;

        let branch = BranchRecord {
            branch_id: branch_id.clone(),
            source_run_id: request.source_run_id.clone(),
            target_run_id: target_run_id.clone(),
            fork_span_id: request.fork_span_id.clone(),
            patch_manifest_artifact_id: patch_artifact.artifact_id.clone(),
            created_at: now,
            created_by: request.created_by.clone(),
            status: RunStatus::Running,
        };
        self.storage.insert_branch(branch.clone())?;

        let dirty_map = plan
            .dirty_spans
            .iter()
            .map(|record| (record.span_id.clone(), record.clone()))
            .collect::<BTreeMap<_, _>>();

        let execution_context = ReplayExecutionContext {
            source_run_id: request.source_run_id.clone(),
            target_run_id: target_run_id.clone(),
            fork_span_id: request.fork_span_id.clone(),
        };

        for source_span in source_spans {
            if !dirty_map.contains_key(&source_span.span_id) {
                continue;
            }

            let mut target_span = self
                .storage
                .get_span(&target_run_id, &source_span.span_id)?;
            let mut patch_disposition = PatchDisposition::RequiresExecution;

            if source_span.span_id == request.fork_span_id {
                patch_disposition = self.apply_patch_to_target(
                    &request,
                    &mut target_span,
                    patch_artifact.artifact_id.clone(),
                )?;
            }

            if patch_disposition == PatchDisposition::SatisfiesSpan {
                target_span.ended_at = Some(now);
                target_span.status = SpanStatus::Completed;
                self.storage.upsert_span(target_span)?;
                continue;
            }

            reset_span_for_replay(&mut target_span);
            if self.executors.supports(&target_span) {
                match self.executors.execute(&target_span, &execution_context) {
                    Ok(result) => {
                        let output_artifact_ids = self.persist_executor_artifacts(
                            &target_run_id,
                            &target_span.span_id,
                            result.output_artifacts,
                        )?;
                        let snapshot_id = self.persist_executor_snapshot(
                            &target_run_id,
                            &target_span.span_id,
                            result.snapshot,
                        )?;
                        target_span.status = result.status;
                        target_span.output_artifact_ids = output_artifact_ids;
                        target_span.output_fingerprint = result.output_fingerprint;
                        target_span.snapshot_id = snapshot_id;
                        target_span.error_summary = result.error_summary;
                        target_span.cost = result.cost;
                        target_span.ended_at = Some(now);
                        self.storage.upsert_span(target_span)?;
                        continue;
                    }
                    Err(ReplayError::Blocked(msg)) => {
                        target_span.status = SpanStatus::Blocked;
                        target_span.ended_at = Some(now);
                        target_span.error_summary = Some(format!("replay blocked: {msg}"));
                        self.storage.upsert_span(target_span)?;
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }

            target_span.status = SpanStatus::Blocked;
            target_span.ended_at = Some(now);
            target_span.error_summary = Some("replay blocked: no executor registered".into());
            self.storage.upsert_span(target_span)?;
        }

        let final_spans = self.storage.list_spans(&target_run_id)?;
        let any_blocked = final_spans
            .iter()
            .any(|span| span.status == SpanStatus::Blocked);
        let any_failed = final_spans
            .iter()
            .any(|span| span.status == SpanStatus::Failed);

        target_run = self.storage.get_run(&target_run_id)?;
        target_run.ended_at = Some(now);
        target_run.status = if any_blocked {
            RunStatus::Blocked
        } else if any_failed {
            RunStatus::Failed
        } else {
            RunStatus::Completed
        };
        let final_artifacts = self.storage.list_artifacts(&target_run_id)?;
        target_run.summary =
            RunSummary::from_run_state(target_run.status, &final_spans, &final_artifacts, None);
        self.storage.update_run(target_run.clone())?;

        let mut branch = self.storage.get_branch(&branch_id)?;
        branch.status = target_run.status;
        self.storage.insert_branch(branch.clone())?;

        let mut replay_job = self.storage.get_replay_job(&replay_job_id)?;
        replay_job.status = match target_run.status {
            RunStatus::Blocked => ReplayJobStatus::Blocked,
            RunStatus::Failed => ReplayJobStatus::Failed,
            _ => ReplayJobStatus::Completed,
        };
        replay_job.ended_at = Some(now);
        self.storage.update_replay_job(replay_job.clone())?;

        Ok(BranchExecution {
            branch,
            replay_job,
            target_run,
            plan,
        })
    }

    fn validate_patch_target(
        &self,
        span: &SpanRecord,
        request: &BranchRequest,
    ) -> Result<(), ReplayError> {
        let supported = matches!(
            (request.patch_manifest.patch_type, span.kind),
            (
                PatchType::PromptEdit,
                replaykit_core_model::SpanKind::LlmCall
            ) | (
                PatchType::ToolOutputOverride,
                replaykit_core_model::SpanKind::ToolCall
            ) | (PatchType::EnvVarOverride, _)
                | (
                    PatchType::ModelConfigEdit,
                    replaykit_core_model::SpanKind::LlmCall
                )
                | (
                    PatchType::RetrievalContextOverride,
                    replaykit_core_model::SpanKind::Retrieval
                )
                | (PatchType::SnapshotOverride, _)
        );

        if supported {
            Ok(())
        } else {
            Err(ReplayError::InvalidPatch(format!(
                "patch {:?} is not valid for span kind {:?}",
                request.patch_manifest.patch_type, span.kind
            )))
        }
    }

    fn apply_patch_to_target(
        &self,
        request: &BranchRequest,
        span: &mut SpanRecord,
        patch_artifact_id: ArtifactId,
    ) -> Result<PatchDisposition, ReplayError> {
        let replacement_fingerprint = format!(
            "patched:{:?}:{}",
            request.patch_manifest.patch_type, request.patch_manifest.created_at
        );

        let disposition = match request.patch_manifest.patch_type {
            PatchType::ToolOutputOverride => {
                if let Some(target_artifact_id) = &request.patch_manifest.target_artifact_id {
                    replace_artifact_id(
                        &mut span.output_artifact_ids,
                        target_artifact_id,
                        patch_artifact_id,
                    )?;
                } else if let Some(slot) = span.output_artifact_ids.first_mut() {
                    *slot = patch_artifact_id;
                } else {
                    span.output_artifact_ids.push(patch_artifact_id);
                }
                span.output_fingerprint = Some(replacement_fingerprint);
                span.error_code = None;
                span.error_summary = None;
                PatchDisposition::SatisfiesSpan
            }
            PatchType::PromptEdit
            | PatchType::EnvVarOverride
            | PatchType::ModelConfigEdit
            | PatchType::RetrievalContextOverride
            | PatchType::SnapshotOverride => {
                if let Some(target_artifact_id) = &request.patch_manifest.target_artifact_id {
                    replace_artifact_id(
                        &mut span.input_artifact_ids,
                        target_artifact_id,
                        patch_artifact_id,
                    )?;
                } else if let Some(slot) = span.input_artifact_ids.first_mut() {
                    *slot = patch_artifact_id;
                } else {
                    span.input_artifact_ids.push(patch_artifact_id);
                }
                span.input_fingerprint = Some(replacement_fingerprint);
                span.output_artifact_ids.clear();
                span.output_fingerprint = None;
                span.snapshot_id = None;
                span.error_code = None;
                span.error_summary = None;
                PatchDisposition::RequiresExecution
            }
        };

        span.attributes.insert(
            "patched".into(),
            Value::Text(format!("{:?}", request.patch_manifest.patch_type)),
        );
        Ok(disposition)
    }

    fn persist_executor_artifacts(
        &self,
        run_id: &RunId,
        span_id: &SpanId,
        artifacts: Vec<ProducedArtifact>,
    ) -> Result<Vec<ArtifactId>, ReplayError> {
        let mut artifact_ids = Vec::with_capacity(artifacts.len());
        for artifact in artifacts {
            let artifact_id = ArtifactId(self.storage.allocate_id(IdKind::Artifact)?);
            self.storage.insert_artifact(ArtifactRecord {
                artifact_id: artifact_id.clone(),
                run_id: run_id.clone(),
                span_id: Some(span_id.clone()),
                artifact_type: artifact.artifact_type,
                mime: artifact.mime,
                sha256: artifact.sha256,
                byte_len: artifact.byte_len,
                blob_path: artifact.blob_path,
                summary: artifact.summary,
                redaction: artifact.redaction,
                created_at: artifact.created_at,
            })?;
            artifact_ids.push(artifact_id);
        }
        Ok(artifact_ids)
    }

    fn persist_executor_snapshot(
        &self,
        run_id: &RunId,
        span_id: &SpanId,
        snapshot: Option<ProducedSnapshot>,
    ) -> Result<Option<SnapshotId>, ReplayError> {
        let Some(snapshot) = snapshot else {
            return Ok(None);
        };

        let artifact_id = ArtifactId(self.storage.allocate_id(IdKind::Artifact)?);
        self.storage.insert_artifact(ArtifactRecord {
            artifact_id: artifact_id.clone(),
            run_id: run_id.clone(),
            span_id: Some(span_id.clone()),
            artifact_type: snapshot.artifact.artifact_type,
            mime: snapshot.artifact.mime,
            sha256: snapshot.artifact.sha256,
            byte_len: snapshot.artifact.byte_len,
            blob_path: snapshot.artifact.blob_path,
            summary: snapshot.artifact.summary,
            redaction: snapshot.artifact.redaction,
            created_at: snapshot.artifact.created_at,
        })?;

        let snapshot_id = SnapshotId(self.storage.allocate_id(IdKind::Snapshot)?);
        self.storage.insert_snapshot(SnapshotRecord {
            snapshot_id: snapshot_id.clone(),
            run_id: run_id.clone(),
            span_id: Some(span_id.clone()),
            kind: snapshot.kind,
            artifact_id,
            summary: snapshot.summary,
            created_at: snapshot.created_at,
        })?;
        Ok(Some(snapshot_id))
    }
}

fn patch_disposition(patch_type: PatchType) -> PatchDisposition {
    match patch_type {
        PatchType::ToolOutputOverride => PatchDisposition::SatisfiesSpan,
        PatchType::PromptEdit
        | PatchType::EnvVarOverride
        | PatchType::ModelConfigEdit
        | PatchType::RetrievalContextOverride
        | PatchType::SnapshotOverride => PatchDisposition::RequiresExecution,
    }
}

fn reset_span_for_replay(span: &mut SpanRecord) {
    span.status = SpanStatus::Running;
    span.ended_at = None;
    span.output_artifact_ids.clear();
    span.snapshot_id = None;
    span.output_fingerprint = None;
    span.error_code = None;
    span.error_summary = None;
    span.cost = CostMetrics::default();
}

fn replace_artifact_id(
    artifact_ids: &mut [ArtifactId],
    old_id: &ArtifactId,
    new_id: ArtifactId,
) -> Result<(), ReplayError> {
    let slot = artifact_ids
        .iter_mut()
        .find(|artifact_id| **artifact_id == *old_id)
        .ok_or_else(|| {
            ReplayError::InvalidPatch(format!(
                "artifact {:?} was not attached to the patched span",
                old_id.0
            ))
        })?;
    *slot = new_id;
    Ok(())
}

fn patch_summary(request: &BranchRequest) -> Document {
    let mut summary = Document::new();
    summary.insert(
        "patch_type".into(),
        Value::Text(format!("{:?}", request.patch_manifest.patch_type)),
    );
    summary.insert(
        "fork_span_id".into(),
        Value::Text(request.fork_span_id.0.clone()),
    );
    summary.insert(
        "replacement".into(),
        request.patch_manifest.replacement.clone(),
    );
    if let Some(note) = &request.patch_manifest.note {
        summary.insert("note".into(), Value::Text(note.clone()));
    }
    summary
}

fn compute_dirty_map(
    fork_span_id: &SpanId,
    spans: &[SpanRecord],
    edges: &[SpanEdgeRecord],
) -> BTreeMap<SpanId, DirtySpanRecord> {
    let mut dirty = BTreeMap::new();
    let children = spans.iter().fold(BTreeMap::new(), |mut acc, span| {
        if let Some(parent) = &span.parent_span_id {
            acc.entry(parent.clone())
                .or_insert_with(Vec::new)
                .push(span.span_id.clone());
        }
        acc
    });

    let data_dependents = edges.iter().fold(BTreeMap::new(), |mut acc, edge| {
        if edge.kind == EdgeKind::DataDependsOn {
            acc.entry(edge.from_span_id.clone())
                .or_insert_with(Vec::new)
                .push(edge.to_span_id.clone());
        }
        acc
    });

    let mut queue = VecDeque::new();
    dirty.insert(
        fork_span_id.clone(),
        DirtySpanRecord {
            span_id: fork_span_id.clone(),
            reasons: vec![DirtyReason::PatchedInput],
            triggered_by: Vec::new(),
        },
    );
    queue.push_back(fork_span_id.clone());

    while let Some(current) = queue.pop_front() {
        let mut data_dependents_for_current = BTreeSet::new();
        if let Some(dependents) = data_dependents.get(&current) {
            for dependent in dependents {
                data_dependents_for_current.insert(dependent.clone());
                push_dirty(
                    &mut dirty,
                    &mut queue,
                    dependent.clone(),
                    DirtyReason::UpstreamOutputChanged,
                    current.clone(),
                );
            }
        }

        if let Some(descendants) = children.get(&current) {
            for child in descendants {
                if data_dependents_for_current.contains(child) {
                    continue;
                }
                push_dirty(
                    &mut dirty,
                    &mut queue,
                    child.clone(),
                    DirtyReason::DependencyUnknown,
                    current.clone(),
                );
            }
        }
    }

    dirty
}

fn push_dirty(
    dirty: &mut BTreeMap<SpanId, DirtySpanRecord>,
    queue: &mut VecDeque<SpanId>,
    span_id: SpanId,
    reason: DirtyReason,
    triggered_by: SpanId,
) {
    if let Some(existing) = dirty.get_mut(&span_id) {
        if !existing.reasons.contains(&reason) {
            existing.reasons.push(reason);
        }
        if !existing.triggered_by.contains(&triggered_by) {
            existing.triggered_by.push(triggered_by);
        }
        return;
    }

    dirty.insert(
        span_id.clone(),
        DirtySpanRecord {
            span_id: span_id.clone(),
            reasons: vec![reason],
            triggered_by: vec![triggered_by],
        },
    );
    queue.push_back(span_id);
}

fn copy_run_data<S: Storage>(
    storage: &S,
    target_run_id: &RunId,
    source_spans: &[SpanRecord],
    source_artifacts: &[ArtifactRecord],
    source_snapshots: &[SnapshotRecord],
    source_edges: &[SpanEdgeRecord],
    source_events: &[replaykit_core_model::EventRecord],
) -> Result<(), ReplayError> {
    for span in source_spans {
        let mut copied = span.clone();
        copied.run_id = target_run_id.clone();
        storage.upsert_span(copied)?;
    }

    for artifact in source_artifacts {
        let mut copied = artifact.clone();
        copied.run_id = target_run_id.clone();
        storage.insert_artifact(copied)?;
    }

    for snapshot in source_snapshots {
        let mut copied = snapshot.clone();
        copied.run_id = target_run_id.clone();
        storage.insert_snapshot(copied)?;
    }

    for edge in source_edges {
        let mut copied = edge.clone();
        copied.run_id = target_run_id.clone();
        storage.insert_edge(copied)?;
    }

    for event in source_events {
        let mut copied = event.clone();
        copied.run_id = target_run_id.clone();
        storage.insert_event(copied)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use replaykit_core_model::{
        ArtifactRecord, ArtifactType, BranchRequest, CostMetrics, Document, EdgeId, EdgeKind,
        PatchManifest, PatchType, ReplayPolicy, RunRecord, RunStatus, SpanEdgeRecord, SpanKind,
        SpanRecord, SpanStatus, TraceId, Value,
    };
    use replaykit_storage::InMemoryStorage;

    use super::*;

    struct FakeExecutorRegistry;

    impl ExecutorRegistry for FakeExecutorRegistry {
        fn supports(&self, span: &SpanRecord) -> bool {
            matches!(span.kind, SpanKind::ToolCall | SpanKind::LlmCall)
        }

        fn execute(
            &self,
            span: &SpanRecord,
            _context: &ReplayExecutionContext,
        ) -> Result<ExecutionResult, ReplayError> {
            let artifact = match span.kind {
                SpanKind::ToolCall => ProducedArtifact {
                    artifact_type: ArtifactType::ToolOutput,
                    mime: "application/json".into(),
                    sha256: format!("tool-output:{}", span.span_id.0),
                    byte_len: 1,
                    blob_path: format!("memory://tool/{}", span.span_id.0),
                    summary: summary_from_pairs(&[("tool", "patched tool output")]),
                    redaction: Document::new(),
                    created_at: 25,
                },
                SpanKind::LlmCall => ProducedArtifact {
                    artifact_type: ArtifactType::ModelResponse,
                    mime: "application/json".into(),
                    sha256: format!("model-output:{}", span.span_id.0),
                    byte_len: 1,
                    blob_path: format!("memory://llm/{}", span.span_id.0),
                    summary: summary_from_pairs(&[("answer", "patched final answer")]),
                    redaction: Document::new(),
                    created_at: 26,
                },
                _ => unreachable!("unsupported span kind for fake executor"),
            };

            let snapshot = matches!(span.kind, SpanKind::LlmCall).then(|| ProducedSnapshot {
                kind: "state".into(),
                artifact: ProducedArtifact {
                    artifact_type: ArtifactType::StateSnapshot,
                    mime: "application/json".into(),
                    sha256: format!("snapshot:{}", span.span_id.0),
                    byte_len: 1,
                    blob_path: format!("memory://snapshot/{}", span.span_id.0),
                    summary: summary_from_pairs(&[("state", "post-replay")]),
                    redaction: Document::new(),
                    created_at: 27,
                },
                summary: summary_from_pairs(&[("state", "post-replay")]),
                created_at: 27,
            });

            Ok(ExecutionResult {
                status: SpanStatus::Completed,
                output_artifacts: vec![artifact],
                output_fingerprint: Some(format!("replayed:{}", span.span_id.0)),
                snapshot,
                error_summary: None,
                cost: CostMetrics {
                    input_tokens: 3,
                    output_tokens: 5,
                    estimated_cost_micros: 11,
                },
            })
        }
    }

    fn insert_run_fixture(storage: &InMemoryStorage) -> RunRecord {
        let run = RunRecord::new(
            RunId("run-src".into()),
            TraceId("trace-src".into()),
            "source",
            "agent.main",
            "adapter",
            "0.1.0",
            1,
        );
        storage.insert_run(run.clone()).unwrap();

        let planner = SpanRecord {
            run_id: run.run_id.clone(),
            span_id: SpanId("planner".into()),
            trace_id: run.trace_id.clone(),
            parent_span_id: None,
            sequence_no: 1,
            kind: SpanKind::PlannerStep,
            name: "planner".into(),
            status: SpanStatus::Completed,
            started_at: 1,
            ended_at: Some(2),
            replay_policy: ReplayPolicy::RecordOnly,
            executor_kind: None,
            executor_version: None,
            input_artifact_ids: Vec::new(),
            output_artifact_ids: Vec::new(),
            snapshot_id: None,
            input_fingerprint: None,
            output_fingerprint: Some("planner-out".into()),
            environment_fingerprint: None,
            attributes: BTreeMap::new(),
            error_code: None,
            error_summary: None,
            cost: CostMetrics::default(),
        };
        let tool = SpanRecord {
            span_id: SpanId("tool".into()),
            sequence_no: 2,
            kind: SpanKind::ToolCall,
            name: "tool".into(),
            replay_policy: ReplayPolicy::RerunnableSupported,
            output_artifact_ids: vec![ArtifactId("tool-output".into())],
            output_fingerprint: Some("tool-out".into()),
            ..planner.clone()
        };
        let final_answer = SpanRecord {
            span_id: SpanId("answer".into()),
            sequence_no: 3,
            kind: SpanKind::LlmCall,
            name: "answer".into(),
            replay_policy: ReplayPolicy::RerunnableSupported,
            parent_span_id: Some(SpanId("planner".into())),
            output_artifact_ids: vec![ArtifactId("answer-output".into())],
            output_fingerprint: Some("answer-out".into()),
            status: SpanStatus::Failed,
            error_summary: Some("failed".into()),
            ..planner.clone()
        };

        storage.upsert_span(planner).unwrap();
        storage.upsert_span(tool).unwrap();
        storage.upsert_span(final_answer).unwrap();
        storage
            .insert_artifact(ArtifactRecord {
                artifact_id: ArtifactId("tool-output".into()),
                run_id: run.run_id.clone(),
                span_id: Some(SpanId("tool".into())),
                artifact_type: ArtifactType::ToolOutput,
                mime: "application/json".into(),
                sha256: "tool-output".into(),
                byte_len: 1,
                blob_path: "memory://tool-output".into(),
                summary: summary_from_pairs(&[("tool", "initial tool output")]),
                redaction: Document::new(),
                created_at: 2,
            })
            .unwrap();
        storage
            .insert_artifact(ArtifactRecord {
                artifact_id: ArtifactId("answer-output".into()),
                run_id: run.run_id.clone(),
                span_id: Some(SpanId("answer".into())),
                artifact_type: ArtifactType::ModelResponse,
                mime: "application/json".into(),
                sha256: "answer-output".into(),
                byte_len: 1,
                blob_path: "memory://answer-output".into(),
                summary: summary_from_pairs(&[("answer", "initial answer")]),
                redaction: Document::new(),
                created_at: 3,
            })
            .unwrap();
        storage
            .insert_edge(SpanEdgeRecord {
                edge_id: EdgeId("edge-1".into()),
                run_id: run.run_id.clone(),
                from_span_id: SpanId("tool".into()),
                to_span_id: SpanId("answer".into()),
                kind: EdgeKind::DataDependsOn,
                attributes: Document::new(),
            })
            .unwrap();
        run
    }

    #[test]
    fn plans_dirty_subgraph_from_data_dependencies() {
        let storage = Arc::new(InMemoryStorage::new());
        let run = insert_run_fixture(&storage);
        let engine = ReplayEngine::new(storage, NoopExecutorRegistry);

        let request = BranchRequest {
            source_run_id: run.run_id,
            fork_span_id: SpanId("tool".into()),
            patch_manifest: PatchManifest {
                patch_type: PatchType::ToolOutputOverride,
                target_artifact_id: Some(ArtifactId("tool-output".into())),
                replacement: Value::Text("patched".into()),
                note: None,
                created_at: 10,
            },
            created_by: None,
        };

        let plan = engine.plan_fork(&request).unwrap();
        let dirty_ids = plan
            .dirty_spans
            .into_iter()
            .map(|record| record.span_id.0)
            .collect::<Vec<_>>();
        assert!(dirty_ids.contains(&"tool".into()));
        assert!(dirty_ids.contains(&"answer".into()));
        assert!(!dirty_ids.contains(&"planner".into()));
    }

    #[test]
    fn plans_dirty_subgraph_for_unknown_children_even_when_data_edges_exist() {
        let storage = Arc::new(InMemoryStorage::new());
        let run = insert_run_fixture(&storage);
        storage
            .upsert_span(SpanRecord {
                run_id: run.run_id.clone(),
                span_id: SpanId("audit".into()),
                trace_id: run.trace_id.clone(),
                parent_span_id: Some(SpanId("tool".into())),
                sequence_no: 4,
                kind: SpanKind::GuardrailCheck,
                name: "audit".into(),
                status: SpanStatus::Completed,
                started_at: 4,
                ended_at: Some(5),
                replay_policy: ReplayPolicy::RecordOnly,
                executor_kind: None,
                executor_version: None,
                input_artifact_ids: Vec::new(),
                output_artifact_ids: Vec::new(),
                snapshot_id: None,
                input_fingerprint: None,
                output_fingerprint: Some("audit-out".into()),
                environment_fingerprint: None,
                attributes: BTreeMap::new(),
                error_code: None,
                error_summary: None,
                cost: CostMetrics::default(),
            })
            .unwrap();
        let engine = ReplayEngine::new(storage, NoopExecutorRegistry);

        let plan = engine
            .plan_fork(&BranchRequest {
                source_run_id: run.run_id,
                fork_span_id: SpanId("tool".into()),
                patch_manifest: PatchManifest {
                    patch_type: PatchType::ToolOutputOverride,
                    target_artifact_id: Some(ArtifactId("tool-output".into())),
                    replacement: Value::Text("patched".into()),
                    note: None,
                    created_at: 10,
                },
                created_by: None,
            })
            .unwrap();

        let dirty_ids = plan
            .dirty_spans
            .into_iter()
            .map(|record| record.span_id.0)
            .collect::<Vec<_>>();
        assert!(dirty_ids.contains(&"tool".into()));
        assert!(dirty_ids.contains(&"answer".into()));
        assert!(dirty_ids.contains(&"audit".into()));
    }

    #[test]
    fn prompt_edit_blocks_without_executor_and_clears_stale_output() {
        let storage = Arc::new(InMemoryStorage::new());
        let run = insert_run_fixture(&storage);
        let engine = ReplayEngine::new(storage.clone(), NoopExecutorRegistry);

        let execution = engine
            .execute_fork(BranchRequest {
                source_run_id: run.run_id,
                fork_span_id: SpanId("answer".into()),
                patch_manifest: PatchManifest {
                    patch_type: PatchType::PromptEdit,
                    target_artifact_id: None,
                    replacement: Value::Text("reword the answer".into()),
                    note: None,
                    created_at: 10,
                },
                created_by: None,
            })
            .unwrap();

        assert_eq!(execution.target_run.status, RunStatus::Blocked);
        assert_eq!(execution.target_run.summary.error_count, 1);
        assert_eq!(
            execution.target_run.summary.failure_class,
            Some(replaykit_core_model::FailureClass::ReplayUnsupported)
        );
        assert_eq!(execution.target_run.summary.token_count, 0);
        assert_eq!(execution.target_run.summary.artifact_count, 3);
        assert_eq!(execution.target_run.summary.final_output_preview, None);
        let answer = storage
            .get_span(&execution.target_run.run_id, &SpanId("answer".into()))
            .unwrap();
        assert_eq!(answer.status, SpanStatus::Blocked);
        assert!(answer.output_artifact_ids.is_empty());
        assert_eq!(
            answer.error_summary.as_deref(),
            Some("replay blocked: no executor registered")
        );
    }

    #[test]
    fn prompt_edit_reruns_and_persists_executor_outputs() {
        let storage = Arc::new(InMemoryStorage::new());
        let run = insert_run_fixture(&storage);
        let engine = ReplayEngine::new(storage.clone(), FakeExecutorRegistry);

        let execution = engine
            .execute_fork(BranchRequest {
                source_run_id: run.run_id,
                fork_span_id: SpanId("answer".into()),
                patch_manifest: PatchManifest {
                    patch_type: PatchType::PromptEdit,
                    target_artifact_id: None,
                    replacement: Value::Text("produce a fixed answer".into()),
                    note: None,
                    created_at: 10,
                },
                created_by: None,
            })
            .unwrap();

        assert_eq!(execution.target_run.status, RunStatus::Completed);
        assert_eq!(execution.target_run.summary.error_count, 0);
        assert_eq!(execution.target_run.summary.failure_class, None);
        assert_eq!(execution.target_run.summary.token_count, 8);
        assert_eq!(execution.target_run.summary.estimated_cost_micros, 11);
        assert_eq!(execution.target_run.summary.artifact_count, 5);
        assert_eq!(execution.target_run.summary.final_output_preview, None);
        let answer = storage
            .get_span(&execution.target_run.run_id, &SpanId("answer".into()))
            .unwrap();
        assert_eq!(answer.status, SpanStatus::Completed);
        assert_eq!(
            answer.output_fingerprint.as_deref(),
            Some("replayed:answer")
        );
        assert_eq!(answer.output_artifact_ids.len(), 1);
        assert_ne!(answer.output_artifact_ids[0].0, "answer-output");
        assert!(answer.snapshot_id.is_some());

        let output_artifact = storage
            .get_artifact(&execution.target_run.run_id, &answer.output_artifact_ids[0])
            .unwrap();
        assert_eq!(output_artifact.artifact_type, ArtifactType::ModelResponse);
        assert_eq!(output_artifact.span_id, Some(SpanId("answer".into())));

        let snapshot = storage
            .get_snapshot(
                &execution.target_run.run_id,
                &answer.snapshot_id.clone().unwrap(),
            )
            .unwrap();
        assert_eq!(snapshot.span_id, Some(SpanId("answer".into())));
        let snapshot_artifact = storage
            .get_artifact(&execution.target_run.run_id, &snapshot.artifact_id)
            .unwrap();
        assert_eq!(snapshot_artifact.artifact_type, ArtifactType::StateSnapshot);
    }

    fn summary_from_pairs(pairs: &[(&str, &str)]) -> Document {
        let mut summary = Document::new();
        for (key, value) in pairs {
            summary.insert((*key).into(), Value::Text((*value).into()));
        }
        summary
    }
}
