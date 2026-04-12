use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, RwLock};

use replaykit_core_model::{
    ArtifactId, ArtifactRecord, BranchId, BranchRecord, DirtySpanRecord, EventRecord, IdKind,
    ReplayJobId, ReplayJobRecord, RunDiffRecord, RunId, RunRecord, SnapshotId, SnapshotRecord,
    SpanEdgeRecord, SpanId, SpanRecord,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageError {
    NotFound(String),
    Conflict(String),
    InvalidInput(String),
    Internal(String),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageError::NotFound(message)
            | StorageError::Conflict(message)
            | StorageError::InvalidInput(message)
            | StorageError::Internal(message) => write!(f, "{message}"),
        }
    }
}

pub trait Storage: Send + Sync {
    fn allocate_id(&self, kind: IdKind) -> Result<String, StorageError>;
    fn next_sequence(&self, run_id: &RunId) -> Result<u64, StorageError>;
    fn insert_run(&self, run: RunRecord) -> Result<(), StorageError>;
    fn update_run(&self, run: RunRecord) -> Result<(), StorageError>;
    fn get_run(&self, run_id: &RunId) -> Result<RunRecord, StorageError>;
    fn list_runs(&self) -> Result<Vec<RunRecord>, StorageError>;

    fn upsert_span(&self, span: SpanRecord) -> Result<(), StorageError>;
    fn get_span(&self, run_id: &RunId, span_id: &SpanId) -> Result<SpanRecord, StorageError>;
    fn list_spans(&self, run_id: &RunId) -> Result<Vec<SpanRecord>, StorageError>;

    fn insert_event(&self, event: EventRecord) -> Result<(), StorageError>;
    fn list_events(&self, run_id: &RunId) -> Result<Vec<EventRecord>, StorageError>;

    fn insert_artifact(&self, artifact: ArtifactRecord) -> Result<(), StorageError>;
    fn get_artifact(
        &self,
        run_id: &RunId,
        artifact_id: &ArtifactId,
    ) -> Result<ArtifactRecord, StorageError>;
    fn list_artifacts(&self, run_id: &RunId) -> Result<Vec<ArtifactRecord>, StorageError>;

    fn insert_snapshot(&self, snapshot: SnapshotRecord) -> Result<(), StorageError>;
    fn get_snapshot(
        &self,
        run_id: &RunId,
        snapshot_id: &SnapshotId,
    ) -> Result<SnapshotRecord, StorageError>;
    fn list_snapshots(&self, run_id: &RunId) -> Result<Vec<SnapshotRecord>, StorageError>;

    fn insert_edge(&self, edge: SpanEdgeRecord) -> Result<(), StorageError>;
    fn list_edges(&self, run_id: &RunId) -> Result<Vec<SpanEdgeRecord>, StorageError>;

    fn insert_branch(&self, branch: BranchRecord) -> Result<(), StorageError>;
    fn get_branch(&self, branch_id: &BranchId) -> Result<BranchRecord, StorageError>;

    fn insert_replay_job(&self, job: ReplayJobRecord) -> Result<(), StorageError>;
    fn update_replay_job(&self, job: ReplayJobRecord) -> Result<(), StorageError>;
    fn get_replay_job(&self, replay_job_id: &ReplayJobId) -> Result<ReplayJobRecord, StorageError>;

    fn insert_diff(&self, diff: RunDiffRecord) -> Result<(), StorageError>;
    fn get_diff(&self, source: &RunId, target: &RunId) -> Result<RunDiffRecord, StorageError>;

    fn dirty_spans_for_run(&self, run_id: &RunId) -> Result<Vec<DirtySpanRecord>, StorageError> {
        let _ = run_id;
        Ok(Vec::new())
    }
}

#[derive(Default)]
struct MemoryState {
    runs: BTreeMap<RunId, RunRecord>,
    spans: BTreeMap<RunId, BTreeMap<SpanId, SpanRecord>>,
    events: BTreeMap<RunId, Vec<EventRecord>>,
    artifacts: BTreeMap<RunId, BTreeMap<ArtifactId, ArtifactRecord>>,
    snapshots: BTreeMap<RunId, BTreeMap<SnapshotId, SnapshotRecord>>,
    edges: BTreeMap<RunId, Vec<SpanEdgeRecord>>,
    branches: BTreeMap<BranchId, BranchRecord>,
    replay_jobs: BTreeMap<ReplayJobId, ReplayJobRecord>,
    diffs: BTreeMap<(RunId, RunId), RunDiffRecord>,
    sequences: BTreeMap<RunId, u64>,
    id_counters: BTreeMap<IdKind, u64>,
}

#[derive(Clone, Default)]
pub struct InMemoryStorage {
    state: Arc<RwLock<MemoryState>>,
}

