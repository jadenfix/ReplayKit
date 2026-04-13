pub mod blob;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use replaykit_core_model::{
    ArtifactId, ArtifactRecord, BranchId, BranchRecord, DirtySpanRecord, EventRecord, IdKind,
    ReplayJobId, ReplayJobRecord, RunDiffRecord, RunId, RunRecord, RunStatus, SnapshotId,
    SnapshotRecord, SpanEdgeRecord, SpanId, SpanRecord,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use blob::{BlobIntegrity, BlobRef, BlobStore, InMemoryBlobStore, LocalBlobStore, sha256_hex};

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

impl std::error::Error for StorageError {}

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
    fn list_branches(&self) -> Result<Vec<BranchRecord>, StorageError>;

    fn insert_replay_job(&self, job: ReplayJobRecord) -> Result<(), StorageError>;
    fn update_replay_job(&self, job: ReplayJobRecord) -> Result<(), StorageError>;
    fn get_replay_job(&self, replay_job_id: &ReplayJobId) -> Result<ReplayJobRecord, StorageError>;

    fn insert_diff(&self, diff: RunDiffRecord) -> Result<(), StorageError>;
    fn get_diff(&self, source: &RunId, target: &RunId) -> Result<RunDiffRecord, StorageError>;

    /// Store an artifact together with its binary content. The implementation
    /// writes the content to a blob store first, then inserts the metadata.
    fn store_artifact_with_content(
        &self,
        artifact: ArtifactRecord,
        content: &[u8],
    ) -> Result<ArtifactRecord, StorageError>;

    /// Read the binary content of a previously stored artifact.
    fn read_artifact_content(
        &self,
        run_id: &RunId,
        artifact_id: &ArtifactId,
    ) -> Result<Vec<u8>, StorageError>;

    /// Verify the on-disk integrity of an artifact's blob.
    fn verify_artifact_integrity(
        &self,
        run_id: &RunId,
        artifact_id: &ArtifactId,
    ) -> Result<BlobIntegrity, StorageError>;

    fn dirty_spans_for_run(&self, run_id: &RunId) -> Result<Vec<DirtySpanRecord>, StorageError> {
        let _ = run_id;
        Ok(Vec::new())
    }
}

/// Report produced by an integrity scan for a single artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactIntegrityReport {
    pub artifact_id: ArtifactId,
    pub run_id: RunId,
    pub status: BlobIntegrity,
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
    blob_store: InMemoryBlobStore,
}

impl InMemoryStorage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn blob_store(&self) -> &InMemoryBlobStore {
        &self.blob_store
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

    fn ensure_span_exists(
        state: &MemoryState,
        run_id: &RunId,
        span_id: &SpanId,
    ) -> Result<(), StorageError> {
        state
            .spans
            .get(run_id)
            .and_then(|spans| spans.get(span_id))
            .map(|_| ())
            .ok_or_else(|| {
                StorageError::NotFound(format!(
                    "span {:?} for run {:?} not found",
                    span_id.0, run_id.0
                ))
            })
    }

    fn get_artifact_record(
        state: &MemoryState,
        run_id: &RunId,
        artifact_id: &ArtifactId,
    ) -> Result<ArtifactRecord, StorageError> {
        let artifact = state
            .artifacts
            .get(run_id)
            .and_then(|artifacts| artifacts.get(artifact_id))
            .cloned()
            .ok_or_else(|| {
                StorageError::NotFound(format!(
                    "artifact {:?} for run {:?} not found",
                    artifact_id.0, run_id.0
                ))
            })?;
        validate_artifact_record(&artifact)?;
        Ok(artifact)
    }

    fn ensure_artifact_attached_to_span(
        artifact: &ArtifactRecord,
        span_id: &SpanId,
        label: &str,
    ) -> Result<(), StorageError> {
        match &artifact.span_id {
            Some(existing_span_id) if existing_span_id == span_id => Ok(()),
            Some(existing_span_id) => Err(StorageError::InvalidInput(format!(
                "{label} {:?} belongs to span {:?}, not {:?}",
                artifact.artifact_id.0, existing_span_id.0, span_id.0
            ))),
            None => Err(StorageError::InvalidInput(format!(
                "{label} {:?} is not attached to span {:?}",
                artifact.artifact_id.0, span_id.0
            ))),
        }
    }
}

fn validate_artifact_record(artifact: &ArtifactRecord) -> Result<(), StorageError> {
    if artifact.mime.trim().is_empty() {
        return Err(StorageError::InvalidInput(format!(
            "artifact {:?} has an empty mime type",
            artifact.artifact_id.0
        )));
    }
    if artifact.sha256.trim().is_empty() {
        return Err(StorageError::InvalidInput(format!(
            "artifact {:?} has an empty sha256",
            artifact.artifact_id.0
        )));
    }
    if artifact.blob_path.trim().is_empty() {
        return Err(StorageError::InvalidInput(format!(
            "artifact {:?} has an empty blob_path",
            artifact.artifact_id.0
        )));
    }

    let Some(blob_path) = local_blob_path(&artifact.blob_path) else {
        return Ok(());
    };

    let metadata = fs::metadata(&blob_path).map_err(|err| {
        StorageError::InvalidInput(format!(
            "artifact {:?} references unreadable blob {:?}: {err}",
            artifact.artifact_id.0, blob_path
        ))
    })?;
    if !metadata.is_file() {
        return Err(StorageError::InvalidInput(format!(
            "artifact {:?} references blob {:?}, which is not a regular file",
            artifact.artifact_id.0, blob_path
        )));
    }
    if metadata.len() != artifact.byte_len as u64 {
        return Err(StorageError::InvalidInput(format!(
            "artifact {:?} expected blob {:?} to be {} bytes but found {}",
            artifact.artifact_id.0,
            blob_path,
            artifact.byte_len,
            metadata.len()
        )));
    }
    if !looks_like_sha256(&artifact.sha256) {
        return Err(StorageError::InvalidInput(format!(
            "artifact {:?} uses local blob {:?} but sha256 is not a 64-character hex digest",
            artifact.artifact_id.0, blob_path
        )));
    }

    Ok(())
}

fn local_blob_path(blob_path: &str) -> Option<PathBuf> {
    if blob_path.starts_with("memory://") {
        return None;
    }
    if let Some(path) = blob_path.strip_prefix("file://") {
        return Some(PathBuf::from(path));
    }
    if blob_path.contains("://") {
        return None;
    }
    Some(PathBuf::from(blob_path))
}

