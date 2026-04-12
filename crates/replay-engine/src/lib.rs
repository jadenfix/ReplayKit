use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use replaykit_core_model::{
    ArtifactId, ArtifactRecord, ArtifactType, BranchId, BranchPlan, BranchRecord, BranchRequest,
    CostMetrics, DirtyReason, DirtySpanRecord, Document, EdgeKind, PatchType, ReplayJobId,
    ReplayJobRecord, ReplayJobStatus, ReplayMode, RunId, RunRecord, RunStatus, SnapshotRecord,
    SpanEdgeRecord, SpanId, SpanRecord, SpanStatus, Value,
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
    pub output_artifact_ids: Vec<ArtifactId>,
    pub output_fingerprint: Option<String>,
    pub snapshot: Option<SnapshotRecord>,
    pub error_summary: Option<String>,
    pub cost: CostMetrics,
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
    ids: AtomicU64,
}

impl<S: Storage, E: ExecutorRegistry> ReplayEngine<S, E> {
    pub fn new(storage: Arc<S>, executors: E) -> Self {
        Self {
            storage,
            executors,
            ids: AtomicU64::new(1),
        }
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
        dirty_spans.sort_by_key(|record| record.span_id.0.clone());

        let dirty_ids = dirty_spans
            .iter()
            .map(|record| record.span_id.clone())
            .collect::<BTreeSet<_>>();

        let reusable_spans = spans
            .iter()
            .filter(|span| !dirty_ids.contains(&span.span_id))
            .map(|span| span.span_id.clone())
            .collect::<Vec<_>>();

        let blocked_spans = spans
            .iter()
            .filter(|span| dirty_ids.contains(&span.span_id))
            .filter(|span| !self.dirty_span_is_satisfied(span, request))
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

        let target_run_id = RunId(self.next_id("branch-run"));
        let branch_id = BranchId(self.next_id("branch"));
        let replay_job_id = ReplayJobId(self.next_id("job"));
        let now = request.patch_manifest.created_at;

        let mut target_run = source_run.clone();
        target_run.run_id = target_run_id.clone();
        target_run.source_run_id = Some(source_run.run_id.clone());
        target_run.branch_id = Some(branch_id.clone());
        target_run.started_at = now;
        target_run.ended_at = None;
        target_run.status = RunStatus::Running;
        target_run.labels.push("branch".into());
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
            artifact_id: ArtifactId(self.next_id("artifact")),
            run_id: target_run_id.clone(),
            span_id: Some(request.fork_span_id.clone()),
            artifact_type: ArtifactType::PatchManifest,
            mime: "application/replaykit-patch".into(),
            sha256: format!("patch-{}", self.next_id("sha")),
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

            let mut target_span = self.storage.get_span(&target_run_id, &source_span.span_id)?;

            if source_span.span_id == request.fork_span_id {
                self.apply_patch_to_target(
                    &request,
                    &mut target_span,
                    patch_artifact.artifact_id.clone(),
                )?;
                target_span.ended_at = Some(now);
                target_span.status = SpanStatus::Completed;
                self.storage.upsert_span(target_span)?;
                continue;
            }

            if self.executors.supports(&target_span) {
                let result = self.executors.execute(&target_span, &execution_context)?;
                target_span.status = result.status;
                target_span.output_artifact_ids = result.output_artifact_ids;
                target_span.output_fingerprint = result.output_fingerprint;
                target_span.snapshot_id = result.snapshot.map(|snapshot| snapshot.snapshot_id);
                target_span.error_summary = result.error_summary;
                target_span.cost = result.cost;
                target_span.ended_at = Some(now);
                self.storage.upsert_span(target_span)?;
                continue;
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
        target_run.summary.span_count = final_spans.len() as u64;
        target_run.summary.artifact_count = self.storage.list_artifacts(&target_run_id)?.len() as u64;
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
            (PatchType::PromptEdit, replaykit_core_model::SpanKind::LlmCall)
                | (PatchType::ToolOutputOverride, replaykit_core_model::SpanKind::ToolCall)
                | (PatchType::EnvVarOverride, _)
                | (PatchType::ModelConfigEdit, replaykit_core_model::SpanKind::LlmCall)
                | (PatchType::RetrievalContextOverride, replaykit_core_model::SpanKind::Retrieval)
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

    fn dirty_span_is_satisfied(&self, span: &SpanRecord, request: &BranchRequest) -> bool {
        span.span_id == request.fork_span_id
    }

    fn apply_patch_to_target(
        &self,
        request: &BranchRequest,
        span: &mut SpanRecord,
        patch_artifact_id: ArtifactId,
    ) -> Result<(), ReplayError> {
        let replacement_fingerprint = format!(
            "patched:{:?}:{}",
            request.patch_manifest.patch_type, request.patch_manifest.created_at
        );

        match request.patch_manifest.patch_type {
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
            }
        }

        span.attributes.insert(
            "patched".into(),
            Value::Text(format!("{:?}", request.patch_manifest.patch_type)),
        );
        Ok(())
    }

    fn next_id(&self, prefix: &str) -> String {
        let value = self.ids.fetch_add(1, Ordering::SeqCst);
        format!("{prefix}-{value:016x}")
    }
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
        let mut propagated = false;
        if let Some(dependents) = data_dependents.get(&current) {
            for dependent in dependents {
                propagated = true;
                push_dirty(
                    &mut dirty,
                    &mut queue,
                    dependent.clone(),
                    DirtyReason::UpstreamOutputChanged,
                    current.clone(),
                );
            }
        }

        if !propagated {
            if let Some(descendants) = children.get(&current) {
                for child in descendants {
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

    for span in source_spans {
        let mut copied = span.clone();
        copied.run_id = target_run_id.clone();
        storage.upsert_span(copied)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use replaykit_core_model::{
        BranchRequest, CostMetrics, Document, EdgeId, EdgeKind, PatchManifest, PatchType,
        ReplayPolicy, RunRecord, SpanEdgeRecord, SpanKind, SpanRecord, SpanStatus, TraceId, Value,
    };
    use replaykit_storage::InMemoryStorage;

    use super::*;

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
            ..planner.clone()
        };

        storage.upsert_span(planner).unwrap();
        storage.upsert_span(tool).unwrap();
        storage.upsert_span(final_answer).unwrap();
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
}