impl InMemoryStorage {
    pub fn new() -> Self {
        Self::default()
    }

    fn ensure_run_exists(state: &MemoryState, run_id: &RunId) -> Result<(), StorageError> {
        if state.runs.contains_key(run_id) {
            Ok(())
        } else {
            Err(StorageError::NotFound(format!(
                "run {:?} not found",
                run_id.0
            )))
        }
    }
}

impl Storage for InMemoryStorage {
    fn allocate_id(&self, kind: IdKind) -> Result<String, StorageError> {
        let mut state = self.state.write().map_err(|_| {
            StorageError::Internal("failed to lock storage for id allocation".into())
        })?;
        let next = state.id_counters.entry(kind).or_insert(0);
        *next += 1;
        Ok(format!("{}-{next:016x}", kind.prefix()))
    }

    fn next_sequence(&self, run_id: &RunId) -> Result<u64, StorageError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| StorageError::Internal("failed to lock storage for sequence".into()))?;
        Self::ensure_run_exists(&state, run_id)?;
        let next = state.sequences.entry(run_id.clone()).or_insert(0);
        *next += 1;
        Ok(*next)
    }

    fn insert_run(&self, run: RunRecord) -> Result<(), StorageError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| StorageError::Internal("failed to lock storage for run insert".into()))?;
        if state.runs.contains_key(&run.run_id) {
            return Err(StorageError::Conflict(format!(
                "run {:?} already exists",
                run.run_id.0
            )));
        }
        state.sequences.insert(run.run_id.clone(), 0);
        state.runs.insert(run.run_id.clone(), run);
        Ok(())
    }

    fn update_run(&self, run: RunRecord) -> Result<(), StorageError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| StorageError::Internal("failed to lock storage for run update".into()))?;
        Self::ensure_run_exists(&state, &run.run_id)?;
        state.runs.insert(run.run_id.clone(), run);
        Ok(())
    }

    fn get_run(&self, run_id: &RunId) -> Result<RunRecord, StorageError> {
        let state = self
            .state
            .read()
            .map_err(|_| StorageError::Internal("failed to lock storage for run read".into()))?;
        state
            .runs
            .get(run_id)
            .cloned()
            .ok_or_else(|| StorageError::NotFound(format!("run {:?} not found", run_id.0)))
    }

    fn list_runs(&self) -> Result<Vec<RunRecord>, StorageError> {
        let state = self
            .state
            .read()
            .map_err(|_| StorageError::Internal("failed to lock storage for run list".into()))?;
        let mut runs = state.runs.values().cloned().collect::<Vec<_>>();
        runs.sort_by_key(|run| run.started_at);
        Ok(runs)
    }

    fn upsert_span(&self, span: SpanRecord) -> Result<(), StorageError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| StorageError::Internal("failed to lock storage for span upsert".into()))?;
        Self::ensure_run_exists(&state, &span.run_id)?;
        state
            .spans
            .entry(span.run_id.clone())
            .or_default()
            .insert(span.span_id.clone(), span);
        Ok(())
    }

    fn get_span(&self, run_id: &RunId, span_id: &SpanId) -> Result<SpanRecord, StorageError> {
        let state = self
            .state
            .read()
            .map_err(|_| StorageError::Internal("failed to lock storage for span read".into()))?;
        state
            .spans
            .get(run_id)
            .and_then(|spans| spans.get(span_id))
            .cloned()
            .ok_or_else(|| {
                StorageError::NotFound(format!(
                    "span {:?} for run {:?} not found",
                    span_id.0, run_id.0
                ))
            })
    }

    fn list_spans(&self, run_id: &RunId) -> Result<Vec<SpanRecord>, StorageError> {
        let state = self
            .state
            .read()
            .map_err(|_| StorageError::Internal("failed to lock storage for span list".into()))?;
        let mut spans = state
            .spans
            .get(run_id)
            .map(|records| records.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        spans.sort_by_key(|span| span.sequence_no);
        Ok(spans)
    }

    fn insert_event(&self, event: EventRecord) -> Result<(), StorageError> {
        let mut state = self.state.write().map_err(|_| {
            StorageError::Internal("failed to lock storage for event insert".into())
        })?;
        Self::ensure_run_exists(&state, &event.run_id)?;
        state
            .events
            .entry(event.run_id.clone())
            .or_default()
            .push(event);
        Ok(())
    }

    fn list_events(&self, run_id: &RunId) -> Result<Vec<EventRecord>, StorageError> {
        let state = self
            .state
            .read()
            .map_err(|_| StorageError::Internal("failed to lock storage for event list".into()))?;
        Ok(state.events.get(run_id).cloned().unwrap_or_default())
    }

    fn insert_artifact(&self, artifact: ArtifactRecord) -> Result<(), StorageError> {
        let mut state = self.state.write().map_err(|_| {
            StorageError::Internal("failed to lock storage for artifact insert".into())
        })?;
        Self::ensure_run_exists(&state, &artifact.run_id)?;
        state
            .artifacts
            .entry(artifact.run_id.clone())
            .or_default()
            .insert(artifact.artifact_id.clone(), artifact);
        Ok(())
    }

    fn get_artifact(
        &self,
        run_id: &RunId,
        artifact_id: &ArtifactId,
    ) -> Result<ArtifactRecord, StorageError> {
        let state = self.state.read().map_err(|_| {
            StorageError::Internal("failed to lock storage for artifact read".into())
        })?;
        state
            .artifacts
            .get(run_id)
            .and_then(|records| records.get(artifact_id))
            .cloned()
            .ok_or_else(|| {
                StorageError::NotFound(format!(
                    "artifact {:?} for run {:?} not found",
                    artifact_id.0, run_id.0
                ))
            })
    }

    fn list_artifacts(&self, run_id: &RunId) -> Result<Vec<ArtifactRecord>, StorageError> {
        let state = self.state.read().map_err(|_| {
            StorageError::Internal("failed to lock storage for artifact list".into())
        })?;
        let mut artifacts = state
            .artifacts
            .get(run_id)
            .map(|records| records.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        artifacts.sort_by_key(|artifact| artifact.created_at);
        Ok(artifacts)
    }

    fn insert_snapshot(&self, snapshot: SnapshotRecord) -> Result<(), StorageError> {
        let mut state = self.state.write().map_err(|_| {
            StorageError::Internal("failed to lock storage for snapshot insert".into())
        })?;
        Self::ensure_run_exists(&state, &snapshot.run_id)?;
        state
            .snapshots
            .entry(snapshot.run_id.clone())
            .or_default()
            .insert(snapshot.snapshot_id.clone(), snapshot);
        Ok(())
    }

    fn get_snapshot(
        &self,
        run_id: &RunId,
        snapshot_id: &SnapshotId,
    ) -> Result<SnapshotRecord, StorageError> {
        let state = self.state.read().map_err(|_| {
            StorageError::Internal("failed to lock storage for snapshot read".into())
        })?;
        state
            .snapshots
            .get(run_id)
            .and_then(|records| records.get(snapshot_id))
            .cloned()
            .ok_or_else(|| {
                StorageError::NotFound(format!(
                    "snapshot {:?} for run {:?} not found",
                    snapshot_id.0, run_id.0
                ))
            })
    }

    fn list_snapshots(&self, run_id: &RunId) -> Result<Vec<SnapshotRecord>, StorageError> {
        let state = self.state.read().map_err(|_| {
            StorageError::Internal("failed to lock storage for snapshot list".into())
        })?;
        let mut snapshots = state
            .snapshots
            .get(run_id)
            .map(|records| records.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        snapshots.sort_by_key(|snapshot| snapshot.created_at);
        Ok(snapshots)
    }

    fn insert_edge(&self, edge: SpanEdgeRecord) -> Result<(), StorageError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| StorageError::Internal("failed to lock storage for edge insert".into()))?;
        Self::ensure_run_exists(&state, &edge.run_id)?;
        state
            .edges
            .entry(edge.run_id.clone())
            .or_default()
            .push(edge);
        Ok(())
    }

    fn list_edges(&self, run_id: &RunId) -> Result<Vec<SpanEdgeRecord>, StorageError> {
        let state = self
            .state
            .read()
            .map_err(|_| StorageError::Internal("failed to lock storage for edge list".into()))?;
        Ok(state.edges.get(run_id).cloned().unwrap_or_default())
    }

    fn insert_branch(&self, branch: BranchRecord) -> Result<(), StorageError> {
        let mut state = self.state.write().map_err(|_| {
            StorageError::Internal("failed to lock storage for branch insert".into())
        })?;
        state.branches.insert(branch.branch_id.clone(), branch);
        Ok(())
    }

    fn get_branch(&self, branch_id: &BranchId) -> Result<BranchRecord, StorageError> {
        let state = self
            .state
            .read()
            .map_err(|_| StorageError::Internal("failed to lock storage for branch read".into()))?;
        state
            .branches
            .get(branch_id)
            .cloned()
            .ok_or_else(|| StorageError::NotFound(format!("branch {:?} not found", branch_id.0)))
    }

    fn insert_replay_job(&self, job: ReplayJobRecord) -> Result<(), StorageError> {
        let mut state = self.state.write().map_err(|_| {
            StorageError::Internal("failed to lock storage for replay job insert".into())
        })?;
        state.replay_jobs.insert(job.replay_job_id.clone(), job);
        Ok(())
    }

    fn update_replay_job(&self, job: ReplayJobRecord) -> Result<(), StorageError> {
        let mut state = self.state.write().map_err(|_| {
            StorageError::Internal("failed to lock storage for replay job update".into())
        })?;
        state.replay_jobs.insert(job.replay_job_id.clone(), job);
        Ok(())
    }

    fn get_replay_job(&self, replay_job_id: &ReplayJobId) -> Result<ReplayJobRecord, StorageError> {
        let state = self.state.read().map_err(|_| {
            StorageError::Internal("failed to lock storage for replay job read".into())
        })?;
        state
            .replay_jobs
            .get(replay_job_id)
            .cloned()
            .ok_or_else(|| {
                StorageError::NotFound(format!("replay job {:?} not found", replay_job_id.0))
            })
    }

    fn insert_diff(&self, diff: RunDiffRecord) -> Result<(), StorageError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| StorageError::Internal("failed to lock storage for diff insert".into()))?;
        state.diffs.insert(
            (diff.source_run_id.clone(), diff.target_run_id.clone()),
            diff,
        );
        Ok(())
    }

    fn get_diff(&self, source: &RunId, target: &RunId) -> Result<RunDiffRecord, StorageError> {
        let state = self
            .state
            .read()
            .map_err(|_| StorageError::Internal("failed to lock storage for diff read".into()))?;
        state
            .diffs
            .get(&(source.clone(), target.clone()))
            .cloned()
            .ok_or_else(|| {
                StorageError::NotFound(format!(
                    "diff for runs {:?} -> {:?} not found",
                    source.0, target.0
                ))
            })
    }
}