fn looks_like_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
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
        if let Some(parent_span_id) = &span.parent_span_id {
            if *parent_span_id == span.span_id {
                return Err(StorageError::InvalidInput(format!(
                    "span {:?} cannot be its own parent",
                    span.span_id.0
                )));
            }
            Self::ensure_span_exists(&state, &span.run_id, parent_span_id)?;
        }
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
        Self::ensure_span_exists(&state, &event.run_id, &event.span_id)?;
        if state
            .events
            .get(&event.run_id)
            .into_iter()
            .flatten()
            .any(|existing| existing.event_id == event.event_id)
        {
            return Err(StorageError::Conflict(format!(
                "event {:?} already exists in run {:?}",
                event.event_id.0, event.run_id.0
            )));
        }
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
        if let Some(span_id) = &artifact.span_id {
            Self::ensure_span_exists(&state, &artifact.run_id, span_id)?;
        }
        validate_artifact_record(&artifact)?;
        if state
            .artifacts
            .get(&artifact.run_id)
            .and_then(|records| records.get(&artifact.artifact_id))
            .is_some()
        {
            return Err(StorageError::Conflict(format!(
                "artifact {:?} already exists in run {:?}",
                artifact.artifact_id.0, artifact.run_id.0
            )));
        }
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
        let artifact = state
            .artifacts
            .get(run_id)
            .and_then(|records| records.get(artifact_id))
            .cloned()
            .ok_or_else(|| {
                StorageError::NotFound(format!(
                    "artifact {:?} for run {:?} not found",
                    artifact_id.0, run_id.0
                ))
            })?;
        validate_artifact_record(&artifact)?;
        Ok(artifact)
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
        for artifact in &artifacts {
            validate_artifact_record(artifact)?;
        }
        artifacts.sort_by_key(|artifact| artifact.created_at);
        Ok(artifacts)
    }

    fn insert_snapshot(&self, snapshot: SnapshotRecord) -> Result<(), StorageError> {
        let mut state = self.state.write().map_err(|_| {
            StorageError::Internal("failed to lock storage for snapshot insert".into())
        })?;
        Self::ensure_run_exists(&state, &snapshot.run_id)?;
        if let Some(span_id) = &snapshot.span_id {
            Self::ensure_span_exists(&state, &snapshot.run_id, span_id)?;
        }
        let artifact = Self::get_artifact_record(&state, &snapshot.run_id, &snapshot.artifact_id)?;
        if let Some(span_id) = &snapshot.span_id {
            Self::ensure_artifact_attached_to_span(&artifact, span_id, "snapshot artifact")?;
        }
        if state
            .snapshots
            .get(&snapshot.run_id)
            .and_then(|records| records.get(&snapshot.snapshot_id))
            .is_some()
        {
            return Err(StorageError::Conflict(format!(
                "snapshot {:?} already exists in run {:?}",
                snapshot.snapshot_id.0, snapshot.run_id.0
            )));
        }
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
        Self::ensure_span_exists(&state, &edge.run_id, &edge.from_span_id)?;
        Self::ensure_span_exists(&state, &edge.run_id, &edge.to_span_id)?;
        if state
            .edges
            .get(&edge.run_id)
            .into_iter()
            .flatten()
            .any(|existing| existing.edge_id == edge.edge_id)
        {
            return Err(StorageError::Conflict(format!(
                "edge {:?} already exists in run {:?}",
                edge.edge_id.0, edge.run_id.0
            )));
        }
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
        Self::ensure_run_exists(&state, &branch.source_run_id)?;
        Self::ensure_run_exists(&state, &branch.target_run_id)?;
        Self::ensure_span_exists(&state, &branch.source_run_id, &branch.fork_span_id)?;
        Self::get_artifact_record(
            &state,
            &branch.target_run_id,
            &branch.patch_manifest_artifact_id,
        )?;
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

    fn list_branches(&self) -> Result<Vec<BranchRecord>, StorageError> {
        let state = self
            .state
            .read()
            .map_err(|_| StorageError::Internal("failed to lock storage for branch list".into()))?;
        Ok(state.branches.values().cloned().collect())
    }

    fn insert_replay_job(&self, job: ReplayJobRecord) -> Result<(), StorageError> {
        let mut state = self.state.write().map_err(|_| {
            StorageError::Internal("failed to lock storage for replay job insert".into())
        })?;
        Self::ensure_run_exists(&state, &job.source_run_id)?;
        if let Some(target_run_id) = &job.target_run_id {
            Self::ensure_run_exists(&state, target_run_id)?;
        }
        if state.replay_jobs.contains_key(&job.replay_job_id) {
            return Err(StorageError::Conflict(format!(
                "replay job {:?} already exists",
                job.replay_job_id.0
            )));
        }
        state.replay_jobs.insert(job.replay_job_id.clone(), job);
        Ok(())
    }

    fn update_replay_job(&self, job: ReplayJobRecord) -> Result<(), StorageError> {
        let mut state = self.state.write().map_err(|_| {
            StorageError::Internal("failed to lock storage for replay job update".into())
        })?;
        Self::ensure_run_exists(&state, &job.source_run_id)?;
        if let Some(target_run_id) = &job.target_run_id {
            Self::ensure_run_exists(&state, target_run_id)?;
        }
        if !state.replay_jobs.contains_key(&job.replay_job_id) {
            return Err(StorageError::NotFound(format!(
                "replay job {:?} not found",
                job.replay_job_id.0
            )));
        }
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
        Self::ensure_run_exists(&state, &diff.source_run_id)?;
        Self::ensure_run_exists(&state, &diff.target_run_id)?;
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

    fn store_artifact_with_content(
        &self,
        mut artifact: ArtifactRecord,
        content: &[u8],
    ) -> Result<ArtifactRecord, StorageError> {
        let blob_ref = self.blob_store.store(content)?;
        artifact.sha256 = blob_ref.sha256.clone();
        artifact.byte_len = blob_ref.byte_len as usize;
        artifact.blob_path = self
            .blob_store
            .blob_path(&blob_ref)
            .to_string_lossy()
            .to_string();
        self.insert_artifact(artifact.clone())?;
        Ok(artifact)
    }

    fn read_artifact_content(
        &self,
        run_id: &RunId,
        artifact_id: &ArtifactId,
    ) -> Result<Vec<u8>, StorageError> {
        let artifact = self.get_artifact(run_id, artifact_id)?;
        let blob_ref = BlobRef {
            sha256: artifact.sha256,
            byte_len: artifact.byte_len as u64,
        };
        self.blob_store.read(&blob_ref)
    }

    fn verify_artifact_integrity(
        &self,
        run_id: &RunId,
        artifact_id: &ArtifactId,
    ) -> Result<BlobIntegrity, StorageError> {
        let artifact = self.get_artifact(run_id, artifact_id)?;
        let blob_ref = BlobRef {
            sha256: artifact.sha256,
            byte_len: artifact.byte_len as u64,
        };
        self.blob_store.verify(&blob_ref)
    }
}

const SQLITE_SCHEMA_VERSION: i32 = 1;

const SQLITE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS id_counters (
    kind TEXT PRIMARY KEY,
    counter INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS run_sequences (
    run_id TEXT PRIMARY KEY,
    next_sequence INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS runs (
    run_id TEXT PRIMARY KEY,
    started_at INTEGER NOT NULL,
    payload_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS spans (
    run_id TEXT NOT NULL,
    span_id TEXT NOT NULL,
    sequence_no INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (run_id, span_id)
);

CREATE INDEX IF NOT EXISTS idx_spans_run_sequence ON spans(run_id, sequence_no);

CREATE TABLE IF NOT EXISTS events (
    run_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    sequence_no INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (run_id, event_id)
);

CREATE INDEX IF NOT EXISTS idx_events_run_sequence ON events(run_id, sequence_no);

CREATE TABLE IF NOT EXISTS artifacts (
    run_id TEXT NOT NULL,
    artifact_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (run_id, artifact_id)
);

CREATE INDEX IF NOT EXISTS idx_artifacts_run_created ON artifacts(run_id, created_at, artifact_id);

CREATE TABLE IF NOT EXISTS snapshots (
    run_id TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (run_id, snapshot_id)
);

CREATE INDEX IF NOT EXISTS idx_snapshots_run_created ON snapshots(run_id, created_at, snapshot_id);

CREATE TABLE IF NOT EXISTS edges (
    run_id TEXT NOT NULL,
    edge_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (run_id, edge_id)
);

CREATE INDEX IF NOT EXISTS idx_edges_run ON edges(run_id);

CREATE TABLE IF NOT EXISTS branches (
    branch_id TEXT PRIMARY KEY,
    payload_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS replay_jobs (
    replay_job_id TEXT PRIMARY KEY,
    payload_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS diffs (
    source_run_id TEXT NOT NULL,
    target_run_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (source_run_id, target_run_id)
);
"#;

const SQLITE_EVENTS_TABLE: &str = r#"
CREATE TABLE events (
    run_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    sequence_no INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (run_id, event_id)
);
"#;

const SQLITE_ARTIFACTS_TABLE: &str = r#"
CREATE TABLE artifacts (
    run_id TEXT NOT NULL,
    artifact_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (run_id, artifact_id)
);
"#;

const SQLITE_SNAPSHOTS_TABLE: &str = r#"
CREATE TABLE snapshots (
    run_id TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (run_id, snapshot_id)
);
"#;

const SQLITE_EDGES_TABLE: &str = r#"
CREATE TABLE edges (
    run_id TEXT NOT NULL,
    edge_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (run_id, edge_id)
);
"#;

#[derive(Clone, Debug)]
pub struct SqliteStorage {
    db_path: Arc<PathBuf>,
    blob_store: Option<LocalBlobStore>,
}

impl SqliteStorage {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let db_path = path.into();
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                StorageError::Internal(format!(
                    "failed to create sqlite storage directory {:?}: {err}",
                    parent
                ))
            })?;
        }

        let storage = Self {
            db_path: Arc::new(db_path),
            blob_store: None,
        };
        let mut conn = Connection::open(storage.db_path.as_ref()).map_err(map_sqlite_error)?;
        initialize_sqlite(&mut conn)?;
        tracing::info!(db_path = %storage.db_path.display(), "opened sqlite storage");
        Ok(storage)
    }

    /// Open storage with a managed blob store rooted at `data_root`.
    /// The SQLite database is placed at `{data_root}/replaykit.db`.
    pub fn open_with_data_root(data_root: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let data_root = data_root.into();
        let db_path = data_root.join("replaykit.db");
        let blob_store = LocalBlobStore::open(&data_root)?;

        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                StorageError::Internal(format!(
                    "failed to create sqlite storage directory {:?}: {err}",
                    parent
                ))
            })?;
        }

        let storage = Self {
            db_path: Arc::new(db_path),
            blob_store: Some(blob_store),
        };
        let mut conn = Connection::open(storage.db_path.as_ref()).map_err(map_sqlite_error)?;
        initialize_sqlite(&mut conn)?;
        tracing::info!(
            db_path = %storage.db_path.display(),
            data_root = %data_root.display(),
            "opened sqlite storage with blob store"
        );
        Ok(storage)
    }

    pub fn db_path(&self) -> &Path {
        self.db_path.as_ref()
    }

    pub fn blob_store(&self) -> Option<&LocalBlobStore> {
        self.blob_store.as_ref()
    }

    fn with_connection<T>(
        &self,
        op: impl FnOnce(&Connection) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        let conn = Connection::open(self.db_path.as_ref()).map_err(map_sqlite_error)?;
        configure_sqlite_connection(&conn)?;
        op(&conn)
    }

    fn with_transaction<T>(
        &self,
        op: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        let mut conn = Connection::open(self.db_path.as_ref()).map_err(map_sqlite_error)?;
        configure_sqlite_connection(&conn)?;
        let tx = conn.transaction().map_err(map_sqlite_error)?;
        let value = op(&tx)?;
        tx.commit().map_err(map_sqlite_error)?;
        Ok(value)
    }

    fn ensure_run_exists_conn(conn: &Connection, run_id: &RunId) -> Result<(), StorageError> {
        let exists = conn
            .query_row(
                "SELECT 1 FROM runs WHERE run_id = ?1",
                params![run_id.0],
                |_| Ok(()),
            )
            .optional()
            .map_err(map_sqlite_error)?;
        exists.ok_or_else(|| StorageError::NotFound(format!("run {:?} not found", run_id.0)))
    }

    fn ensure_span_exists_conn(
        conn: &Connection,
        run_id: &RunId,
        span_id: &SpanId,
    ) -> Result<(), StorageError> {
        let exists = conn
            .query_row(
                "SELECT 1 FROM spans WHERE run_id = ?1 AND span_id = ?2",
                params![run_id.0, span_id.0],
                |_| Ok(()),
            )
            .optional()
            .map_err(map_sqlite_error)?;
        exists.ok_or_else(|| {
            StorageError::NotFound(format!(
                "span {:?} for run {:?} not found",
                span_id.0, run_id.0
            ))
        })
    }

    fn get_artifact_conn(
        conn: &Connection,
        run_id: &RunId,
        artifact_id: &ArtifactId,
    ) -> Result<ArtifactRecord, StorageError> {
        let payload = conn
            .query_row(
                "SELECT payload_json FROM artifacts WHERE run_id = ?1 AND artifact_id = ?2",
                params![run_id.0, artifact_id.0],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite_error)?
            .ok_or_else(|| {
                StorageError::NotFound(format!(
                    "artifact {:?} for run {:?} not found",
                    artifact_id.0, run_id.0
                ))
            })?;
        let artifact = decode_json(&payload)?;
        validate_artifact_record(&artifact)?;
        Ok(artifact)
    }
}

