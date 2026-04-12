use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use replaykit_core_model::{
    ArtifactId, ArtifactRecord, ArtifactType, BranchRequest, CostMetrics, Document, EdgeId,
    EdgeKind, EventId, EventRecord, HostMetadata, PatchType, ReplayPolicy, RunId, RunRecord,
    RunStatus, SnapshotId, SnapshotRecord, SpanEdgeRecord, SpanId, SpanKind, SpanRecord,
    SpanStatus, TraceId, Value,
};
use replaykit_storage::{Storage, StorageError};

#[derive(Debug)]
pub enum CollectorError {
    Storage(StorageError),
    InvalidInput(String),
}

impl fmt::Display for CollectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CollectorError::Storage(err) => write!(f, "{err}"),
            CollectorError::InvalidInput(message) => write!(f, "{message}"),
        }
    }
}

impl From<StorageError> for CollectorError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

#[derive(Clone, Debug)]
pub struct BeginRun {
    pub title: String,
    pub entrypoint: String,
    pub adapter_name: String,
    pub adapter_version: String,
    pub started_at: u64,
    pub git_sha: Option<String>,
    pub environment_fingerprint: Option<String>,
    pub host: HostMetadata,
    pub labels: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SpanSpec {
    pub span_id: Option<SpanId>,
    pub parent_span_id: Option<SpanId>,
    pub kind: SpanKind,
    pub name: String,
    pub started_at: u64,
    pub replay_policy: ReplayPolicy,
    pub executor_kind: Option<String>,
    pub executor_version: Option<String>,
    pub input_artifact_ids: Vec<ArtifactId>,
    pub input_fingerprint: Option<String>,
    pub environment_fingerprint: Option<String>,
    pub attributes: Document,
}

#[derive(Clone, Debug)]
pub struct EndSpan {
    pub ended_at: u64,
    pub status: SpanStatus,
    pub output_artifact_ids: Vec<ArtifactId>,
    pub snapshot_id: Option<SnapshotId>,
    pub output_fingerprint: Option<String>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    pub cost: CostMetrics,
}

#[derive(Clone, Debug)]
pub struct EventSpec {
    pub timestamp: u64,
    pub kind: String,
    pub payload: Document,
}

#[derive(Clone, Debug)]
pub struct ArtifactSpec {
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
pub struct SnapshotSpec {
    pub kind: String,
    pub artifact_id: ArtifactId,
    pub summary: Document,
    pub created_at: u64,
}

#[derive(Clone, Debug)]
pub struct EdgeSpec {
    pub from_span_id: SpanId,
    pub to_span_id: SpanId,
    pub kind: EdgeKind,
    pub attributes: Document,
}

pub struct Collector<S: Storage> {
    storage: Arc<S>,
    ids: AtomicU64,
}

impl<S: Storage> Collector<S> {
    pub fn new(storage: Arc<S>) -> Self {
        Self {
            storage,
            ids: AtomicU64::new(1),
        }
    }

    pub fn storage(&self) -> &Arc<S> {
        &self.storage
    }

    pub fn begin_run(&self, request: BeginRun) -> Result<RunRecord, CollectorError> {
        let run_id = RunId(self.next_id("run"));
        let trace_id = TraceId(self.next_id("trace"));
        let mut run = RunRecord::new(
            run_id.clone(),
            trace_id,
            request.title,
            request.entrypoint,
            request.adapter_name,
            request.adapter_version,
            request.started_at,
        );
        run.git_sha = request.git_sha;
        run.environment_fingerprint = request.environment_fingerprint;
        run.host = request.host;
        run.labels = request.labels;
        self.storage.insert_run(run.clone())?;
        Ok(run)
    }

