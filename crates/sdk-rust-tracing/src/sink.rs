use std::collections::BTreeMap;
use std::fmt;

use replaykit_collector::{
    ArtifactSpec, Collector, CollectorError, EdgeSpec, EndSpan, SnapshotSpec, SpanSpec,
};
use replaykit_core_model::{
    ArtifactRecord, ArtifactType, CostMetrics, Document, EdgeKind, ReplayPolicy, RunId, RunRecord,
    RunStatus, SpanId, SpanKind, SpanRecord, SpanStatus, TraceId,
};
use replaykit_storage::Storage;

use crate::CompletedSpanSpec;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum SinkError {
    Collector(CollectorError),
}

impl fmt::Display for SinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SinkError::Collector(e) => write!(f, "{e}"),
        }
    }
}

impl From<CollectorError> for SinkError {
    fn from(e: CollectorError) -> Self {
        SinkError::Collector(e)
    }
}

// ---------------------------------------------------------------------------
// Sink trait
// ---------------------------------------------------------------------------

/// Abstraction over how ReplayKit span/artifact/edge data is persisted.
///
/// Today the only implementation is [`CollectorSink`] which wraps an
/// in-process [`Collector`].  A future local-transport sink could send data
/// over a socket without changing any SDK call-sites.
pub trait Sink: Send + Sync {
    fn run_id(&self) -> &RunId;
    fn trace_id(&self) -> &TraceId;

    /// Record a fully-completed span (atomic start + end).
    fn emit_completed_span(&self, spec: CompletedSpanSpec) -> Result<SpanRecord, SinkError>;

    /// Open a live span (used by the tracing [`Layer`](crate::layer::ReplayKitLayer)).
    fn open_span(&self, spec: OpenSpanSpec) -> Result<SpanRecord, SinkError>;

    /// Close a previously-opened live span.
    fn close_span(&self, span_id: &SpanId, spec: CloseSpanSpec) -> Result<SpanRecord, SinkError>;

    /// Record an artifact attached to an optional span.
    fn emit_artifact(
        &self,
        span_id: Option<&SpanId>,
        artifact_type: ArtifactType,
        created_at: u64,
        summary: Document,
    ) -> Result<ArtifactRecord, SinkError>;

    /// Record a `DataDependsOn` edge.
    fn emit_dependency(&self, from: SpanId, to: SpanId) -> Result<(), SinkError>;

    /// Record an arbitrary edge.
    fn emit_edge(&self, from: SpanId, to: SpanId, kind: EdgeKind) -> Result<(), SinkError>;

    /// Finish the run and recompute its summary.
    fn finish_run(
        &self,
        ended_at: u64,
        status: RunStatus,
        final_output: Option<String>,
    ) -> Result<RunRecord, SinkError>;
}

// ---------------------------------------------------------------------------
// Open / Close specs (for live spans via the tracing Layer)
// ---------------------------------------------------------------------------

pub struct OpenSpanSpec {
    pub span_id: Option<SpanId>,
    pub parent_span_id: Option<SpanId>,
    pub kind: SpanKind,
    pub name: String,
    pub started_at: u64,
    pub replay_policy: ReplayPolicy,
    pub executor_kind: Option<String>,
    pub executor_version: Option<String>,
    pub input_fingerprint: Option<String>,
    pub environment_fingerprint: Option<String>,
    pub attributes: Document,
}

pub struct CloseSpanSpec {
    pub ended_at: u64,
    pub status: SpanStatus,
    pub output_fingerprint: Option<String>,
    pub error_summary: Option<String>,
    pub cost: CostMetrics,
}

// ---------------------------------------------------------------------------
// CollectorSink  (in-process, wraps Collector<S>)
// ---------------------------------------------------------------------------

pub struct CollectorSink<S: Storage> {
    collector: Collector<S>,
    run_id: RunId,
    trace_id: TraceId,
}

impl<S: Storage> CollectorSink<S> {
    pub fn new(collector: Collector<S>, run_id: RunId, trace_id: TraceId) -> Self {
        Self {
            collector,
            run_id,
            trace_id,
        }
    }

    pub fn collector(&self) -> &Collector<S> {
        &self.collector
    }

    fn make_artifact(
        &self,
        span_id: Option<&SpanId>,
        artifact_type: ArtifactType,
        created_at: u64,
        summary: Document,
    ) -> Result<ArtifactRecord, CollectorError> {
        self.collector.add_artifact(
            &self.run_id,
            span_id,
            ArtifactSpec {
                artifact_type,
                mime: "application/json".into(),
                sha256: format!("summary-{created_at}"),
                byte_len: summary.len(),
                blob_path: format!("memory://artifact/{created_at}"),
                summary,
                redaction: Document::new(),
                created_at,
            },
        )
    }
}

impl<S: Storage> Sink for CollectorSink<S> {
    fn run_id(&self) -> &RunId {
        &self.run_id
    }

    fn trace_id(&self) -> &TraceId {
        &self.trace_id
    }