fn configure_sqlite_connection(conn: &Connection) -> Result<(), StorageError> {
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(map_sqlite_error)?;
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;",
    )
    .map_err(map_sqlite_error)?;
    Ok(())
}

fn initialize_sqlite(conn: &mut Connection) -> Result<(), StorageError> {
    configure_sqlite_connection(conn)?;
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(map_sqlite_error)?;
    if version > SQLITE_SCHEMA_VERSION {
        return Err(StorageError::Internal(format!(
            "sqlite schema version {version} is newer than supported version {SQLITE_SCHEMA_VERSION}"
        )));
    }

    let tx = conn.transaction().map_err(map_sqlite_error)?;
    if version == 0 {
        migrate_legacy_run_scoped_tables(&tx)?;
    }
    tx.execute_batch(SQLITE_SCHEMA).map_err(map_sqlite_error)?;
    tx.pragma_update(None, "user_version", SQLITE_SCHEMA_VERSION)
        .map_err(map_sqlite_error)?;
    tx.commit().map_err(map_sqlite_error)?;
    Ok(())
}

fn migrate_legacy_run_scoped_tables(conn: &Connection) -> Result<(), StorageError> {
    migrate_scoped_table(
        conn,
        "events",
        &["run_id", "event_id"],
        SQLITE_EVENTS_TABLE,
        "INSERT INTO events(run_id, event_id, sequence_no, payload_json)
         SELECT run_id, event_id, sequence_no, payload_json FROM __legacy_events",
    )?;
    migrate_scoped_table(
        conn,
        "artifacts",
        &["run_id", "artifact_id"],
        SQLITE_ARTIFACTS_TABLE,
        "INSERT INTO artifacts(run_id, artifact_id, created_at, payload_json)
         SELECT run_id, artifact_id, created_at, payload_json FROM __legacy_artifacts",
    )?;
    migrate_scoped_table(
        conn,
        "snapshots",
        &["run_id", "snapshot_id"],
        SQLITE_SNAPSHOTS_TABLE,
        "INSERT INTO snapshots(run_id, snapshot_id, created_at, payload_json)
         SELECT run_id, snapshot_id, created_at, payload_json FROM __legacy_snapshots",
    )?;
    migrate_scoped_table(
        conn,
        "edges",
        &["run_id", "edge_id"],
        SQLITE_EDGES_TABLE,
        "INSERT INTO edges(run_id, edge_id, payload_json)
         SELECT run_id, edge_id, payload_json FROM __legacy_edges",
    )?;
    Ok(())
}

fn migrate_scoped_table(
    conn: &Connection,
    table_name: &str,
    expected_pk: &[&str],
    create_statement: &str,
    copy_statement: &str,
) -> Result<(), StorageError> {
    if !table_exists(conn, table_name)? {
        return Ok(());
    }
    if table_primary_key_columns(conn, table_name)? == expected_pk {
        return Ok(());
    }

    let legacy_name = format!("__legacy_{table_name}");
    conn.execute_batch(&format!(
        "ALTER TABLE {table_name} RENAME TO {legacy_name};"
    ))
    .map_err(map_sqlite_error)?;
    conn.execute_batch(create_statement)
        .map_err(map_sqlite_error)?;
    conn.execute_batch(copy_statement)
        .map_err(map_sqlite_error)?;
    conn.execute_batch(&format!("DROP TABLE {legacy_name};"))
        .map_err(map_sqlite_error)?;
    Ok(())
}

fn table_exists(conn: &Connection, table_name: &str) -> Result<bool, StorageError> {
    let exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![table_name],
            |_| Ok(()),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    Ok(exists.is_some())
}

fn table_primary_key_columns(
    conn: &Connection,
    table_name: &str,
) -> Result<Vec<String>, StorageError> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table_name})"))
        .map_err(map_sqlite_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
        })
        .map_err(map_sqlite_error)?;
    let mut columns = rows
        .map(|row| row.map_err(map_sqlite_error))
        .collect::<Result<Vec<_>, _>>()?;
    columns.sort_by_key(|(_, pk_position)| *pk_position);
    Ok(columns
        .into_iter()
        .filter(|(_, pk_position)| *pk_position > 0)
        .map(|(name, _)| name)
        .collect())
}

impl Storage for SqliteStorage {
    fn allocate_id(&self, kind: IdKind) -> Result<String, StorageError> {
        let kind_name = kind.prefix().to_owned();
        self.with_transaction(|tx| {
            tx.execute(
                "INSERT INTO id_counters(kind, counter) VALUES (?1, 0)
                 ON CONFLICT(kind) DO NOTHING",
                params![kind_name],
            )
            .map_err(map_sqlite_error)?;
            tx.execute(
                "UPDATE id_counters SET counter = counter + 1 WHERE kind = ?1",
                params![kind_name],
            )
            .map_err(map_sqlite_error)?;
            let counter: u64 = tx
                .query_row(
                    "SELECT counter FROM id_counters WHERE kind = ?1",
                    params![kind_name],
                    |row| row.get(0),
                )
                .map_err(map_sqlite_error)?;
            Ok(format!("{}-{counter:016x}", kind.prefix()))
        })
    }