    pub fn start_span(
        &self,
        run_id: &RunId,
        trace_id: &TraceId,
        spec: SpanSpec,
    ) -> Result<SpanRecord, CollectorError> {
        let sequence_no = self.storage.next_sequence(run_id)?;
        let span_id = spec
            .span_id
            .unwrap_or_else(|| SpanId(self.next_id("span")));
        let span = SpanRecord {
            run_id: run_id.clone(),
            span_id,
            trace_id: trace_id.clone(),
            parent_span_id: spec.parent_span_id,
            sequence_no,
            kind: spec.kind,
            name: spec.name,
            status: SpanStatus::Running,
            started_at: spec.started_at,
            ended_at: None,
            replay_policy: spec.replay_policy,
            executor_kind: spec.executor_kind,
            executor_version: spec.executor_version,
            input_artifact_ids: spec.input_artifact_ids,
            output_artifact_ids: Vec::new(),
            snapshot_id: None,
            input_fingerprint: spec.input_fingerprint,
            output_fingerprint: None,
            environment_fingerprint: spec.environment_fingerprint,
            attributes: spec.attributes,
            error_code: None,
            error_summary: None,
            cost: CostMetrics::default(),
        };
        self.storage.upsert_span(span.clone())?;
        Ok(span)
    }

    pub fn end_span(
        &self,
        run_id: &RunId,
        span_id: &SpanId,
        update: EndSpan,
    ) -> Result<SpanRecord, CollectorError> {
        let mut span = self.storage.get_span(run_id, span_id)?;
        span.ended_at = Some(update.ended_at);
        span.status = update.status;
        span.output_artifact_ids = update.output_artifact_ids;
        span.snapshot_id = update.snapshot_id;
        span.output_fingerprint = update.output_fingerprint;
        span.error_code = update.error_code;
        span.error_summary = update.error_summary;
        span.cost = update.cost;
        self.storage.upsert_span(span.clone())?;
        Ok(span)
    }

    pub fn add_event(
        &self,
        run_id: &RunId,
        span_id: &SpanId,
        spec: EventSpec,
    ) -> Result<EventRecord, CollectorError> {
        let event = EventRecord {
            event_id: EventId(self.next_id("event")),
            run_id: run_id.clone(),
            span_id: span_id.clone(),
            sequence_no: self.storage.next_sequence(run_id)?,
            timestamp: spec.timestamp,
            kind: spec.kind,
            payload: spec.payload,
        };
        self.storage.insert_event(event.clone())?;
        Ok(event)
    }

    pub fn add_artifact(
        &self,
        run_id: &RunId,
        span_id: Option<&SpanId>,
        spec: ArtifactSpec,
    ) -> Result<ArtifactRecord, CollectorError> {
        let artifact = ArtifactRecord {
            artifact_id: ArtifactId(self.next_id("artifact")),
            run_id: run_id.clone(),
            span_id: span_id.cloned(),
            artifact_type: spec.artifact_type,
            mime: spec.mime,
            sha256: spec.sha256,
            byte_len: spec.byte_len,
            blob_path: spec.blob_path,
            summary: spec.summary,
            redaction: spec.redaction,
            created_at: spec.created_at,
        };
        self.storage.insert_artifact(artifact.clone())?;
        Ok(artifact)
    }

    pub fn add_snapshot(
        &self,
        run_id: &RunId,
        span_id: Option<&SpanId>,
        spec: SnapshotSpec,
    ) -> Result<SnapshotRecord, CollectorError> {
        let snapshot = SnapshotRecord {
            snapshot_id: SnapshotId(self.next_id("snapshot")),
            run_id: run_id.clone(),
            span_id: span_id.cloned(),
            kind: spec.kind,
            artifact_id: spec.artifact_id,
            summary: spec.summary,
            created_at: spec.created_at,
        };
        self.storage.insert_snapshot(snapshot.clone())?;
        Ok(snapshot)
    }

