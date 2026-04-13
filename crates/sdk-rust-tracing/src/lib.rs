pub mod helpers;
pub mod layer;
pub mod sink;

use std::collections::BTreeMap;
use std::sync::Arc;

use replaykit_collector::{BeginRun, Collector, CollectorError};
use replaykit_core_model::{
    ArtifactRecord, ArtifactType, CostMetrics, Document, EdgeKind, HostMetadata, ReplayPolicy,
    RunRecord, RunStatus, SpanId, SpanKind, SpanRecord, SpanStatus, Value,
};
use replaykit_storage::Storage;

use crate::sink::{CollectorSink, Sink, SinkError};

// ---------------------------------------------------------------------------
// Re-exports
// ---------------------------------------------------------------------------

pub use crate::helpers::{
    SpanBuilder, file_read, file_write, guardrail_check, human_input, model_call, planner_step,
    retrieval, shell_command, tool_call,
};
pub use crate::layer::{Clock, ReplayKitLayer, SequentialClock, WallClock};
pub use crate::sink::{CloseSpanSpec, OpenSpanSpec};
// ---------------------------------------------------------------------------
// CompletedSpanSpec
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// SemanticSession
// ---------------------------------------------------------------------------

/// High-level session for capturing a single agent run.
///
/// Create with [`SemanticSession::start`] (in-process collector) or
/// [`SemanticSession::with_sink`] (custom transport).
pub struct SemanticSession {
    sink: Arc<dyn Sink>,
    run: RunRecord,
}

impl SemanticSession {
    /// Start a new run backed by the given storage.
    pub fn start<S: Storage + 'static>(
        storage: Arc<S>,
        title: impl Into<String>,
        entrypoint: impl Into<String>,
        started_at: u64,
    ) -> Result<Self, CollectorError> {
        let collector = Collector::new(storage);
        let run = collector.begin_run(BeginRun {
            title: title.into(),
            entrypoint: entrypoint.into(),
            adapter_name: "replaykit-sdk-rust-tracing".into(),
            adapter_version: env!("CARGO_PKG_VERSION").into(),
            started_at,
            git_sha: None,
            environment_fingerprint: None,
            host: HostMetadata {
                os: std::env::consts::OS.into(),
                arch: std::env::consts::ARCH.into(),
                hostname: None,
            },
            labels: vec!["sdk".into()],
        })?;

        let run_id = run.run_id.clone();
        let trace_id = run.trace_id.clone();
        let sink: Arc<dyn Sink> = Arc::new(CollectorSink::new(collector, run_id, trace_id));

        Ok(Self { sink, run })
    }

    /// Create a session with a custom [`Sink`] implementation.
    pub fn with_sink(sink: Arc<dyn Sink>, run: RunRecord) -> Self {
        Self { sink, run }
    }

    pub fn run(&self) -> &RunRecord {
        &self.run
    }

    /// Get a reference to the underlying sink (useful for sharing with a
    /// tracing [`ReplayKitLayer`]).
    pub fn sink(&self) -> &Arc<dyn Sink> {
        &self.sink
    }

    /// Build a tracing [`Layer`](tracing_subscriber::layer::Layer) that emits
    /// spans through this session's sink.
    pub fn layer(&self, clock: Arc<dyn Clock>) -> ReplayKitLayer {
        ReplayKitLayer::new(self.sink.clone(), clock)
    }

    /// Record a fully-completed span in one call.
    pub fn record_completed_span(&self, spec: CompletedSpanSpec) -> Result<SpanRecord, SinkError> {
        self.sink.emit_completed_span(spec)
    }

    /// Record an artifact.
    pub fn record_artifact(
        &self,
        span_id: Option<&SpanId>,
        artifact_type: ArtifactType,
        created_at: u64,
        summary: Document,
    ) -> Result<ArtifactRecord, SinkError> {
        self.sink
            .emit_artifact(span_id, artifact_type, created_at, summary)
    }

    /// Record a `DataDependsOn` edge between two spans.
    pub fn add_dependency(
        &self,
        from_span_id: SpanId,
        to_span_id: SpanId,
    ) -> Result<(), SinkError> {
        self.sink.emit_dependency(from_span_id, to_span_id)
    }

    /// Record an arbitrary edge.
    pub fn add_edge(
        &self,
        from_span_id: SpanId,
        to_span_id: SpanId,
        kind: EdgeKind,
    ) -> Result<(), SinkError> {
        self.sink.emit_edge(from_span_id, to_span_id, kind)
    }

    /// Finish the run.
    pub fn finish(self, ended_at: u64, status: RunStatus) -> Result<RunRecord, SinkError> {
        self.sink.finish_run(ended_at, status, None)
    }
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