    fn next_sequence(&self, run_id: &RunId) -> Result<u64, StorageError> {
        self.with_transaction(|tx| {
            let exists = tx
                .query_row(
                    "SELECT 1 FROM run_sequences WHERE run_id = ?1",
                    params![run_id.0],
                    |_| Ok(()),
                )
                .optional()
                .map_err(map_sqlite_error)?;
            if exists.is_none() {
                return Err(StorageError::NotFound(format!(
                    "run {:?} not found",
                    run_id.0
                )));
            }

            tx.execute(
                "UPDATE run_sequences SET next_sequence = next_sequence + 1 WHERE run_id = ?1",
                params![run_id.0],
            )
            .map_err(map_sqlite_error)?;
            tx.query_row(
                "SELECT next_sequence FROM run_sequences WHERE run_id = ?1",
                params![run_id.0],
                |row| row.get(0),
            )
            .map_err(map_sqlite_error)
        })
    }

    fn insert_run(&self, run: RunRecord) -> Result<(), StorageError> {
        let payload = encode_json(&run)?;
        self.with_transaction(|tx| {
            tx.execute(
                "INSERT INTO runs(run_id, started_at, payload_json) VALUES (?1, ?2, ?3)",
                params![run.run_id.0, run.started_at, payload],
            )
            .map_err(map_constraint_or_sqlite_error)?;
            tx.execute(
                "INSERT INTO run_sequences(run_id, next_sequence) VALUES (?1, 0)",
                params![run.run_id.0],
            )
            .map_err(map_constraint_or_sqlite_error)?;
            Ok(())
        })
    }

    fn update_run(&self, run: RunRecord) -> Result<(), StorageError> {
        let payload = encode_json(&run)?;
        self.with_connection(|conn| {
            let updated = conn
                .execute(
                    "UPDATE runs SET started_at = ?2, payload_json = ?3 WHERE run_id = ?1",
                    params![run.run_id.0, run.started_at, payload],
                )
                .map_err(map_sqlite_error)?;
            if updated == 0 {
                return Err(StorageError::NotFound(format!(
                    "run {:?} not found",
                    run.run_id.0
                )));
            }
            Ok(())
        })
    }

    fn get_run(&self, run_id: &RunId) -> Result<RunRecord, StorageError> {
        self.with_connection(|conn| {
            let payload = conn
                .query_row(
                    "SELECT payload_json FROM runs WHERE run_id = ?1",
                    params![run_id.0],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite_error)?
                .ok_or_else(|| StorageError::NotFound(format!("run {:?} not found", run_id.0)))?;
            decode_json(&payload)
        })
    }

