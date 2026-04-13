use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use replaykit_core_model::{
    ArtifactId, ArtifactRecord, ArtifactType, BranchRequest, CostMetrics, Document, EdgeId,
    EdgeKind, EventId, EventRecord, HostMetadata, IdKind, PatchType, ReplayPolicy, RunId,
    RunRecord, RunStatus, RunSummary, SnapshotId, SnapshotRecord, SpanEdgeRecord, SpanId, SpanKind,
    SpanRecord, SpanStatus, TraceId, Value,
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
}

impl<S: Storage> Collector<S> {
    pub fn new(storage: Arc<S>) -> Self {
        Self { storage }
    }

    pub fn storage(&self) -> &Arc<S> {
        &self.storage
    }

    pub fn begin_run(&self, request: BeginRun) -> Result<RunRecord, CollectorError> {
        let run_id = RunId(self.allocate_id(IdKind::Run)?);
        let trace_id = TraceId(self.allocate_id(IdKind::Trace)?);
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
        self.ensure_run_exists(run_id)?;
        let sequence_no = self.storage.next_sequence(run_id)?;
        if let Some(parent_span_id) = &spec.parent_span_id {
            self.ensure_span_exists(run_id, parent_span_id)?;
        }
        for artifact_id in &spec.input_artifact_ids {
            self.lookup_artifact(
                run_id,
                artifact_id,
                format!(
                    "input artifact {:?} was not found in run {:?}",
                    artifact_id.0, run_id.0
                ),
            )?;
        }
        let span_id = match spec.span_id {
            Some(span_id) => {
                if self.storage.get_span(run_id, &span_id).is_ok() {
                    return Err(CollectorError::InvalidInput(format!(
                        "span {:?} already exists in run {:?}",
                        span_id.0, run_id.0
                    )));
                }
                span_id
            }
            None => SpanId(self.allocate_id(IdKind::Span)?),
        };
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
        if span.parent_span_id.is_none() {
            let mut run = self.storage.get_run(run_id)?;
            if run.root_span_id.is_none() {
                run.root_span_id = Some(span.span_id.clone());
                self.storage.update_run(run)?;
            }
        }
        Ok(span)
    }

