use std::sync::Arc;

use replaykit_api::ReplayKitService;
use replaykit_core_model::{
    BranchRequest, PatchManifest, PatchType, RunId, SpanId, SpanKind, Value,
};
use replaykit_replay_engine::NoopExecutorRegistry;
use replaykit_sdk_rust_tracing::{CompletedSpanSpec, SemanticSession, summary_from_pairs};
use replaykit_storage::InMemoryStorage;

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let command = args.get(1).map(String::as_str).unwrap_or("demo");

    let storage = Arc::new(InMemoryStorage::new());
    let service = ReplayKitService::new(storage.clone(), NoopExecutorRegistry);

    match command {
        "demo" => {
            let run_id = seed_demo_run(storage).expect("seed demo run");
            print_run(&service, &run_id);
        }
        "demo-branch" => {
            let run_id = seed_demo_run(storage).expect("seed demo run");
            let execution = service
                .create_branch(BranchRequest {
                    source_run_id: run_id.clone(),
                    fork_span_id: SpanId("tool-search".into()),
                    patch_manifest: PatchManifest {
                        patch_type: PatchType::ToolOutputOverride,
                        target_artifact_id: None,
                        replacement: Value::Text("patched repository result".into()),
                        note: Some("simulate corrected tool output".into()),
                        created_at: 100,
                    },
                    created_by: Some("cli".into()),
                })
                .expect("branch creation");
            println!("source run: {}", run_id.0);
            println!("branch run: {}", execution.target_run.run_id.0);
            println!("branch status: {:?}", execution.target_run.status);
            println!("dirty spans:");
            for dirty in execution.plan.dirty_spans {
                println!("  - {} {:?}", dirty.span_id.0, dirty.reasons);
            }
            let diff = service
                .cached_diff(
                    &execution.branch.source_run_id,
                    &execution.branch.target_run_id,
                )
                .expect("cached diff");
            println!(
                "diff summary: changed_spans={} first_divergent={}",
                diff.changed_span_count,
                diff.first_divergent_span_id
                    .map(|id| id.0)
                    .unwrap_or_else(|| "none".into())
            );
        }
        _ => {
            println!("usage:");
            println!("  replaykit demo");
            println!("  replaykit demo-branch");
        }
    }
}

fn seed_demo_run(storage: Arc<InMemoryStorage>) -> Result<RunId, String> {
    let session = SemanticSession::start(storage, "demo coding run", "agent.main")
        .map_err(|err| err.to_string())?;
    let run_id = session.run().run_id.clone();

    let planner = session
        .record_completed_span(CompletedSpanSpec {
            span_id: Some(SpanId("planner".into())),
            kind: SpanKind::PlannerStep,
            name: "plan".into(),
            started_at: 1,
            ended_at: 2,
            replay_policy: replaykit_core_model::ReplayPolicy::RecordOnly,
            output_summary: Some(summary_from_pairs(&[("plan", "inspect and patch code")])),
            output_fingerprint: Some("planner-v1".into()),
            ..CompletedSpanSpec::simple(SpanKind::PlannerStep, "plan", 1, 2)
        })
        .map_err(|err| err.to_string())?;

    let tool = session
        .record_completed_span(CompletedSpanSpec {
            span_id: Some(SpanId("tool-search".into())),
            parent_span_id: Some(planner.span_id.clone()),
            kind: SpanKind::ToolCall,
            name: "search_repository".into(),
            started_at: 3,
            ended_at: 4,
            replay_policy: replaykit_core_model::ReplayPolicy::RerunnableSupported,
            output_summary: Some(summary_from_pairs(&[("match", "src/lib.rs")])),
            output_artifact_type: Some(replaykit_core_model::ArtifactType::ToolOutput),
            output_fingerprint: Some("tool-v1".into()),
            ..CompletedSpanSpec::simple(SpanKind::ToolCall, "search_repository", 3, 4)
        })
        .map_err(|err| err.to_string())?;

    let answer = session
        .record_completed_span(CompletedSpanSpec {
            span_id: Some(SpanId("final-answer".into())),
            parent_span_id: Some(planner.span_id.clone()),
            kind: SpanKind::LlmCall,
            name: "compose_answer".into(),
            started_at: 5,
            ended_at: 6,
            status: replaykit_core_model::SpanStatus::Failed,
            replay_policy: replaykit_core_model::ReplayPolicy::RerunnableSupported,
            input_summary: Some(summary_from_pairs(&[("tool", "tool-search")])),
            output_summary: Some(summary_from_pairs(&[("answer", "tests failed")])),
            output_artifact_type: Some(replaykit_core_model::ArtifactType::ModelResponse),
            output_fingerprint: Some("answer-v1".into()),
            error_summary: Some("could not produce a correct patch".into()),
            ..CompletedSpanSpec::simple(SpanKind::LlmCall, "compose_answer", 5, 6)
        })
        .map_err(|err| err.to_string())?;

    session
        .add_dependency(tool.span_id.clone(), answer.span_id.clone())
        .map_err(|err| err.to_string())?;
    session
        .finish(7, replaykit_core_model::RunStatus::Failed)
        .map_err(|err| err.to_string())?;

    Ok(run_id)
}

fn print_run(service: &ReplayKitService<InMemoryStorage, NoopExecutorRegistry>, run_id: &RunId) {
    let runs = service.list_runs().expect("runs");
    println!("runs: {}", runs.len());
    let trees = service.run_tree(run_id).expect("run tree");
    for node in trees {
        print_node(&node, 0);
    }
}

fn print_node(node: &replaykit_core_model::RunTreeNode, depth: usize) {
    let indent = "  ".repeat(depth);
    println!(
        "{}- {} [{:?}] {:?}",
        indent, node.span.name, node.span.kind, node.span.status
    );
    for child in &node.children {
        print_node(child, depth + 1);
    }
}
