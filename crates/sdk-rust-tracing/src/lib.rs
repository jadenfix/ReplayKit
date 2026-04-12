use std::collections::BTreeMap;
use std::sync::Arc;

use replaykit_collector::{
    ArtifactSpec, BeginRun, Collector, CollectorError, EdgeSpec, EndSpan, SnapshotSpec, SpanSpec,
};
use replaykit_core_model::{
    ArtifactType, CostMetrics, Document, EdgeKind, HostMetadata, ReplayPolicy, RunId, RunRecord,
    RunStatus, SpanId, SpanKind, SpanRecord, SpanStatus, TraceId, Value,
};
use replaykit_storage::Storage;

pub struct SemanticSession<S: Storage> {
    collector: Collector<S>,
    run: RunRecord,
}

impl<S: Storage> SemanticSession<S> {
    pub fn start(storage: Arc<S>, title: impl Into<String>, entrypoint: impl Into<String>) -> Result<Self, CollectorError> {
        let collector = Collector::new(storage);
        let run = collector.begin_run(BeginRun {
            title: title.into(),
            entrypoint: entrypoint.into(),
            adapter_name: "replaykit-sdk-rust-tracing".into(),
            adapter_version: env!("CARGO_PKG_VERSION").into(),
            started_at: 1,
            git_sha: None,
            environment_fingerprint: None,
            host: HostMetadata {
                os: std::env::consts::OS.into(),
                arch: std::env::consts::ARCH.into(),
                hostname: None,
            },
            labels: vec!["demo".into()],
        })?;
        Ok(Self { collector, run })
    }

    pub fn run(&self) -> &RunRecord {
        &self.run
    }

    pub fn record_artifact(
        &self,
        span_id: Option<&SpanId>,
        artifact_type: ArtifactType,
        created_at: u64,
        summary: Document,
    ) -> Result<replaykit_core_model::ArtifactRecord, CollectorError> {
        self.collector.add_artifact(
            &self.run.run_id,
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

    pub fn record_completed_span(
        &self,
        spec: CompletedSpanSpec,
    ) -> Result<SpanRecord, CollectorError> {
        let input_artifact = match spec.input_summary {
            Some(summary) => Some(self.record_artifact(
                None,
                spec.input_artifact_type.unwrap_or(ArtifactType::DebugLog),
                spec.started_at,
                summary,
            )?),
            None => None,
        };

        let started = self.collector.start_span(
            &self.run.run_id,
            &self.run.trace_id,
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
                    .map(|artifact| artifact.artifact_id.clone())
                    .collect(),
                input_fingerprint: spec.input_fingerprint,
                environment_fingerprint: spec.environment_fingerprint,
                attributes: spec.attributes,
            },
        )?;

        let output_artifact = match spec.output_summary {
            Some(summary) => Some(self.record_artifact(
                Some(&started.span_id),
                spec.output_artifact_type.unwrap_or(ArtifactType::DebugLog),
                spec.ended_at,
                summary,
            )?),
            None => None,
        };

        let snapshot = match spec.snapshot_summary {
            Some(summary) => {
                let state_artifact = self.record_artifact(
                    Some(&started.span_id),
                    ArtifactType::StateSnapshot,
                    spec.ended_at,
                    summary.clone(),
                )?;
                Some(self.collector.add_snapshot(
                    &self.run.run_id,
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

        self.collector.end_span(
            &self.run.run_id,
            &started.span_id,
            EndSpan {
                ended_at: spec.ended_at,
                status: spec.status,
                output_artifact_ids: output_artifact
                    .iter()
                    .map(|artifact| artifact.artifact_id.clone())
                    .collect(),
                snapshot_id: snapshot.map(|snapshot| snapshot.snapshot_id),
                output_fingerprint: spec.output_fingerprint,
                error_code: None,
                error_summary: spec.error_summary,
                cost: spec.cost,
            },
        )
    }

    pub fn add_dependency(
        &self,
        from_span_id: SpanId,
        to_span_id: SpanId,
    ) -> Result<(), CollectorError> {
        self.collector.add_edge(
            &self.run.run_id,
            EdgeSpec {
                from_span_id,
                to_span_id,
                kind: EdgeKind::DataDependsOn,
                attributes: BTreeMap::new(),
            },
        )?;
        Ok(())
    }

    pub fn finish(self, ended_at: u64, status: RunStatus) -> Result<RunRecord, CollectorError> {
        self.collector
            .finish_run(&self.run.run_id, ended_at, status, None)
    }
}

#[derive(Clone, Debug)]
pub struct CompletedSpanSpec {
    pub span_id: Option<SpanId>,
    pub parent_span_id: Option<SpanId>,
    pub kind: SpanKind,
    pub name: String,
    pub started_at: u64,
    pub ended_at: u64,
    pub status: SpanStatus,
    pub replay_policy: ReplayPolicy,
    pub executor_kind: Option<String>,
    pub executor_version: Option<String>,
    pub input_fingerprint: Option<String>,
    pub output_fingerprint: Option<String>,
    pub environment_fingerprint: Option<String>,
    pub attributes: Document,
    pub input_summary: Option<Document>,
    pub input_artifact_type: Option<ArtifactType>,
    pub output_summary: Option<Document>,
    pub output_artifact_type: Option<ArtifactType>,
    pub snapshot_summary: Option<Document>,
    pub error_summary: Option<String>,
    pub cost: CostMetrics,
}

impl CompletedSpanSpec {
    pub fn simple(kind: SpanKind, name: impl Into<String>, started_at: u64, ended_at: u64) -> Self {
        Self {
            span_id: None,
            parent_span_id: None,
            kind,
            name: name.into(),
            started_at,
            ended_at,
            status: SpanStatus::Completed,
            replay_policy: ReplayPolicy::RecordOnly,
            executor_kind: None,
            executor_version: None,
            input_fingerprint: None,
            output_fingerprint: None,
            environment_fingerprint: None,
            attributes: BTreeMap::new(),
            input_summary: None,
            input_artifact_type: None,
            output_summary: None,
            output_artifact_type: None,
            snapshot_summary: None,
            error_summary: None,
            cost: CostMetrics::default(),
        }
    }
}

pub fn summary_from_pairs(pairs: &[(&str, &str)]) -> Document {
    let mut summary = Document::new();
    for (key, value) in pairs {
        summary.insert((*key).to_owned(), Value::Text((*value).to_owned()));
    }
    summary
}

pub fn fixed_ids(run: &RunRecord) -> (&RunId, &TraceId) {
    (&run.run_id, &run.trace_id)
}