    pub fn end_span(
        &self,
        run_id: &RunId,
        span_id: &SpanId,
        update: EndSpan,
    ) -> Result<SpanRecord, CollectorError> {
        let mut span = self.storage.get_span(run_id, span_id)?;
        if span.is_terminal() {
            return Err(CollectorError::InvalidInput(format!(
                "span {:?} in run {:?} has already ended",
                span_id.0, run_id.0
            )));
        }
        if update.ended_at < span.started_at {
            return Err(CollectorError::InvalidInput(format!(
                "span {:?} in run {:?} cannot end before it started",
                span_id.0, run_id.0
            )));
        }
        for artifact_id in &update.output_artifact_ids {
            let artifact = self.lookup_artifact(
                run_id,
                artifact_id,
                format!(
                    "output artifact {:?} was not found in run {:?}",
                    artifact_id.0, run_id.0
                ),
            )?;
            self.ensure_artifact_attached_to_span(&artifact, span_id, "output artifact")?;
        }
        if let Some(snapshot_id) = &update.snapshot_id {
            let snapshot = self.lookup_snapshot(
                run_id,
                snapshot_id,
                format!(
                    "snapshot {:?} was not found in run {:?}",
                    snapshot_id.0, run_id.0
                ),
            )?;
            self.ensure_snapshot_attached_to_span(&snapshot, span_id)?;
        }
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
        self.ensure_span_exists(run_id, span_id)?;
        let event = EventRecord {
            event_id: EventId(self.allocate_id(IdKind::Event)?),
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
        self.ensure_run_exists(run_id)?;
        if let Some(span_id) = span_id {
            self.ensure_span_exists(run_id, span_id)?;
        }
        let artifact = ArtifactRecord {
            artifact_id: ArtifactId(self.allocate_id(IdKind::Artifact)?),
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
        self.ensure_run_exists(run_id)?;
        let artifact = self.lookup_artifact(
            run_id,
            &spec.artifact_id,
            format!(
                "snapshot artifact {:?} was not found in run {:?}",
                spec.artifact_id.0, run_id.0
            ),
        )?;
        if let Some(span_id) = span_id {
            self.ensure_span_exists(run_id, span_id)?;
            self.ensure_artifact_attached_to_span(&artifact, span_id, "snapshot artifact")?;
        }
        let snapshot = SnapshotRecord {
            snapshot_id: SnapshotId(self.allocate_id(IdKind::Snapshot)?),
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

    pub fn add_edge(
        &self,
        run_id: &RunId,
        spec: EdgeSpec,
    ) -> Result<SpanEdgeRecord, CollectorError> {
        self.ensure_run_exists(run_id)?;
        self.ensure_span_exists(run_id, &spec.from_span_id)?;
        self.ensure_span_exists(run_id, &spec.to_span_id)?;
        let edge = SpanEdgeRecord {
            edge_id: EdgeId(self.allocate_id(IdKind::Edge)?),
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
        if ended_at < run.started_at {
            return Err(CollectorError::InvalidInput(format!(
                "run {:?} cannot end before it started",
                run_id.0
            )));
        }
        let spans = self.storage.list_spans(run_id)?;
        if spans.iter().any(|span| !span.is_terminal()) {
            return Err(CollectorError::InvalidInput(format!(
                "run {:?} cannot finish while spans are still running",
                run_id.0
            )));
        }
        let artifacts = self.storage.list_artifacts(run_id)?;
        let preview = final_output_preview.or_else(|| run.summary.final_output_preview.clone());
        run.ended_at = Some(ended_at);
        run.status = status;
        run.summary = RunSummary::from_run_state(status, &spans, &artifacts, preview);
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
        if ended_at < run.started_at {
            return Err(CollectorError::InvalidInput(format!(
                "run {:?} cannot end before it started",
                run_id.0
            )));
        }
        let spans = self.storage.list_spans(run_id)?;
        let artifacts = self.storage.list_artifacts(run_id)?;
        let error = error.into();
        run.ended_at = Some(ended_at);
        run.status = RunStatus::Interrupted;
        run.summary = RunSummary::from_run_state(run.status, &spans, &artifacts, Some(error));
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
        let sha256 = format!(
            "patch:{}:{}:{}",
            request.source_run_id.0, request.fork_span_id.0, request.patch_manifest.created_at
        );
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

    fn allocate_id(&self, kind: IdKind) -> Result<String, CollectorError> {
        self.storage.allocate_id(kind).map_err(Into::into)
    }

    fn ensure_run_exists(&self, run_id: &RunId) -> Result<(), CollectorError> {
        self.storage.get_run(run_id).map(|_| ()).map_err(|err| {
            self.not_found_as_invalid_input(err, format!("run {:?} was not found", run_id.0))
        })
    }

    fn ensure_span_exists(&self, run_id: &RunId, span_id: &SpanId) -> Result<(), CollectorError> {
        self.storage
            .get_span(run_id, span_id)
            .map(|_| ())
            .map_err(|err| {
                self.not_found_as_invalid_input(
                    err,
                    format!("span {:?} was not found in run {:?}", span_id.0, run_id.0),
                )
            })
    }

    fn ensure_artifact_attached_to_span(
        &self,
        artifact: &ArtifactRecord,
        span_id: &SpanId,
        label: &str,
    ) -> Result<(), CollectorError> {
        match &artifact.span_id {
            Some(existing_span_id) if existing_span_id == span_id => Ok(()),
            Some(existing_span_id) => Err(CollectorError::InvalidInput(format!(
                "{label} {:?} belongs to span {:?}, not {:?}",
                artifact.artifact_id.0, existing_span_id.0, span_id.0
            ))),
            None => Err(CollectorError::InvalidInput(format!(
                "{label} {:?} is not attached to span {:?}",
                artifact.artifact_id.0, span_id.0
            ))),
        }
    }

    fn ensure_snapshot_attached_to_span(
        &self,
        snapshot: &SnapshotRecord,
        span_id: &SpanId,
    ) -> Result<(), CollectorError> {
        match &snapshot.span_id {
            Some(existing_span_id) if existing_span_id == span_id => Ok(()),
            Some(existing_span_id) => Err(CollectorError::InvalidInput(format!(
                "snapshot {:?} belongs to span {:?}, not {:?}",
                snapshot.snapshot_id.0, existing_span_id.0, span_id.0
            ))),
            None => Err(CollectorError::InvalidInput(format!(
                "snapshot {:?} is not attached to span {:?}",
                snapshot.snapshot_id.0, span_id.0
            ))),
        }
    }

    fn lookup_artifact(
        &self,
        run_id: &RunId,
        artifact_id: &ArtifactId,
        not_found_message: String,
    ) -> Result<ArtifactRecord, CollectorError> {
        self.storage
            .get_artifact(run_id, artifact_id)
            .map_err(|err| self.not_found_as_invalid_input(err, not_found_message))
    }

    fn lookup_snapshot(
        &self,
        run_id: &RunId,
        snapshot_id: &SnapshotId,
        not_found_message: String,
    ) -> Result<SnapshotRecord, CollectorError> {
        self.storage
            .get_snapshot(run_id, snapshot_id)
            .map_err(|err| self.not_found_as_invalid_input(err, not_found_message))
    }

    fn not_found_as_invalid_input(&self, error: StorageError, message: String) -> CollectorError {
        match error {
            StorageError::NotFound(_) => CollectorError::InvalidInput(message),
            other => CollectorError::Storage(other),
        }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use replaykit_core_model::{
        ArtifactId, ArtifactType, FailureClass, HostMetadata, ReplayPolicy, RunStatus, SpanKind,
        SpanStatus,
    };
    use replaykit_storage::InMemoryStorage;

    use super::*;

    fn sample_begin_run() -> BeginRun {
        BeginRun {
            title: "demo".into(),
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

    #[test]
    fn rejects_missing_parent_span() {
        let storage = Arc::new(InMemoryStorage::new());
        let collector = Collector::new(storage);
        let run = collector.begin_run(sample_begin_run()).unwrap();

        let err = collector
            .start_span(
                &run.run_id,
                &run.trace_id,
                SpanSpec {
                    span_id: Some(SpanId("child".into())),
                    parent_span_id: Some(SpanId("missing".into())),
                    kind: SpanKind::ToolCall,
                    name: "tool".into(),
                    started_at: 2,
                    replay_policy: ReplayPolicy::RerunnableSupported,
                    executor_kind: None,
                    executor_version: None,
                    input_artifact_ids: Vec::new(),
                    input_fingerprint: None,
                    environment_fingerprint: None,
                    attributes: Document::new(),
                },
            )
            .unwrap_err();

        assert!(matches!(err, CollectorError::InvalidInput(_)));
    }

    #[test]
    fn rejects_end_span_with_output_artifact_from_other_span() {
        let storage = Arc::new(InMemoryStorage::new());
        let collector = Collector::new(storage);
        let run = collector.begin_run(sample_begin_run()).unwrap();

        let one = collector
            .start_span(
                &run.run_id,
                &run.trace_id,
                SpanSpec {
                    span_id: Some(SpanId("one".into())),
                    parent_span_id: None,
                    kind: SpanKind::ToolCall,
                    name: "one".into(),
                    started_at: 2,
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
        let two = collector
            .start_span(
                &run.run_id,
                &run.trace_id,
                SpanSpec {
                    span_id: Some(SpanId("two".into())),
                    parent_span_id: None,
                    kind: SpanKind::ToolCall,
                    name: "two".into(),
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

        let artifact = collector
            .add_artifact(
                &run.run_id,
                Some(&one.span_id),
                ArtifactSpec {
                    artifact_type: ArtifactType::ToolOutput,
                    mime: "application/json".into(),
                    sha256: "abc".into(),
                    byte_len: 1,
                    blob_path: "memory://artifact".into(),
                    summary: Document::new(),
                    redaction: Document::new(),
                    created_at: 4,
                },
            )
            .unwrap();

        let err = collector
            .end_span(
                &run.run_id,
                &two.span_id,
                EndSpan {
                    ended_at: 5,
                    status: SpanStatus::Completed,
                    output_artifact_ids: vec![artifact.artifact_id],
                    snapshot_id: None,
                    output_fingerprint: Some("v2".into()),
                    error_code: None,
                    error_summary: None,
                    cost: CostMetrics::default(),
                },
            )
            .unwrap_err();

        assert!(matches!(err, CollectorError::InvalidInput(_)));
    }

    #[test]
    fn rejects_missing_input_artifact_on_start_span() {
        let storage = Arc::new(InMemoryStorage::new());
        let collector = Collector::new(storage);
        let run = collector.begin_run(sample_begin_run()).unwrap();

        let err = collector
            .start_span(
                &run.run_id,
                &run.trace_id,
                SpanSpec {
                    span_id: Some(SpanId("tool".into())),
                    parent_span_id: None,
                    kind: SpanKind::ToolCall,
                    name: "tool".into(),
                    started_at: 2,
                    replay_policy: ReplayPolicy::RerunnableSupported,
                    executor_kind: None,
                    executor_version: None,
                    input_artifact_ids: vec![ArtifactId("missing".into())],
                    input_fingerprint: None,
                    environment_fingerprint: None,
                    attributes: Document::new(),
                },
            )
            .unwrap_err();

        assert!(matches!(err, CollectorError::InvalidInput(_)));
    }

    #[test]
    fn rejects_ending_span_twice() {
        let storage = Arc::new(InMemoryStorage::new());
        let collector = Collector::new(storage);
        let run = collector.begin_run(sample_begin_run()).unwrap();
        let span = collector
            .start_span(
                &run.run_id,
                &run.trace_id,
                SpanSpec {
                    span_id: Some(SpanId("tool".into())),
                    parent_span_id: None,
                    kind: SpanKind::ToolCall,
                    name: "tool".into(),
                    started_at: 2,
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

        collector
            .end_span(
                &run.run_id,
                &span.span_id,
                EndSpan {
                    ended_at: 3,
                    status: SpanStatus::Completed,
                    output_artifact_ids: Vec::new(),
                    snapshot_id: None,
                    output_fingerprint: Some("done".into()),
                    error_code: None,
                    error_summary: None,
                    cost: CostMetrics::default(),
                },
            )
            .unwrap();

        let err = collector
            .end_span(
                &run.run_id,
                &span.span_id,
                EndSpan {
                    ended_at: 4,
                    status: SpanStatus::Completed,
                    output_artifact_ids: Vec::new(),
                    snapshot_id: None,
                    output_fingerprint: Some("done-again".into()),
                    error_code: None,
                    error_summary: None,
                    cost: CostMetrics::default(),
                },
            )
            .unwrap_err();

        assert!(matches!(err, CollectorError::InvalidInput(_)));
    }

    #[test]
    fn rejects_finishing_run_with_running_spans() {
        let storage = Arc::new(InMemoryStorage::new());
        let collector = Collector::new(storage);
        let run = collector.begin_run(sample_begin_run()).unwrap();
        collector
            .start_span(
                &run.run_id,
                &run.trace_id,
                SpanSpec {
                    span_id: Some(SpanId("tool".into())),
                    parent_span_id: None,
                    kind: SpanKind::ToolCall,
                    name: "tool".into(),
                    started_at: 2,
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

        let err = collector
            .finish_run(&run.run_id, 3, RunStatus::Completed, None)
            .unwrap_err();

        assert!(matches!(err, CollectorError::InvalidInput(_)));
    }

    #[test]
    fn finish_run_recomputes_summary_from_persisted_state() {
        let storage = Arc::new(InMemoryStorage::new());
        let collector = Collector::new(storage);
        let run = collector.begin_run(sample_begin_run()).unwrap();

        let completed = collector
            .start_span(
                &run.run_id,
                &run.trace_id,
                SpanSpec {
                    span_id: Some(SpanId("planner".into())),
                    parent_span_id: None,
                    kind: SpanKind::PlannerStep,
                    name: "planner".into(),
                    started_at: 2,
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
        collector
            .end_span(
                &run.run_id,
                &completed.span_id,
                EndSpan {
                    ended_at: 3,
                    status: SpanStatus::Completed,
                    output_artifact_ids: Vec::new(),
                    snapshot_id: None,
                    output_fingerprint: Some("planner-out".into()),
                    error_code: None,
                    error_summary: None,
                    cost: CostMetrics {
                        input_tokens: 3,
                        output_tokens: 5,
                        estimated_cost_micros: 7,
                    },
                },
            )
            .unwrap();

        let failed = collector
            .start_span(
                &run.run_id,
                &run.trace_id,
                SpanSpec {
                    span_id: Some(SpanId("tool".into())),
                    parent_span_id: Some(completed.span_id.clone()),
                    kind: SpanKind::ToolCall,
                    name: "tool".into(),
                    started_at: 4,
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
        collector
            .end_span(
                &run.run_id,
                &failed.span_id,
                EndSpan {
                    ended_at: 5,
                    status: SpanStatus::Failed,
                    output_artifact_ids: Vec::new(),
                    snapshot_id: None,
                    output_fingerprint: None,
                    error_code: Some("tool_error".into()),
                    error_summary: Some("tool failed".into()),
                    cost: CostMetrics {
                        input_tokens: 11,
                        output_tokens: 13,
                        estimated_cost_micros: 17,
                    },
                },
            )
            .unwrap();

        let finished = collector
            .finish_run(&run.run_id, 6, RunStatus::Failed, Some("failed".into()))
            .unwrap();

        assert_eq!(finished.summary.span_count, 2);
        assert_eq!(finished.summary.artifact_count, 0);
        assert_eq!(finished.summary.error_count, 1);
        assert_eq!(finished.summary.token_count, 32);
        assert_eq!(finished.summary.estimated_cost_micros, 24);
        assert_eq!(
            finished.summary.failure_class,
            Some(FailureClass::ToolFailure)
        );
        assert_eq!(
            finished.summary.final_output_preview.as_deref(),
            Some("failed")
        );
    }

    #[test]
    fn abort_run_recomputes_summary_for_interrupted_runs() {
        let storage = Arc::new(InMemoryStorage::new());
        let collector = Collector::new(storage);
        let run = collector.begin_run(sample_begin_run()).unwrap();

        collector
            .start_span(
                &run.run_id,
                &run.trace_id,
                SpanSpec {
                    span_id: Some(SpanId("planner".into())),
                    parent_span_id: None,
                    kind: SpanKind::PlannerStep,
                    name: "planner".into(),
                    started_at: 2,
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

        let aborted = collector.abort_run(&run.run_id, 3, "interrupted").unwrap();
        assert_eq!(aborted.status, RunStatus::Interrupted);
        assert_eq!(aborted.summary.error_count, 1);
        assert_eq!(aborted.summary.failure_class, Some(FailureClass::Unknown));
        assert_eq!(
            aborted.summary.final_output_preview.as_deref(),
            Some("interrupted")
        );
    }
}