    fn list_runs(&self) -> Result<Vec<RunRecord>, StorageError> {
        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT payload_json FROM runs ORDER BY started_at ASC, run_id ASC")
                .map_err(map_sqlite_error)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(map_sqlite_error)?;
            collect_json_rows(rows)
        })
    }

    fn upsert_span(&self, span: SpanRecord) -> Result<(), StorageError> {
        let payload = encode_json(&span)?;
        self.with_connection(|conn| {
            Self::ensure_run_exists_conn(conn, &span.run_id)?;
            if let Some(parent_span_id) = &span.parent_span_id {
                if *parent_span_id == span.span_id {
                    return Err(StorageError::InvalidInput(format!(
                        "span {:?} cannot be its own parent",
                        span.span_id.0
                    )));
                }
                Self::ensure_span_exists_conn(conn, &span.run_id, parent_span_id)?;
            }
            conn.execute(
                "INSERT INTO spans(run_id, span_id, sequence_no, payload_json)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(run_id, span_id) DO UPDATE
                 SET sequence_no = excluded.sequence_no, payload_json = excluded.payload_json",
                params![span.run_id.0, span.span_id.0, span.sequence_no, payload],
            )
            .map_err(map_sqlite_error)?;
            Ok(())
        })
    }

    fn get_span(&self, run_id: &RunId, span_id: &SpanId) -> Result<SpanRecord, StorageError> {
        self.with_connection(|conn| {
            let payload = conn
                .query_row(
                    "SELECT payload_json FROM spans WHERE run_id = ?1 AND span_id = ?2",
                    params![run_id.0, span_id.0],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite_error)?
                .ok_or_else(|| {
                    StorageError::NotFound(format!(
                        "span {:?} for run {:?} not found",
                        span_id.0, run_id.0
                    ))
                })?;
            decode_json(&payload)
        })
    }

    fn list_spans(&self, run_id: &RunId) -> Result<Vec<SpanRecord>, StorageError> {
        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT payload_json FROM spans
                     WHERE run_id = ?1
                     ORDER BY sequence_no ASC, span_id ASC",
                )
                .map_err(map_sqlite_error)?;
            let rows = stmt
                .query_map(params![run_id.0], |row| row.get::<_, String>(0))
                .map_err(map_sqlite_error)?;
            collect_json_rows(rows)
        })
    }

    fn insert_event(&self, event: EventRecord) -> Result<(), StorageError> {
        let payload = encode_json(&event)?;
        self.with_connection(|conn| {
            Self::ensure_run_exists_conn(conn, &event.run_id)?;
            Self::ensure_span_exists_conn(conn, &event.run_id, &event.span_id)?;
            conn.execute(
                "INSERT INTO events(run_id, event_id, sequence_no, payload_json)
                 VALUES (?1, ?2, ?3, ?4)",
                params![event.run_id.0, event.event_id.0, event.sequence_no, payload],
            )
            .map_err(map_constraint_or_sqlite_error)?;
            Ok(())
        })
    }

    fn list_events(&self, run_id: &RunId) -> Result<Vec<EventRecord>, StorageError> {
        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT payload_json FROM events
                     WHERE run_id = ?1
                     ORDER BY sequence_no ASC, event_id ASC",
                )
                .map_err(map_sqlite_error)?;
            let rows = stmt
                .query_map(params![run_id.0], |row| row.get::<_, String>(0))
                .map_err(map_sqlite_error)?;
            collect_json_rows(rows)
        })
    }

    fn insert_artifact(&self, artifact: ArtifactRecord) -> Result<(), StorageError> {
        let payload = encode_json(&artifact)?;
        self.with_connection(|conn| {
            Self::ensure_run_exists_conn(conn, &artifact.run_id)?;
            if let Some(span_id) = &artifact.span_id {
                Self::ensure_span_exists_conn(conn, &artifact.run_id, span_id)?;
            }
            validate_artifact_record(&artifact)?;
            conn.execute(
                "INSERT INTO artifacts(run_id, artifact_id, created_at, payload_json)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    artifact.run_id.0,
                    artifact.artifact_id.0,
                    artifact.created_at,
                    payload
                ],
            )
            .map_err(map_constraint_or_sqlite_error)?;
            Ok(())
        })
    }

    fn get_artifact(
        &self,
        run_id: &RunId,
        artifact_id: &ArtifactId,
    ) -> Result<ArtifactRecord, StorageError> {
        self.with_connection(|conn| {
            let payload = conn
                .query_row(
                    "SELECT payload_json FROM artifacts WHERE run_id = ?1 AND artifact_id = ?2",
                    params![run_id.0, artifact_id.0],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite_error)?
                .ok_or_else(|| {
                    StorageError::NotFound(format!(
                        "artifact {:?} for run {:?} not found",
                        artifact_id.0, run_id.0
                    ))
                })?;
            let artifact = decode_json(&payload)?;
            validate_artifact_record(&artifact)?;
            Ok(artifact)
        })
    }

    fn list_artifacts(&self, run_id: &RunId) -> Result<Vec<ArtifactRecord>, StorageError> {
        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT payload_json FROM artifacts
                     WHERE run_id = ?1
                     ORDER BY created_at ASC, artifact_id ASC",
                )
                .map_err(map_sqlite_error)?;
            let rows = stmt
                .query_map(params![run_id.0], |row| row.get::<_, String>(0))
                .map_err(map_sqlite_error)?;
            let artifacts = collect_json_rows(rows)?;
            for artifact in &artifacts {
                validate_artifact_record(artifact)?;
            }
            Ok(artifacts)
        })
    }

    fn insert_snapshot(&self, snapshot: SnapshotRecord) -> Result<(), StorageError> {
        let payload = encode_json(&snapshot)?;
        self.with_connection(|conn| {
            Self::ensure_run_exists_conn(conn, &snapshot.run_id)?;
            if let Some(span_id) = &snapshot.span_id {
                Self::ensure_span_exists_conn(conn, &snapshot.run_id, span_id)?;
            }
            let artifact = Self::get_artifact_conn(conn, &snapshot.run_id, &snapshot.artifact_id)?;
            if let Some(span_id) = &snapshot.span_id {
                match artifact.span_id {
                    Some(ref artifact_span_id) if artifact_span_id == span_id => {}
                    Some(ref artifact_span_id) => {
                        return Err(StorageError::InvalidInput(format!(
                            "snapshot artifact {:?} belongs to span {:?}, not {:?}",
                            artifact.artifact_id.0, artifact_span_id.0, span_id.0
                        )));
                    }
                    None => {
                        return Err(StorageError::InvalidInput(format!(
                            "snapshot artifact {:?} is not attached to span {:?}",
                            artifact.artifact_id.0, span_id.0
                        )));
                    }
                }
            }
            conn.execute(
                "INSERT INTO snapshots(run_id, snapshot_id, created_at, payload_json)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    snapshot.run_id.0,
                    snapshot.snapshot_id.0,
                    snapshot.created_at,
                    payload
                ],
            )
            .map_err(map_constraint_or_sqlite_error)?;
            Ok(())
        })
    }

    fn get_snapshot(
        &self,
        run_id: &RunId,
        snapshot_id: &SnapshotId,
    ) -> Result<SnapshotRecord, StorageError> {
        self.with_connection(|conn| {
            let payload = conn
                .query_row(
                    "SELECT payload_json FROM snapshots WHERE run_id = ?1 AND snapshot_id = ?2",
                    params![run_id.0, snapshot_id.0],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite_error)?
                .ok_or_else(|| {
                    StorageError::NotFound(format!(
                        "snapshot {:?} for run {:?} not found",
                        snapshot_id.0, run_id.0
                    ))
                })?;
            decode_json(&payload)
        })
    }

    fn list_snapshots(&self, run_id: &RunId) -> Result<Vec<SnapshotRecord>, StorageError> {
        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT payload_json FROM snapshots
                     WHERE run_id = ?1
                     ORDER BY created_at ASC, snapshot_id ASC",
                )
                .map_err(map_sqlite_error)?;
            let rows = stmt
                .query_map(params![run_id.0], |row| row.get::<_, String>(0))
                .map_err(map_sqlite_error)?;
            collect_json_rows(rows)
        })
    }

    fn insert_edge(&self, edge: SpanEdgeRecord) -> Result<(), StorageError> {
        let payload = encode_json(&edge)?;
        self.with_connection(|conn| {
            Self::ensure_run_exists_conn(conn, &edge.run_id)?;
            Self::ensure_span_exists_conn(conn, &edge.run_id, &edge.from_span_id)?;
            Self::ensure_span_exists_conn(conn, &edge.run_id, &edge.to_span_id)?;
            conn.execute(
                "INSERT INTO edges(run_id, edge_id, payload_json)
                 VALUES (?1, ?2, ?3)",
                params![edge.run_id.0, edge.edge_id.0, payload],
            )
            .map_err(map_constraint_or_sqlite_error)?;
            Ok(())
        })
    }

    fn list_edges(&self, run_id: &RunId) -> Result<Vec<SpanEdgeRecord>, StorageError> {
        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT payload_json FROM edges WHERE run_id = ?1 ORDER BY edge_id ASC")
                .map_err(map_sqlite_error)?;
            let rows = stmt
                .query_map(params![run_id.0], |row| row.get::<_, String>(0))
                .map_err(map_sqlite_error)?;
            collect_json_rows(rows)
        })
    }

    fn insert_branch(&self, branch: BranchRecord) -> Result<(), StorageError> {
        let payload = encode_json(&branch)?;
        self.with_connection(|conn| {
            Self::ensure_run_exists_conn(conn, &branch.source_run_id)?;
            Self::ensure_run_exists_conn(conn, &branch.target_run_id)?;
            Self::ensure_span_exists_conn(conn, &branch.source_run_id, &branch.fork_span_id)?;
            Self::get_artifact_conn(
                conn,
                &branch.target_run_id,
                &branch.patch_manifest_artifact_id,
            )?;
            conn.execute(
                "INSERT INTO branches(branch_id, payload_json)
                 VALUES (?1, ?2)
                 ON CONFLICT(branch_id) DO UPDATE SET payload_json = excluded.payload_json",
                params![branch.branch_id.0, payload],
            )
            .map_err(map_sqlite_error)?;
            Ok(())
        })
    }

    fn get_branch(&self, branch_id: &BranchId) -> Result<BranchRecord, StorageError> {
        self.with_connection(|conn| {
            let payload = conn
                .query_row(
                    "SELECT payload_json FROM branches WHERE branch_id = ?1",
                    params![branch_id.0],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite_error)?
                .ok_or_else(|| {
                    StorageError::NotFound(format!("branch {:?} not found", branch_id.0))
                })?;
            decode_json(&payload)
        })
    }

    fn list_branches(&self) -> Result<Vec<BranchRecord>, StorageError> {
        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT payload_json FROM branches ORDER BY branch_id ASC")
                .map_err(map_sqlite_error)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(map_sqlite_error)?;
            collect_json_rows(rows)
        })
    }

    fn insert_replay_job(&self, job: ReplayJobRecord) -> Result<(), StorageError> {
        let payload = encode_json(&job)?;
        self.with_connection(|conn| {
            Self::ensure_run_exists_conn(conn, &job.source_run_id)?;
            if let Some(target_run_id) = &job.target_run_id {
                Self::ensure_run_exists_conn(conn, target_run_id)?;
            }
            conn.execute(
                "INSERT INTO replay_jobs(replay_job_id, payload_json) VALUES (?1, ?2)",
                params![job.replay_job_id.0, payload],
            )
            .map_err(map_constraint_or_sqlite_error)?;
            Ok(())
        })
    }

    fn update_replay_job(&self, job: ReplayJobRecord) -> Result<(), StorageError> {
        let payload = encode_json(&job)?;
        self.with_connection(|conn| {
            Self::ensure_run_exists_conn(conn, &job.source_run_id)?;
            if let Some(target_run_id) = &job.target_run_id {
                Self::ensure_run_exists_conn(conn, target_run_id)?;
            }
            let updated = conn
                .execute(
                    "UPDATE replay_jobs SET payload_json = ?2 WHERE replay_job_id = ?1",
                    params![job.replay_job_id.0, payload],
                )
                .map_err(map_sqlite_error)?;
            if updated == 0 {
                return Err(StorageError::NotFound(format!(
                    "replay job {:?} not found",
                    job.replay_job_id.0
                )));
            }
            Ok(())
        })
    }

    fn get_replay_job(&self, replay_job_id: &ReplayJobId) -> Result<ReplayJobRecord, StorageError> {
        self.with_connection(|conn| {
            let payload = conn
                .query_row(
                    "SELECT payload_json FROM replay_jobs WHERE replay_job_id = ?1",
                    params![replay_job_id.0],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite_error)?
                .ok_or_else(|| {
                    StorageError::NotFound(format!("replay job {:?} not found", replay_job_id.0))
                })?;
            decode_json(&payload)
        })
    }

    fn insert_diff(&self, diff: RunDiffRecord) -> Result<(), StorageError> {
        let payload = encode_json(&diff)?;
        self.with_connection(|conn| {
            Self::ensure_run_exists_conn(conn, &diff.source_run_id)?;
            Self::ensure_run_exists_conn(conn, &diff.target_run_id)?;
            conn.execute(
                "INSERT INTO diffs(source_run_id, target_run_id, payload_json)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(source_run_id, target_run_id) DO UPDATE
                 SET payload_json = excluded.payload_json",
                params![diff.source_run_id.0, diff.target_run_id.0, payload],
            )
            .map_err(map_sqlite_error)?;
            Ok(())
        })
    }

    fn get_diff(&self, source: &RunId, target: &RunId) -> Result<RunDiffRecord, StorageError> {
        self.with_connection(|conn| {
            let payload = conn
                .query_row(
                    "SELECT payload_json FROM diffs WHERE source_run_id = ?1 AND target_run_id = ?2",
                    params![source.0, target.0],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite_error)?
                .ok_or_else(|| {
                    StorageError::NotFound(format!(
                        "diff for runs {:?} -> {:?} not found",
                        source.0, target.0
                    ))
                })?;
            decode_json(&payload)
        })
    }

    fn store_artifact_with_content(
        &self,
        mut artifact: ArtifactRecord,
        content: &[u8],
    ) -> Result<ArtifactRecord, StorageError> {
        let blob_store = self.blob_store.as_ref().ok_or_else(|| {
            StorageError::Internal("cannot store artifact content: no blob store configured".into())
        })?;

        // Store blob first (safe orphan on DB failure).
        let blob_ref = blob_store.store(content)?;
        artifact.sha256 = blob_ref.sha256.clone();
        artifact.byte_len = blob_ref.byte_len as usize;
        artifact.blob_path = blob_store
            .blob_path(&blob_ref)
            .to_string_lossy()
            .to_string();

        self.insert_artifact(artifact.clone())?;
        Ok(artifact)
    }

    fn read_artifact_content(
        &self,
        run_id: &RunId,
        artifact_id: &ArtifactId,
    ) -> Result<Vec<u8>, StorageError> {
        let blob_store = self.blob_store.as_ref().ok_or_else(|| {
            StorageError::Internal("cannot read artifact content: no blob store configured".into())
        })?;
        let artifact = self.get_artifact(run_id, artifact_id)?;
        let blob_ref = BlobRef {
            sha256: artifact.sha256,
            byte_len: artifact.byte_len as u64,
        };
        blob_store.read(&blob_ref)
    }

    fn verify_artifact_integrity(
        &self,
        run_id: &RunId,
        artifact_id: &ArtifactId,
    ) -> Result<BlobIntegrity, StorageError> {
        let blob_store = self.blob_store.as_ref().ok_or_else(|| {
            StorageError::Internal(
                "cannot verify artifact integrity: no blob store configured".into(),
            )
        })?;
        // Fetch artifact metadata without blob-path validation (the blob may
        // be missing/corrupt, which is exactly what we're checking).
        let artifact: ArtifactRecord = self.with_connection(|conn| {
            let payload = conn
                .query_row(
                    "SELECT payload_json FROM artifacts WHERE run_id = ?1 AND artifact_id = ?2",
                    params![run_id.0, artifact_id.0],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite_error)?
                .ok_or_else(|| {
                    StorageError::NotFound(format!(
                        "artifact {:?} for run {:?} not found",
                        artifact_id.0, run_id.0
                    ))
                })?;
            decode_json(&payload)
        })?;
        let blob_ref = BlobRef {
            sha256: artifact.sha256,
            byte_len: artifact.byte_len as u64,
        };
        blob_store.verify(&blob_ref)
    }
}