pub fn root_spans(spans: &[SpanRecord]) -> Vec<SpanRecord> {
    let known_span_ids = spans
        .iter()
        .map(|span| span.span_id.clone())
        .collect::<BTreeSet<_>>();

    spans
        .iter()
        .filter(|span| match &span.parent_span_id {
            None => true,
            Some(parent_id) => !known_span_ids.contains(parent_id),
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use replaykit_core_model::{
        HostMetadata, ReplayPolicy, RunRecord, RunStatus, SpanKind, SpanRecord, SpanStatus, TraceId,
    };

    use super::*;

    fn sample_run() -> RunRecord {
        let mut run = RunRecord::new(
            RunId("run-1".into()),
            TraceId("trace-1".into()),
            "demo",
            "demo.main",
            "test-adapter",
            "0.1.0",
            1,
        );
        run.host = HostMetadata {
            os: "macos".into(),
            arch: "arm64".into(),
            hostname: Some("localhost".into()),
        };
        run.status = RunStatus::Running;
        run
    }

    #[test]
    fn round_trips_runs_and_spans() {
        let storage = InMemoryStorage::new();
        let run = sample_run();
        storage.insert_run(run.clone()).unwrap();

        let span = SpanRecord {
            run_id: run.run_id.clone(),
            span_id: SpanId("span-1".into()),
            trace_id: run.trace_id.clone(),
            parent_span_id: None,
            sequence_no: storage.next_sequence(&run.run_id).unwrap(),
            kind: SpanKind::Run,
            name: "root".into(),
            status: SpanStatus::Running,
            started_at: 1,
            ended_at: None,
            replay_policy: ReplayPolicy::RecordOnly,
            executor_kind: None,
            executor_version: None,
            input_artifact_ids: Vec::new(),
            output_artifact_ids: Vec::new(),
            snapshot_id: None,
            input_fingerprint: None,
            output_fingerprint: None,
            environment_fingerprint: None,
            attributes: BTreeMap::new(),
            error_code: None,
            error_summary: None,
            cost: Default::default(),
        };

        storage.upsert_span(span.clone()).unwrap();

        assert_eq!(storage.get_run(&run.run_id).unwrap().title, "demo");
        assert_eq!(
            storage.get_span(&run.run_id, &span.span_id).unwrap().name,
            "root"
        );
        assert_eq!(storage.list_spans(&run.run_id).unwrap().len(), 1);
    }

    #[test]
    fn allocates_unique_ids_across_calls() {
        let storage = InMemoryStorage::new();

        let first_run = storage.allocate_id(IdKind::Run).unwrap();
        let second_run = storage.allocate_id(IdKind::Run).unwrap();
        let first_span = storage.allocate_id(IdKind::Span).unwrap();

        assert_ne!(first_run, second_run);
        assert!(first_run.starts_with("run-"));
        assert!(second_run.starts_with("run-"));
        assert!(first_span.starts_with("span-"));
    }
}