    fn emit_completed_span(&self, spec: CompletedSpanSpec) -> Result<SpanRecord, SinkError> {
        // 1. Input artifact (before span start, so ID can be passed as input).
        let input_artifact = match spec.input_summary {
            Some(summary) => Some(self.make_artifact(
                None,
                spec.input_artifact_type.unwrap_or(ArtifactType::DebugLog),
                spec.started_at,
                summary,
            )?),
            None => None,
        };

        // 2. Start span.
        let started = self.collector.start_span(
            &self.run_id,
            &self.trace_id,
            SpanSpec {
                span_id: spec.span_id,
                parent_span_id: spec.parent_span_id,
                kind: spec.kind,
                name: spec.name,
                started_at: spec.started_at,
                replay_policy: spec.replay_policy,
                executor_kind: spec.executor_kind,
                executor_version: spec.executor_version,
                input_artifact_ids: input_artifact
                    .iter()
                    .map(|a| a.artifact_id.clone())
                    .collect(),
                input_fingerprint: spec.input_fingerprint,
                environment_fingerprint: spec.environment_fingerprint,
                attributes: spec.attributes,
            },
        )?;

        // 3. Output artifact (attached to span).
        let output_artifact = match spec.output_summary {
            Some(summary) => Some(self.make_artifact(
                Some(&started.span_id),
                spec.output_artifact_type.unwrap_or(ArtifactType::DebugLog),
                spec.ended_at,
                summary,
            )?),
            None => None,
        };

        // 4. Snapshot (if any).
        let snapshot = match spec.snapshot_summary {
            Some(summary) => {
                let state_artifact = self.make_artifact(
                    Some(&started.span_id),
                    ArtifactType::StateSnapshot,
                    spec.ended_at,
                    summary.clone(),
                )?;
                Some(self.collector.add_snapshot(
                    &self.run_id,
                    Some(&started.span_id),
                    SnapshotSpec {
                        kind: "state".into(),
                        artifact_id: state_artifact.artifact_id,
                        summary,
                        created_at: spec.ended_at,
                    },
                )?)
            }
            None => None,
        };

        // 5. End span.
        Ok(self.collector.end_span(
            &self.run_id,
            &started.span_id,
            EndSpan {
                ended_at: spec.ended_at,
                status: spec.status,
                output_artifact_ids: output_artifact
                    .iter()
                    .map(|a| a.artifact_id.clone())
                    .collect(),
                snapshot_id: snapshot.map(|s| s.snapshot_id),
                output_fingerprint: spec.output_fingerprint,
                error_code: None,
                error_summary: spec.error_summary,
                cost: spec.cost,
            },
        )?)
    }

    fn open_span(&self, spec: OpenSpanSpec) -> Result<SpanRecord, SinkError> {
        Ok(self.collector.start_span(
            &self.run_id,
            &self.trace_id,
            SpanSpec {
                span_id: spec.span_id,
                parent_span_id: spec.parent_span_id,
                kind: spec.kind,
                name: spec.name,
                started_at: spec.started_at,
                replay_policy: spec.replay_policy,
                executor_kind: spec.executor_kind,
                executor_version: spec.executor_version,
                input_artifact_ids: Vec::new(),
                input_fingerprint: spec.input_fingerprint,
                environment_fingerprint: spec.environment_fingerprint,
                attributes: spec.attributes,
            },
        )?)
    }

    fn close_span(&self, span_id: &SpanId, spec: CloseSpanSpec) -> Result<SpanRecord, SinkError> {
        Ok(self.collector.end_span(
            &self.run_id,
            span_id,
            EndSpan {
                ended_at: spec.ended_at,
                status: spec.status,
                output_artifact_ids: Vec::new(),
                snapshot_id: None,
                output_fingerprint: spec.output_fingerprint,
                error_code: None,
                error_summary: spec.error_summary,
                cost: spec.cost,
            },
        )?)
    }

    fn emit_artifact(
        &self,
        span_id: Option<&SpanId>,
        artifact_type: ArtifactType,
        created_at: u64,
        summary: Document,
    ) -> Result<ArtifactRecord, SinkError> {
        Ok(self.make_artifact(span_id, artifact_type, created_at, summary)?)
    }

    fn emit_dependency(&self, from: SpanId, to: SpanId) -> Result<(), SinkError> {
        self.emit_edge(from, to, EdgeKind::DataDependsOn)
    }

    fn emit_edge(&self, from: SpanId, to: SpanId, kind: EdgeKind) -> Result<(), SinkError> {
        self.collector.add_edge(
            &self.run_id,
            EdgeSpec {
                from_span_id: from,
                to_span_id: to,
                kind,
                attributes: BTreeMap::new(),
            },
        )?;
        Ok(())
    }

    fn finish_run(
        &self,
        ended_at: u64,
        status: RunStatus,
        final_output: Option<String>,
    ) -> Result<RunRecord, SinkError> {
        Ok(self
            .collector
            .finish_run(&self.run_id, ended_at, status, final_output)?)
    }
}