// ---------------------------------------------------------------------------
// Recovery and integrity scanning (SqliteStorage)
// ---------------------------------------------------------------------------

impl SqliteStorage {
    /// Mark all runs with status == Running as Interrupted.
    /// Returns the list of affected run IDs.
    pub fn recover_interrupted_runs(&self) -> Result<Vec<RunId>, StorageError> {
        self.with_connection(|conn| {
            // Find all runs currently in Running state.
            let mut stmt = conn
                .prepare("SELECT payload_json FROM runs")
                .map_err(map_sqlite_error)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(map_sqlite_error)?;

            let mut affected = Vec::new();
            for row in rows {
                let payload = row.map_err(map_sqlite_error)?;
                let run: RunRecord = decode_json(&payload)?;
                if run.status == RunStatus::Running {
                    affected.push(run.run_id.clone());
                    let mut updated = run;
                    updated.status = RunStatus::Interrupted;
                    let new_payload = encode_json(&updated)?;
                    conn.execute(
                        "UPDATE runs SET payload_json = ?2 WHERE run_id = ?1",
                        params![updated.run_id.0, new_payload],
                    )
                    .map_err(map_sqlite_error)?;
                }
            }

            if !affected.is_empty() {
                tracing::warn!(
                    count = affected.len(),
                    "marked running runs as interrupted during recovery"
                );
            }

            Ok(affected)
        })
    }

    /// Scan all artifacts and verify their blob integrity.
    /// Returns a report for every artifact that is NOT valid.
    pub fn scan_artifact_integrity(&self) -> Result<Vec<ArtifactIntegrityReport>, StorageError> {
        let blob_store = self.blob_store.as_ref().ok_or_else(|| {
            StorageError::Internal(
                "cannot scan artifact integrity: no blob store configured".into(),
            )
        })?;

        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT payload_json FROM artifacts")
                .map_err(map_sqlite_error)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(map_sqlite_error)?;

            let mut reports = Vec::new();
            for row in rows {
                let payload = row.map_err(map_sqlite_error)?;
                let artifact: ArtifactRecord = decode_json(&payload)?;

                // Only verify artifacts whose blob_path is managed by us.
                if artifact.blob_path.starts_with("memory://") || artifact.blob_path.contains("://")
                {
                    continue;
                }

                let blob_ref = BlobRef {
                    sha256: artifact.sha256,
                    byte_len: artifact.byte_len as u64,
                };
                let status = blob_store.verify(&blob_ref)?;
                if status != BlobIntegrity::Valid {
                    tracing::warn!(
                        artifact_id = %artifact.artifact_id.0,
                        run_id = %artifact.run_id.0,
                        status = ?status,
                        "artifact integrity failure"
                    );
                    reports.push(ArtifactIntegrityReport {
                        artifact_id: artifact.artifact_id,
                        run_id: artifact.run_id,
                        status,
                    });
                }
            }
            Ok(reports)
        })
    }
}

fn encode_json<T: Serialize>(value: &T) -> Result<String, StorageError> {
    serde_json::to_string(value).map_err(|err| {
        StorageError::Internal(format!("failed to serialize storage payload: {err}"))
    })
}

fn decode_json<T: DeserializeOwned>(payload: &str) -> Result<T, StorageError> {
    serde_json::from_str(payload).map_err(|err| {
        StorageError::Internal(format!("failed to deserialize storage payload: {err}"))
    })
}

fn collect_json_rows<T: DeserializeOwned>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<String>>,
) -> Result<Vec<T>, StorageError> {
    let mut values = Vec::new();
    for row in rows {
        let payload = row.map_err(map_sqlite_error)?;
        values.push(decode_json(&payload)?);
    }
    Ok(values)
}

fn map_constraint_or_sqlite_error(error: rusqlite::Error) -> StorageError {
    match &error {
        rusqlite::Error::SqliteFailure(code, _)
            if code.extended_code == 1555 || code.extended_code == 2067 =>
        {
            StorageError::Conflict(error.to_string())
        }
        _ => map_sqlite_error(error),
    }
}

