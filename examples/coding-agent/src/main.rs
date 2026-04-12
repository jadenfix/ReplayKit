use std::sync::Arc;

use replaykit_api::ReplayKitService;
use replaykit_core_model::{RunStatus, SpanId, SpanKind};
use replaykit_replay_engine::NoopExecutorRegistry;
use replaykit_sdk_rust_tracing::{CompletedSpanSpec, SemanticSession, summary_from_pairs};
use replaykit_storage::InMemoryStorage;

fn main() {
    let storage = Arc::new(InMemoryStorage::new());
    let session = SemanticSession::start(storage.clone(), "example coding agent", "agent.main")
        .expect("create session");

    let planner = session
        .record_completed_span(CompletedSpanSpec {
            span_id: Some(SpanId("planner".into())),
            kind: SpanKind::PlannerStep,
            name: "planner".into(),
            started_at: 1,
            ended_at: 2,
            output_summary: Some(summary_from_pairs(&[("goal", "fix failing tests")])),
            output_fingerprint: Some("planner-v1".into()),
            ..CompletedSpanSpec::simple(SpanKind::PlannerStep, "planner", 1, 2)
        })
        .expect("planner");

    let shell = session
        .record_completed_span(CompletedSpanSpec {
            span_id: Some(SpanId("test-run".into())),
            parent_span_id: Some(planner.span_id.clone()),
            kind: SpanKind::ShellCommand,
            name: "cargo test".into(),
            started_at: 3,
            ended_at: 4,
            replay_policy: replaykit_core_model::ReplayPolicy::RerunnableSupported,
            output_summary: Some(summary_from_pairs(&[("stderr", "1 test failed")])),
            output_artifact_type: Some(replaykit_core_model::ArtifactType::ShellStderr),
            output_fingerprint: Some("test-v1".into()),
            ..CompletedSpanSpec::simple(SpanKind::ShellCommand, "cargo test", 3, 4)
        })
        .expect("shell");

    session
        .add_dependency(planner.span_id.clone(), shell.span_id.clone())
        .expect("dependency");
    session.finish(5, RunStatus::Failed).expect("finish run");

    let service = ReplayKitService::new(storage, NoopExecutorRegistry);
    let runs = service.list_runs().expect("runs");
    println!("captured {} run(s)", runs.len());
}