    pub fn add_edge(&self, run_id: &RunId, spec: EdgeSpec) -> Result<SpanEdgeRecord, CollectorError> {
        let edge = SpanEdgeRecord {
            edge_id: EdgeId(self.next_id("edge")),
            run_id: run_id.clone(),
            from_span_id: spec.from_span_id,
            to_span_id: spec.to_span_id,
            kind: spec.kind,
            attributes: spec.attributes,
        };
        self.storage.insert_edge(edge.clone())?;
        Ok(edge)
    }

    pub fn finish_run(
        &self,
        run_id: &RunId,
        ended_at: u64,
        status: RunStatus,
        final_output_preview: Option<String>,
    ) -> Result<RunRecord, CollectorError> {
        let mut run = self.storage.get_run(run_id)?;
        run.ended_at = Some(ended_at);
        run.status = status;
        run.summary.span_count = self.storage.list_spans(run_id)?.len() as u64;
        run.summary.artifact_count = self.storage.list_artifacts(run_id)?.len() as u64;
        run.summary.final_output_preview = final_output_preview;
        self.storage.update_run(run.clone())?;
        Ok(run)
    }

    pub fn abort_run(
        &self,
        run_id: &RunId,
        ended_at: u64,
        error: impl Into<String>,
    ) -> Result<RunRecord, CollectorError> {
        let mut run = self.storage.get_run(run_id)?;
        run.ended_at = Some(ended_at);
        run.status = RunStatus::Interrupted;
        run.summary.error_count += 1;
        run.summary.failure_class = Some(replaykit_core_model::FailureClass::Unknown);
        run.summary.final_output_preview = Some(error.into());
        self.storage.update_run(run.clone())?;
        Ok(run)
    }

    pub fn encode_patch_manifest(request: &BranchRequest) -> Document {
        let mut body = BTreeMap::new();
        body.insert(
            "patch_type".into(),
            Value::Text(format!("{:?}", request.patch_manifest.patch_type)),
        );
        body.insert(
            "replacement".into(),
            request.patch_manifest.replacement.clone(),
        );
        if let Some(note) = &request.patch_manifest.note {
            body.insert("note".into(), Value::Text(note.clone()));
        }
        if let Some(target_artifact_id) = &request.patch_manifest.target_artifact_id {
            body.insert(
                "target_artifact_id".into(),
                Value::Text(target_artifact_id.0.clone()),
            );
        }
        body.insert(
            "fork_span_id".into(),
            Value::Text(request.fork_span_id.0.clone()),
        );
        body.insert(
            "source_run_id".into(),
            Value::Text(request.source_run_id.0.clone()),
        );
        body
    }

    pub fn patch_manifest_artifact(
        &self,
        run_id: &RunId,
        request: &BranchRequest,
    ) -> Result<ArtifactRecord, CollectorError> {
        let summary = Self::encode_patch_manifest(request);
        let sha256 = format!("patch-{}", self.ids.load(Ordering::SeqCst));
        self.add_artifact(
            run_id,
            Some(&request.fork_span_id),
            ArtifactSpec {
                artifact_type: ArtifactType::PatchManifest,
                mime: "application/replaykit-patch".into(),
                sha256,
                byte_len: summary.len(),
                blob_path: format!("memory://patch/{}", request.fork_span_id.0),
                summary,
                redaction: Document::new(),
                created_at: request.patch_manifest.created_at,
            },
        )
    }

    fn next_id(&self, prefix: &str) -> String {
        let value = self.ids.fetch_add(1, Ordering::SeqCst);
        format!("{prefix}-{value:016x}")
    }
}

pub fn patch_type_label(value: PatchType) -> &'static str {
    match value {
        PatchType::PromptEdit => "prompt-edit",
        PatchType::ToolOutputOverride => "tool-output-override",
        PatchType::EnvVarOverride => "env-var-override",
        PatchType::ModelConfigEdit => "model-config-edit",
        PatchType::RetrievalContextOverride => "retrieval-context-override",
        PatchType::SnapshotOverride => "snapshot-override",
    }
}