fn map_sqlite_error(error: rusqlite::Error) -> StorageError {
    StorageError::Internal(format!("sqlite error: {error}"))
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
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use replaykit_core_model::{
        ArtifactId, ArtifactRecord, ArtifactType, HostMetadata, ReplayPolicy, RunRecord, RunStatus,
        SpanId, SpanKind, SpanRecord, SpanStatus, TraceId,
    };
    use rusqlite::Connection;

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

    #[test]
    fn sqlite_storage_round_trips_and_persists_across_reopen() {
        let db_path = unique_db_path("round-trip");
        let storage = SqliteStorage::open(&db_path).unwrap();
        let run = sample_run();
        storage.insert_run(run.clone()).unwrap();

        let span = SpanRecord {
            run_id: run.run_id.clone(),
            span_id: SpanId("span-sqlite".into()),
            trace_id: run.trace_id.clone(),
            parent_span_id: None,
            sequence_no: storage.next_sequence(&run.run_id).unwrap(),
            kind: SpanKind::Run,
            name: "root".into(),
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
            output_fingerprint: Some("sqlite-span".into()),
            environment_fingerprint: None,
            attributes: BTreeMap::new(),
            error_code: None,
            error_summary: None,
            cost: Default::default(),
        };
        storage.upsert_span(span.clone()).unwrap();

        let reopened = SqliteStorage::open(&db_path).unwrap();
        assert_eq!(reopened.get_run(&run.run_id).unwrap().title, "demo");
        assert_eq!(
            reopened
                .get_span(&run.run_id, &span.span_id)
                .unwrap()
                .output_fingerprint,
            Some("sqlite-span".into())
        );
        assert_eq!(
            reopened.allocate_id(IdKind::Run).unwrap(),
            "run-0000000000000001"
        );
        assert_eq!(
            reopened.allocate_id(IdKind::Run).unwrap(),
            "run-0000000000000002"
        );

        let _ = fs::remove_file(db_path);
    }

    #[test]
    fn sqlite_storage_rejects_artifact_with_missing_span() {
        let db_path = unique_db_path("missing-span");
        let storage = SqliteStorage::open(&db_path).unwrap();
        let run = sample_run();
        storage.insert_run(run.clone()).unwrap();

        let err = storage
            .insert_artifact(ArtifactRecord {
                artifact_id: ArtifactId("artifact-missing-span".into()),
                run_id: run.run_id.clone(),
                span_id: Some(SpanId("missing".into())),
                artifact_type: ArtifactType::ToolOutput,
                mime: "application/json".into(),
                sha256: "missing".into(),
                byte_len: 1,
                blob_path: "memory://missing".into(),
                summary: BTreeMap::new(),
                redaction: BTreeMap::new(),
                created_at: 1,
            })
            .unwrap_err();

        assert!(matches!(
            err,
            StorageError::NotFound(_) | StorageError::InvalidInput(_)
        ));
        let _ = fs::remove_file(db_path);
    }

    #[test]
    fn sqlite_storage_migrates_legacy_run_scoped_tables() {
        let db_path = unique_db_path("legacy-schema");
        let run = sample_run();
        let artifact = ArtifactRecord {
            artifact_id: ArtifactId("artifact-legacy".into()),
            run_id: run.run_id.clone(),
            span_id: None,
            artifact_type: ArtifactType::ToolOutput,
            mime: "application/json".into(),
            sha256: "legacy".into(),
            byte_len: 1,
            blob_path: "memory://legacy".into(),
            summary: BTreeMap::new(),
            redaction: BTreeMap::new(),
            created_at: 2,
        };

        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE runs (
                run_id TEXT PRIMARY KEY,
                started_at INTEGER NOT NULL,
                payload_json TEXT NOT NULL
            );
            CREATE TABLE run_sequences (
                run_id TEXT PRIMARY KEY,
                next_sequence INTEGER NOT NULL
            );
            CREATE TABLE artifacts (
                artifact_id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                payload_json TEXT NOT NULL
            );
            "#,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO runs(run_id, started_at, payload_json) VALUES (?1, ?2, ?3)",
            params![run.run_id.0, run.started_at, encode_json(&run).unwrap()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO run_sequences(run_id, next_sequence) VALUES (?1, 0)",
            params![run.run_id.0],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO artifacts(artifact_id, run_id, created_at, payload_json) VALUES (?1, ?2, ?3, ?4)",
            params![
                artifact.artifact_id.0,
                artifact.run_id.0,
                artifact.created_at,
                encode_json(&artifact).unwrap()
            ],
        )
        .unwrap();
        drop(conn);

        let storage = SqliteStorage::open(&db_path).unwrap();
        assert_eq!(
            storage
                .get_artifact(&run.run_id, &artifact.artifact_id)
                .unwrap()
                .sha256,
            "legacy"
        );

        let conn = Connection::open(&db_path).unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(artifacts)").unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
            })
            .unwrap();
        let mut pk_columns = rows
            .map(|row| row.unwrap())
            .filter(|(_, pk_position)| *pk_position > 0)
            .collect::<Vec<_>>();
        pk_columns.sort_by_key(|(_, pk_position)| *pk_position);
        assert_eq!(
            pk_columns
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            vec!["run_id".to_string(), "artifact_id".to_string()]
        );

        let _ = fs::remove_file(db_path);
    }

    #[test]
    fn in_memory_storage_rejects_missing_local_blob_file() {
        let storage = InMemoryStorage::new();
        let run = sample_run();
        storage.insert_run(run.clone()).unwrap();

        let missing_path = unique_blob_path("missing-local-blob.txt");
        let err = storage
            .insert_artifact(ArtifactRecord {
                artifact_id: ArtifactId("artifact-local-missing".into()),
                run_id: run.run_id.clone(),
                span_id: None,
                artifact_type: ArtifactType::FileBlob,
                mime: "application/octet-stream".into(),
                sha256: "a".repeat(64),
                byte_len: 4,
                blob_path: missing_path.display().to_string(),
                summary: BTreeMap::new(),
                redaction: BTreeMap::new(),
                created_at: 1,
            })
            .unwrap_err();

        assert!(matches!(err, StorageError::InvalidInput(_)));
    }

    #[test]
    fn sqlite_storage_rejects_local_blob_with_wrong_size() {
        let db_path = unique_db_path("local-blob-size");
        let blob_path = unique_blob_path("local-blob-size.bin");
        fs::write(&blob_path, b"blob").unwrap();

        let storage = SqliteStorage::open(&db_path).unwrap();
        let run = sample_run();
        storage.insert_run(run.clone()).unwrap();

        let err = storage
            .insert_artifact(ArtifactRecord {
                artifact_id: ArtifactId("artifact-wrong-size".into()),
                run_id: run.run_id.clone(),
                span_id: None,
                artifact_type: ArtifactType::FileBlob,
                mime: "application/octet-stream".into(),
                sha256: "b".repeat(64),
                byte_len: 99,
                blob_path: blob_path.display().to_string(),
                summary: BTreeMap::new(),
                redaction: BTreeMap::new(),
                created_at: 1,
            })
            .unwrap_err();

        assert!(matches!(err, StorageError::InvalidInput(_)));
        let _ = fs::remove_file(blob_path);
        let _ = fs::remove_file(db_path);
    }

    #[test]
    fn sqlite_storage_detects_deleted_local_blob_on_read() {
        let db_path = unique_db_path("deleted-local-blob");
        let blob_path = unique_blob_path("deleted-local-blob.bin");
        fs::write(&blob_path, b"blob").unwrap();

        let storage = SqliteStorage::open(&db_path).unwrap();
        let run = sample_run();
        storage.insert_run(run.clone()).unwrap();
        let artifact = ArtifactRecord {
            artifact_id: ArtifactId("artifact-local-read".into()),
            run_id: run.run_id.clone(),
            span_id: None,
            artifact_type: ArtifactType::FileBlob,
            mime: "application/octet-stream".into(),
            sha256: "c".repeat(64),
            byte_len: 4,
            blob_path: blob_path.display().to_string(),
            summary: BTreeMap::new(),
            redaction: BTreeMap::new(),
            created_at: 1,
        };
        storage.insert_artifact(artifact.clone()).unwrap();

        fs::remove_file(&blob_path).unwrap();

        let err = storage
            .get_artifact(&run.run_id, &artifact.artifact_id)
            .unwrap_err();
        assert!(matches!(err, StorageError::InvalidInput(_)));

        let _ = fs::remove_file(db_path);
    }

    fn unique_db_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("replaykit-{label}-{nonce}.db"))
    }

    fn unique_blob_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("replaykit-blob-{label}-{nonce}"))
    }

    fn unique_data_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("replaykit-data-{label}-{nonce}"))
    }

    // ---- Phase 3 tests: artifact + blob coherence ----

    #[test]
    fn sqlite_artifact_with_content_round_trip() {
        let data_root = unique_data_root("artifact-content-rt");
        let storage = SqliteStorage::open_with_data_root(&data_root).unwrap();
        let run = sample_run();
        storage.insert_run(run.clone()).unwrap();

        let content = b"hello, managed blob!";
        let artifact = ArtifactRecord {
            artifact_id: ArtifactId("artifact-content-1".into()),
            run_id: run.run_id.clone(),
            span_id: None,
            artifact_type: ArtifactType::ToolOutput,
            mime: "text/plain".into(),
            sha256: String::new(), // will be set by store
            byte_len: 0,           // will be set by store
            blob_path: String::new(),
            summary: BTreeMap::new(),
            redaction: BTreeMap::new(),
            created_at: 1,
        };
        let stored = storage
            .store_artifact_with_content(artifact, content)
            .unwrap();

        assert_eq!(stored.byte_len, content.len());
        assert_eq!(stored.sha256, blob::sha256_hex(content));
        assert!(!stored.blob_path.is_empty());

        // Read content back.
        let read_back = storage
            .read_artifact_content(&run.run_id, &stored.artifact_id)
            .unwrap();
        assert_eq!(read_back, content);

        let _ = fs::remove_dir_all(&data_root);
    }

    #[test]
    fn sqlite_artifact_integrity_valid() {
        let data_root = unique_data_root("integrity-valid");
        let storage = SqliteStorage::open_with_data_root(&data_root).unwrap();
        let run = sample_run();
        storage.insert_run(run.clone()).unwrap();

        let content = b"check my integrity";
        let artifact = ArtifactRecord {
            artifact_id: ArtifactId("artifact-int-1".into()),
            run_id: run.run_id.clone(),
            span_id: None,
            artifact_type: ArtifactType::ToolOutput,
            mime: "text/plain".into(),
            sha256: String::new(),
            byte_len: 0,
            blob_path: String::new(),
            summary: BTreeMap::new(),
            redaction: BTreeMap::new(),
            created_at: 1,
        };
        let stored = storage
            .store_artifact_with_content(artifact, content)
            .unwrap();
        let integrity = storage
            .verify_artifact_integrity(&run.run_id, &stored.artifact_id)
            .unwrap();
        assert_eq!(integrity, BlobIntegrity::Valid);

        let _ = fs::remove_dir_all(&data_root);
    }

    #[test]
    fn sqlite_integrity_error_when_blob_deleted() {
        let data_root = unique_data_root("integrity-deleted");
        let storage = SqliteStorage::open_with_data_root(&data_root).unwrap();
        let run = sample_run();
        storage.insert_run(run.clone()).unwrap();

        let content = b"delete me later";
        let artifact = ArtifactRecord {
            artifact_id: ArtifactId("artifact-del-1".into()),
            run_id: run.run_id.clone(),
            span_id: None,
            artifact_type: ArtifactType::ToolOutput,
            mime: "text/plain".into(),
            sha256: String::new(),
            byte_len: 0,
            blob_path: String::new(),
            summary: BTreeMap::new(),
            redaction: BTreeMap::new(),
            created_at: 1,
        };
        let stored = storage
            .store_artifact_with_content(artifact, content)
            .unwrap();

        // Delete the blob.
        fs::remove_file(&stored.blob_path).unwrap();

        let integrity = storage
            .verify_artifact_integrity(&run.run_id, &stored.artifact_id)
            .unwrap();
        assert_eq!(integrity, BlobIntegrity::Missing);

        let _ = fs::remove_dir_all(&data_root);
    }

    #[test]
    fn sqlite_integrity_error_when_blob_mutated() {
        let data_root = unique_data_root("integrity-mutated");
        let storage = SqliteStorage::open_with_data_root(&data_root).unwrap();
        let run = sample_run();
        storage.insert_run(run.clone()).unwrap();

        let content = b"mutate me later";
        let artifact = ArtifactRecord {
            artifact_id: ArtifactId("artifact-mut-1".into()),
            run_id: run.run_id.clone(),
            span_id: None,
            artifact_type: ArtifactType::ToolOutput,
            mime: "text/plain".into(),
            sha256: String::new(),
            byte_len: 0,
            blob_path: String::new(),
            summary: BTreeMap::new(),
            redaction: BTreeMap::new(),
            created_at: 1,
        };
        let stored = storage
            .store_artifact_with_content(artifact, content)
            .unwrap();

        // Overwrite blob with same size but different content.
        fs::write(&stored.blob_path, b"MUTATE ME LATER").unwrap();

        let integrity = storage
            .verify_artifact_integrity(&run.run_id, &stored.artifact_id)
            .unwrap();
        match integrity {
            BlobIntegrity::HashMismatch { .. } => {} // expected
            other => panic!("expected HashMismatch, got {:?}", other),
        }

        let _ = fs::remove_dir_all(&data_root);
    }

    #[test]
    fn inmemory_artifact_with_content_round_trip() {
        let storage = InMemoryStorage::new();
        let run = sample_run();
        storage.insert_run(run.clone()).unwrap();

        let content = b"in-memory artifact content";
        let artifact = ArtifactRecord {
            artifact_id: ArtifactId("artifact-mem-1".into()),
            run_id: run.run_id.clone(),
            span_id: None,
            artifact_type: ArtifactType::ToolOutput,
            mime: "text/plain".into(),
            sha256: String::new(),
            byte_len: 0,
            blob_path: String::new(),
            summary: BTreeMap::new(),
            redaction: BTreeMap::new(),
            created_at: 1,
        };
        let stored = storage
            .store_artifact_with_content(artifact, content)
            .unwrap();

        let read_back = storage
            .read_artifact_content(&run.run_id, &stored.artifact_id)
            .unwrap();
        assert_eq!(read_back, content);
    }

    #[test]
    fn dedup_two_artifacts_same_content_share_one_blob() {
        let data_root = unique_data_root("dedup-artifact");
        let storage = SqliteStorage::open_with_data_root(&data_root).unwrap();
        let run = sample_run();
        storage.insert_run(run.clone()).unwrap();

        let content = b"shared content";
        let art1 = ArtifactRecord {
            artifact_id: ArtifactId("artifact-dup-1".into()),
            run_id: run.run_id.clone(),
            span_id: None,
            artifact_type: ArtifactType::ToolOutput,
            mime: "text/plain".into(),
            sha256: String::new(),
            byte_len: 0,
            blob_path: String::new(),
            summary: BTreeMap::new(),
            redaction: BTreeMap::new(),
            created_at: 1,
        };
        let art2 = ArtifactRecord {
            artifact_id: ArtifactId("artifact-dup-2".into()),
            run_id: run.run_id.clone(),
            span_id: None,
            artifact_type: ArtifactType::ToolOutput,
            mime: "text/plain".into(),
            sha256: String::new(),
            byte_len: 0,
            blob_path: String::new(),
            summary: BTreeMap::new(),
            redaction: BTreeMap::new(),
            created_at: 2,
        };

        let stored1 = storage.store_artifact_with_content(art1, content).unwrap();
        let stored2 = storage.store_artifact_with_content(art2, content).unwrap();

        // Same blob path (content-addressed dedup).
        assert_eq!(stored1.blob_path, stored2.blob_path);
        assert_eq!(stored1.sha256, stored2.sha256);
        // But different artifact IDs.
        assert_ne!(stored1.artifact_id, stored2.artifact_id);

        let _ = fs::remove_dir_all(&data_root);
    }

    // ---- Phase 5 tests: recovery and integrity scanning ----

    #[test]
    fn recover_marks_running_runs_as_interrupted() {
        let data_root = unique_data_root("recovery");
        let storage = SqliteStorage::open_with_data_root(&data_root).unwrap();

        let mut run1 = sample_run();
        run1.run_id = RunId("run-recovery-1".into());
        run1.status = RunStatus::Running;
        storage.insert_run(run1.clone()).unwrap();

        let mut run2 = sample_run();
        run2.run_id = RunId("run-recovery-2".into());
        run2.status = RunStatus::Completed;
        storage.insert_run(run2.clone()).unwrap();

        let affected = storage.recover_interrupted_runs().unwrap();
        assert_eq!(affected.len(), 1);
        assert_eq!(affected[0], run1.run_id);

        let recovered = storage.get_run(&run1.run_id).unwrap();
        assert_eq!(recovered.status, RunStatus::Interrupted);

        // Run2 remains completed.
        let unchanged = storage.get_run(&run2.run_id).unwrap();
        assert_eq!(unchanged.status, RunStatus::Completed);

        let _ = fs::remove_dir_all(&data_root);
    }

    #[test]
    fn scan_integrity_surfaces_missing_blob() {
        let data_root = unique_data_root("scan-missing");
        let storage = SqliteStorage::open_with_data_root(&data_root).unwrap();
        let run = sample_run();
        storage.insert_run(run.clone()).unwrap();

        let content = b"scan test blob";
        let artifact = ArtifactRecord {
            artifact_id: ArtifactId("artifact-scan-1".into()),
            run_id: run.run_id.clone(),
            span_id: None,
            artifact_type: ArtifactType::ToolOutput,
            mime: "text/plain".into(),
            sha256: String::new(),
            byte_len: 0,
            blob_path: String::new(),
            summary: BTreeMap::new(),
            redaction: BTreeMap::new(),
            created_at: 1,
        };
        let stored = storage
            .store_artifact_with_content(artifact, content)
            .unwrap();

        // Delete the blob file.
        fs::remove_file(&stored.blob_path).unwrap();

        let reports = storage.scan_artifact_integrity().unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].artifact_id, stored.artifact_id);
        assert_eq!(reports[0].status, BlobIntegrity::Missing);

        let _ = fs::remove_dir_all(&data_root);
    }

    #[test]
    fn scan_integrity_surfaces_hash_mismatch() {
        let data_root = unique_data_root("scan-hash");
        let storage = SqliteStorage::open_with_data_root(&data_root).unwrap();
        let run = sample_run();
        storage.insert_run(run.clone()).unwrap();

        let content = b"hash scan test";
        let artifact = ArtifactRecord {
            artifact_id: ArtifactId("artifact-scan-h1".into()),
            run_id: run.run_id.clone(),
            span_id: None,
            artifact_type: ArtifactType::ToolOutput,
            mime: "text/plain".into(),
            sha256: String::new(),
            byte_len: 0,
            blob_path: String::new(),
            summary: BTreeMap::new(),
            redaction: BTreeMap::new(),
            created_at: 1,
        };
        let stored = storage
            .store_artifact_with_content(artifact, content)
            .unwrap();

        // Corrupt blob with same-length content.
        fs::write(&stored.blob_path, b"HASH SCAN TEST").unwrap();

        let reports = storage.scan_artifact_integrity().unwrap();
        assert_eq!(reports.len(), 1);
        match &reports[0].status {
            BlobIntegrity::HashMismatch { .. } => {} // expected
            other => panic!("expected HashMismatch, got {:?}", other),
        }

        let _ = fs::remove_dir_all(&data_root);
    }
}