pub fn summary_from_pairs(pairs: &[(&str, &str)]) -> Document {
    let mut summary = Document::new();
    for (key, value) in pairs {
        summary.insert((*key).to_owned(), Value::Text((*value).to_owned()));
    }
    summary
}

pub fn fixed_ids(
    run: &RunRecord,
) -> (&replaykit_core_model::RunId, &replaykit_core_model::TraceId) {
    (&run.run_id, &run.trace_id)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use replaykit_core_model::*;
    use replaykit_storage::InMemoryStorage;

    use super::*;
    use crate::helpers;

    fn make_session() -> (Arc<InMemoryStorage>, SemanticSession) {
        let storage = Arc::new(InMemoryStorage::new());
        let session =
            SemanticSession::start(storage.clone(), "test run", "test.main", 100).expect("session");
        (storage, session)
    }

    // -- Span emission through the session ---------------------------------

    #[test]
    fn record_completed_span_roundtrips() {
        let (storage, session) = make_session();
        let run_id = session.run().run_id.clone();

        let span = session
            .record_completed_span(
                helpers::planner_step("plan")
                    .span_id("planner-1")
                    .times(100, 110)
                    .output_fingerprint("fp-1")
                    .build(),
            )
            .expect("span");

        assert_eq!(span.kind, SpanKind::PlannerStep);
        assert_eq!(span.name, "plan");
        assert_eq!(span.replay_policy, ReplayPolicy::RecordOnly);
        assert_eq!(span.output_fingerprint.as_deref(), Some("fp-1"));

        session.finish(111, RunStatus::Completed).expect("finish");

        let spans = storage.list_spans(&run_id).expect("spans");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].span_id, SpanId("planner-1".into()));
    }

    // -- Artifact association ----------------------------------------------

    #[test]
    fn input_and_output_artifacts_attached() {
        let (storage, session) = make_session();
        let run_id = session.run().run_id.clone();

        session
            .record_completed_span(
                helpers::model_call("llm")
                    .span_id("llm-1")
                    .times(100, 120)
                    .input(
                        ArtifactType::ModelRequest,
                        summary_from_pairs(&[("model", "claude")]),
                    )
                    .output(
                        ArtifactType::ModelResponse,
                        summary_from_pairs(&[("tokens", "350")]),
                    )
                    .build(),
            )
            .expect("llm");

        session.finish(121, RunStatus::Completed).expect("finish");

        let artifacts = storage.list_artifacts(&run_id).expect("artifacts");
        assert_eq!(artifacts.len(), 2);
        let types: Vec<_> = artifacts.iter().map(|a| a.artifact_type).collect();
        assert!(types.contains(&ArtifactType::ModelRequest));
        assert!(types.contains(&ArtifactType::ModelResponse));
    }

    // -- Replay policy propagation -----------------------------------------

    #[test]
    fn helpers_set_correct_replay_policies() {
        let cases = vec![
            (helpers::planner_step("a").build(), ReplayPolicy::RecordOnly),
            (
                helpers::model_call("b").build(),
                ReplayPolicy::RerunnableSupported,
            ),
            (
                helpers::tool_call("c").build(),
                ReplayPolicy::RerunnableSupported,
            ),
            (
                helpers::shell_command("d").build(),
                ReplayPolicy::RerunnableSupported,
            ),
            (helpers::file_read("e").build(), ReplayPolicy::PureReusable),
            (
                helpers::file_write("f").build(),
                ReplayPolicy::RerunnableSupported,
            ),
            (helpers::human_input("g").build(), ReplayPolicy::RecordOnly),
            (
                helpers::retrieval("h").build(),
                ReplayPolicy::CacheableIfFingerprintMatches,
            ),
            (
                helpers::guardrail_check("i").build(),
                ReplayPolicy::PureReusable,
            ),
        ];

        for (spec, expected_policy) in cases {
            assert_eq!(
                spec.replay_policy, expected_policy,
                "wrong policy for {:?}",
                spec.kind,
            );
        }
    }

    // -- Executor metadata propagation -------------------------------------

    #[test]
    fn executor_metadata_propagated() {
        let (_, session) = make_session();

        let span = session
            .record_completed_span(
                helpers::model_call("llm")
                    .span_id("llm-exec")
                    .times(100, 120)
                    .executor("claude-3.5-sonnet", "2024-10-22")
                    .build(),
            )
            .expect("span");

        assert_eq!(span.executor_kind.as_deref(), Some("claude-3.5-sonnet"));
        assert_eq!(span.executor_version.as_deref(), Some("2024-10-22"));

        session.finish(121, RunStatus::Completed).expect("finish");
    }

    // -- Dependency edge emission ------------------------------------------

    #[test]
    fn dependency_edge_emitted() {
        let (storage, session) = make_session();
        let run_id = session.run().run_id.clone();

        let a = session
            .record_completed_span(
                helpers::tool_call("search")
                    .span_id("tool-1")
                    .times(100, 110)
                    .build(),
            )
            .expect("a");

        let b = session
            .record_completed_span(
                helpers::model_call("llm")
                    .span_id("llm-1")
                    .times(111, 120)
                    .build(),
            )
            .expect("b");

        session
            .add_dependency(b.span_id.clone(), a.span_id.clone())
            .expect("dep");

        session.finish(121, RunStatus::Completed).expect("finish");

        let edges = storage.list_edges(&run_id).expect("edges");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].kind, EdgeKind::DataDependsOn);
        assert_eq!(edges[0].from_span_id, SpanId("llm-1".into()));
        assert_eq!(edges[0].to_span_id, SpanId("tool-1".into()));
    }

    // -- Parent-child structure --------------------------------------------

    #[test]
    fn parent_child_structure_correct() {
        let (storage, session) = make_session();
        let run_id = session.run().run_id.clone();

        let parent = session
            .record_completed_span(
                helpers::planner_step("plan")
                    .span_id("parent-1")
                    .times(100, 200)
                    .build(),
            )
            .expect("parent");

        session
            .record_completed_span(
                helpers::tool_call("child")
                    .span_id("child-1")
                    .parent(&parent.span_id)
                    .times(110, 120)
                    .build(),
            )
            .expect("child");

        session.finish(201, RunStatus::Completed).expect("finish");

        let spans = storage.list_spans(&run_id).expect("spans");
        let child = spans
            .iter()
            .find(|s| s.span_id == SpanId("child-1".into()))
            .expect("child");
        assert_eq!(child.parent_span_id, Some(SpanId("parent-1".into())));
    }

    // -- Snapshot association ----------------------------------------------

    #[test]
    fn snapshot_created_when_summary_provided() {
        let (storage, session) = make_session();
        let run_id = session.run().run_id.clone();

        session
            .record_completed_span(
                helpers::planner_step("plan")
                    .span_id("snap-1")
                    .times(100, 110)
                    .snapshot(summary_from_pairs(&[("state", "initial")]))
                    .build(),
            )
            .expect("span");

        session.finish(111, RunStatus::Completed).expect("finish");

        let snapshots = storage.list_snapshots(&run_id).expect("snaps");
        assert_eq!(snapshots.len(), 1);
    }

    // -- Layer integration -------------------------------------------------

    #[test]
    fn layer_captures_spans_with_rk_kind() {
        use tracing_subscriber::prelude::*;

        let storage = Arc::new(InMemoryStorage::new());
        let session = SemanticSession::start(storage.clone(), "layer test", "test", 1).unwrap();
        let run_id = session.run().run_id.clone();

        let clock = Arc::new(SequentialClock::new(10));
        let layer = session.layer(clock);

        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        {
            let _span = tracing::info_span!("my_planner", rk_kind = "PlannerStep").entered();
        }

        session.finish(100, RunStatus::Completed).unwrap();

        let spans = storage.list_spans(&run_id).unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].kind, SpanKind::PlannerStep);
        assert_eq!(spans[0].name, "my_planner");
    }

    #[test]
    fn layer_ignores_non_rk_spans() {
        use tracing_subscriber::prelude::*;

        let storage = Arc::new(InMemoryStorage::new());
        let session = SemanticSession::start(storage.clone(), "test", "test", 1).unwrap();
        let run_id = session.run().run_id.clone();

        let clock = Arc::new(SequentialClock::new(10));
        let layer = session.layer(clock);

        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        {
            let _span = tracing::info_span!("internal_log", foo = "bar").entered();
        }

        session.finish(100, RunStatus::Completed).unwrap();

        let spans = storage.list_spans(&run_id).unwrap();
        assert!(spans.is_empty());
    }

    #[test]
    fn layer_propagates_parent_child() {
        use tracing_subscriber::prelude::*;

        let storage = Arc::new(InMemoryStorage::new());
        let session = SemanticSession::start(storage.clone(), "test", "test", 1).unwrap();
        let run_id = session.run().run_id.clone();

        let clock = Arc::new(SequentialClock::new(10));
        let layer = session.layer(clock);

        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        {
            let parent = tracing::info_span!("parent", rk_kind = "PlannerStep").entered();
            {
                let _child =
                    tracing::info_span!(parent: parent.id(), "child", rk_kind = "ToolCall")
                        .entered();
            }
        }

        session.finish(100, RunStatus::Completed).unwrap();

        let spans = storage.list_spans(&run_id).unwrap();
        assert_eq!(spans.len(), 2);

        let child = spans.iter().find(|s| s.name == "child").unwrap();
        let parent = spans.iter().find(|s| s.name == "parent").unwrap();
        assert_eq!(child.parent_span_id, Some(parent.span_id.clone()));
    }
}
