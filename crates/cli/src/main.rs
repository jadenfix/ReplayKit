use std::sync::Arc;

use clap::{Parser, Subcommand};
use replaykit_api::ReplayKitService;
use replaykit_api::views::{
    self, ArtifactPreviewView, DependencyView, RunDiffSummaryView, RunSummaryView, SpanDetailView,
    TreeNodeView,
};
use replaykit_core_model::{
    BranchRequest, PatchManifest, PatchType, RunId, RunTreeNode, SpanId, SpanKind, Value,
};
use replaykit_replay_engine::NoopExecutorRegistry;
use replaykit_sdk_rust_tracing::{CompletedSpanSpec, SemanticSession, summary_from_pairs};
use replaykit_storage::{InMemoryStorage, SqliteStorage, Storage};

// ---------------------------------------------------------------------------
// CLI argument parsing
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "replaykit", about = "ReplayKit debugger CLI")]
struct Cli {
    /// Storage backend: "memory" or "sqlite"
    #[arg(long, env = "REPLAYKIT_STORAGE", default_value = "memory")]
    storage: String,

    /// SQLite database path (when storage=sqlite)
    #[arg(
        long,
        env = "REPLAYKIT_DB_PATH",
        default_value = "data/replaykit/replaykit.db"
    )]
    db_path: String,

    /// Output as JSON instead of human-readable text
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run management commands
    Runs {
        #[command(subcommand)]
        action: RunsAction,
    },
    /// Replay and branching commands
    Replay {
        #[command(subcommand)]
        action: ReplayAction,
    },
    /// Start the local HTTP API server
    Serve {
        /// Port to listen on
        #[arg(long, default_value = "3210")]
        port: u16,
    },
    /// Seed a demo run (for testing)
    Demo,
    /// Seed a demo run and create a branch (for testing)
    DemoBranch,
}

#[derive(Subcommand)]
enum RunsAction {
    /// List all runs
    List,
    /// Show run summary
    Show {
        /// Run ID
        run_id: String,
    },
    /// Show run span tree
    Tree {
        /// Run ID
        run_id: String,
    },
    /// Diff two runs
    Diff {
        /// Source run ID
        source: String,
        /// Target run ID
        target: String,
    },
    /// Show span details
    Span {
        /// Run ID
        run_id: String,
        /// Span ID
        span_id: String,
    },
    /// List artifacts for a span
    Artifacts {
        /// Run ID
        run_id: String,
        /// Span ID
        span_id: String,
    },
    /// List dependencies for a span
    Deps {
        /// Run ID
        run_id: String,
        /// Span ID
        span_id: String,
    },
}

#[derive(Subcommand)]
enum ReplayAction {
    /// Create a forked branch with a patch
    Fork {
        /// Source run ID
        run_id: String,
        /// Span ID to fork at
        #[arg(long)]
        span: String,
        /// Path to patch file (JSON with "replacement" text)
        #[arg(long)]
        patch: Option<String>,
        /// Inline replacement value
        #[arg(long)]
        replacement: Option<String>,
        /// Patch type (default: tool_output_override)
        #[arg(long, default_value = "tool_output_override")]
        patch_type: String,
    },
    /// Preview what a fork would do (dry-run)
    Plan {
        /// Source run ID
        run_id: String,
        /// Span ID to fork at
        #[arg(long)]
        span: String,
        /// Patch type (default: tool_output_override)
        #[arg(long, default_value = "tool_output_override")]
        patch_type: String,
        /// Inline replacement value
        #[arg(long, default_value = "")]
        replacement: String,
    },
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let cli = Cli::parse();

    match cli.storage.as_str() {
        "memory" => dispatch(cli, Arc::new(InMemoryStorage::new())),
        "sqlite" => {
            let storage = SqliteStorage::open(&cli.db_path).expect("open sqlite storage");
            dispatch(cli, Arc::new(storage));
        }
        other => {
            eprintln!("unsupported storage backend: {other}");
            std::process::exit(2);
        }
    }
}

fn dispatch<S: Storage + 'static>(cli: Cli, storage: Arc<S>) {
    let service = Arc::new(ReplayKitService::new(storage.clone(), NoopExecutorRegistry));
    let json = cli.json;

    match cli.command {
        Commands::Runs { action } => match action {
            RunsAction::List => cmd_runs_list(&service, json),
            RunsAction::Show { run_id } => cmd_runs_show(&service, &run_id, json),
            RunsAction::Tree { run_id } => cmd_runs_tree(&service, &run_id, json),
            RunsAction::Diff { source, target } => cmd_runs_diff(&service, &source, &target, json),
            RunsAction::Span { run_id, span_id } => {
                cmd_span_detail(&service, &run_id, &span_id, json)
            }
            RunsAction::Artifacts { run_id, span_id } => {
                cmd_span_artifacts(&service, &run_id, &span_id, json)
            }
            RunsAction::Deps { run_id, span_id } => {
                cmd_span_deps(&service, &run_id, &span_id, json)
            }
        },
        Commands::Replay { action } => match action {
            ReplayAction::Fork {
                run_id,
                span,
                patch,
                replacement,
                patch_type,
            } => cmd_replay_fork(
                &service,
                &run_id,
                &span,
                patch,
                replacement,
                &patch_type,
                json,
            ),
            ReplayAction::Plan {
                run_id,
                span,
                patch_type,
                replacement,
            } => cmd_replay_plan(&service, &run_id, &span, &patch_type, &replacement, json),
        },
        Commands::Serve { port } => cmd_serve(service, port),
        Commands::Demo => cmd_demo(storage, &service, json),
        Commands::DemoBranch => cmd_demo_branch(storage, &service, json),
    }
}

// ---------------------------------------------------------------------------
// Command implementations
// ---------------------------------------------------------------------------

fn cmd_runs_list<S: Storage, E: replaykit_replay_engine::ExecutorRegistry>(
    service: &ReplayKitService<S, E>,
    json: bool,
) {
    let runs = service.list_runs().expect("list runs");
    if json {
        let views: Vec<RunSummaryView> = runs.iter().map(RunSummaryView::from_record).collect();
        println!("{}", serde_json::to_string_pretty(&views).unwrap());
        return;
    }
    if runs.is_empty() {
        println!("No runs found.");
        return;
    }
    println!(
        "{:<20} {:<30} {:<12} {:>6} {:>6}",
        "RUN ID", "TITLE", "STATUS", "SPANS", "ERRORS"
    );
    println!("{}", "-".repeat(78));
    for run in &runs {
        let badge = status_badge(views::run_status_label(run.status));
        println!(
            "{:<20} {:<30} {:<12} {:>6} {:>6}",
            run.run_id.0,
            truncate(&run.title, 28),
            badge,
            run.summary.span_count,
            run.summary.error_count,
        );
    }
}

fn cmd_runs_show<S: Storage, E: replaykit_replay_engine::ExecutorRegistry>(
    service: &ReplayKitService<S, E>,
    run_id: &str,
    json: bool,
) {
    let run = service.get_run(&RunId(run_id.into())).expect("get run");
    if json {
        let view = RunSummaryView::from_record(&run);
        println!("{}", serde_json::to_string_pretty(&view).unwrap());
        return;
    }
    let view = RunSummaryView::from_record(&run);
    println!("Run:    {}", view.run_id);
    println!("Title:  {}", view.title);
    println!("Status: {}", status_badge(view.status_label));
    println!("Spans:  {}", view.span_count);
    println!("Errors: {}", view.error_count);
    println!("Tokens: {}", view.token_count);
    if let Some(fc) = &view.failure_class {
        println!("Failure: {:?}", fc);
    }
    if let Some(preview) = &view.final_output_preview {
        println!("Output: {}", preview);
    }
    if view.is_branch {
        println!(
            "Branch of: {}",
            view.source_run_id.as_deref().unwrap_or("?")
        );
    }
}

fn cmd_runs_tree<S: Storage, E: replaykit_replay_engine::ExecutorRegistry>(
    service: &ReplayKitService<S, E>,
    run_id: &str,
    json: bool,
) {
    let rid = RunId(run_id.into());
    let run = service.get_run(&rid).expect("get run");
    let tree = service.run_tree(&rid).expect("run tree");

    if json {
        let view = views::RunTreeView {
            run_id: run.run_id.0.clone(),
            title: run.title.clone(),
            status: run.status,
            nodes: tree.iter().map(TreeNodeView::from_tree_node).collect(),
        };
        println!("{}", serde_json::to_string_pretty(&view).unwrap());
        return;
    }

    println!(
        "{} [{}]",
        run.title,
        status_badge(views::run_status_label(run.status))
    );
    for node in &tree {
        print_tree_node(node, "", true);
    }
}

fn cmd_runs_diff<S: Storage, E: replaykit_replay_engine::ExecutorRegistry>(
    service: &ReplayKitService<S, E>,
    source: &str,
    target: &str,
    json: bool,
) {
    let diff = service
        .cached_diff(&RunId(source.into()), &RunId(target.into()))
        .expect("get diff");

    if json {
        let view = RunDiffSummaryView::from_record(&diff);
        println!("{}", serde_json::to_string_pretty(&view).unwrap());
        return;
    }

    let view = RunDiffSummaryView::from_record(&diff);
    println!("Diff: {} -> {}", view.source_run_id, view.target_run_id);
    println!(
        "Source status: {}  Target status: {}",
        status_badge(views::run_status_label(view.source_status)),
        status_badge(views::run_status_label(view.target_status)),
    );
    println!("Changed spans:     {}", view.changed_span_count);
    println!("Changed artifacts: {}", view.changed_artifact_count);
    if let Some(div) = &view.first_divergent_span_id {
        println!("First divergence:  {}", div);
    }
}

fn cmd_span_detail<S: Storage, E: replaykit_replay_engine::ExecutorRegistry>(
    service: &ReplayKitService<S, E>,
    run_id: &str,
    span_id: &str,
    json: bool,
) {
    let span = service
        .get_span(&RunId(run_id.into()), &SpanId(span_id.into()))
        .expect("get span");

    if json {
        let view = SpanDetailView::from_record(&span);
        println!("{}", serde_json::to_string_pretty(&view).unwrap());
        return;
    }

    let view = SpanDetailView::from_record(&span);
    println!("Span:     {}", view.span_id);
    println!("Name:     {}", view.name);
    println!("Kind:     {:?}", view.kind);
    println!("Status:   {}", status_badge(view.status_label));
    if let Some(parent) = &view.parent_span_id {
        println!("Parent:   {}", parent);
    }
    println!("Policy:   {}", view.replay_policy);
    if let Some(ek) = &view.executor_kind {
        println!(
            "Executor: {} {}",
            ek,
            view.executor_version.as_deref().unwrap_or("")
        );
    }
    if let Some(err) = &view.error_summary {
        println!("Error:    {}", err);
    }
    println!(
        "Artifacts: {} in / {} out",
        view.input_artifact_count, view.output_artifact_count
    );
}

fn cmd_span_artifacts<S: Storage, E: replaykit_replay_engine::ExecutorRegistry>(
    service: &ReplayKitService<S, E>,
    run_id: &str,
    span_id: &str,
    json: bool,
) {
    let artifacts = service
        .span_artifacts(&RunId(run_id.into()), &SpanId(span_id.into()))
        .expect("get artifacts");

    if json {
        let views: Vec<ArtifactPreviewView> = artifacts
            .iter()
            .map(ArtifactPreviewView::from_record)
            .collect();
        println!("{}", serde_json::to_string_pretty(&views).unwrap());
        return;
    }

    if artifacts.is_empty() {
        println!("No artifacts.");
        return;
    }
    for a in &artifacts {
        let view = ArtifactPreviewView::from_record(a);
        println!(
            "  {} [{:?}] {} ({} bytes)",
            view.artifact_id, view.artifact_type, view.mime, view.byte_len
        );
    }
}

fn cmd_span_deps<S: Storage, E: replaykit_replay_engine::ExecutorRegistry>(
    service: &ReplayKitService<S, E>,
    run_id: &str,
    span_id: &str,
    json: bool,
) {
    let edges = service
        .span_dependencies(&RunId(run_id.into()), &SpanId(span_id.into()))
        .expect("get deps");

    if json {
        let views: Vec<DependencyView> = edges.iter().map(DependencyView::from_record).collect();
        println!("{}", serde_json::to_string_pretty(&views).unwrap());
        return;
    }

    if edges.is_empty() {
        println!("No dependencies.");
        return;
    }
    for e in &edges {
        let view = DependencyView::from_record(e);
        println!(
            "  {} -> {} [{:?}]",
            view.from_span_id, view.to_span_id, view.kind
        );
    }
}

fn cmd_replay_fork<S: Storage + 'static, E: replaykit_replay_engine::ExecutorRegistry>(
    service: &ReplayKitService<S, E>,
    run_id: &str,
    span_id: &str,
    patch_path: Option<String>,
    replacement_inline: Option<String>,
    patch_type_str: &str,
    json: bool,
) {
    let replacement_text = if let Some(path) = patch_path {
        std::fs::read_to_string(&path).unwrap_or_else(|e| {
            eprintln!("failed to read patch file {path}: {e}");
            std::process::exit(1);
        })
    } else if let Some(text) = replacement_inline {
        text
    } else {
        eprintln!("provide --patch <file> or --replacement <text>");
        std::process::exit(1);
    };

    let patch_type = parse_patch_type(patch_type_str);

    let request = BranchRequest {
        source_run_id: RunId(run_id.into()),
        fork_span_id: SpanId(span_id.into()),
        patch_manifest: PatchManifest {
            patch_type,
            target_artifact_id: None,
            replacement: Value::Text(replacement_text),
            note: None,
            created_at: now_epoch_secs(),
        },
        created_by: Some("cli".into()),
    };

    let execution = service.create_branch(request).expect("create branch");

    if json {
        let view = views::BranchExecutionView::from_parts(
            &execution.branch,
            &execution.target_run,
            &execution.replay_job,
            &execution.plan,
        );
        println!("{}", serde_json::to_string_pretty(&view).unwrap());
        return;
    }

    println!("Branch created:");
    println!("  Source:  {}", execution.branch.source_run_id.0);
    println!("  Target:  {}", execution.branch.target_run_id.0);
    println!(
        "  Status:  {}",
        status_badge(views::run_status_label(execution.target_run.status))
    );
    println!("  Dirty spans:    {}", execution.plan.dirty_spans.len());
    println!("  Blocked spans:  {}", execution.plan.blocked_spans.len());
    println!("  Reusable spans: {}", execution.plan.reusable_spans.len());

    for dirty in &execution.plan.dirty_spans {
        let reasons: Vec<String> = dirty.reasons.iter().map(|r| format!("{r:?}")).collect();
        println!("    {} [{}]", dirty.span_id.0, reasons.join(", "));
    }
}

fn cmd_replay_plan<S: Storage + 'static, E: replaykit_replay_engine::ExecutorRegistry>(
    service: &ReplayKitService<S, E>,
    run_id: &str,
    span_id: &str,
    patch_type_str: &str,
    replacement: &str,
    json: bool,
) {
    let patch_type = parse_patch_type(patch_type_str);

    let request = BranchRequest {
        source_run_id: RunId(run_id.into()),
        fork_span_id: SpanId(span_id.into()),
        patch_manifest: PatchManifest {
            patch_type,
            target_artifact_id: None,
            replacement: Value::Text(replacement.into()),
            note: None,
            created_at: now_epoch_secs(),
        },
        created_by: Some("cli".into()),
    };

    let plan = service.plan_branch(&request).expect("plan branch");

    if json {
        let view = views::BranchPlanView::from_plan(&plan);
        println!("{}", serde_json::to_string_pretty(&view).unwrap());
        return;
    }

    println!(
        "Fork plan for {} at {}",
        plan.source_run_id.0, plan.fork_span_id.0
    );
    println!("  Dirty spans:    {}", plan.dirty_spans.len());
    println!("  Blocked spans:  {}", plan.blocked_spans.len());
    println!("  Reusable spans: {}", plan.reusable_spans.len());
    for dirty in &plan.dirty_spans {
        let reasons: Vec<String> = dirty.reasons.iter().map(|r| format!("{r:?}")).collect();
        println!("    {} [{}]", dirty.span_id.0, reasons.join(", "));
    }
}

fn cmd_serve<S: Storage + 'static, E: replaykit_replay_engine::ExecutorRegistry + 'static>(
    service: Arc<ReplayKitService<S, E>>,
    port: u16,
) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        let router = replaykit_api::server::build_router(service);
        let addr = format!("127.0.0.1:{port}");
        println!("ReplayKit API server listening on http://{addr}");
        let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
        axum::serve(listener, router).await.expect("serve");
    });
}

fn cmd_demo<S: Storage + 'static, E: replaykit_replay_engine::ExecutorRegistry>(
    storage: Arc<S>,
    service: &ReplayKitService<S, E>,
    json: bool,
) {
    let run_id = seed_demo_run(storage).expect("seed demo run");
    if json {
        cmd_runs_show(service, &run_id.0, true);
    } else {
        cmd_runs_tree(service, &run_id.0, false);
    }
}

fn cmd_demo_branch<S: Storage + 'static, E: replaykit_replay_engine::ExecutorRegistry>(
    storage: Arc<S>,
    service: &ReplayKitService<S, E>,
    json: bool,
) {
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

    if json {
        let view = views::BranchExecutionView::from_parts(
            &execution.branch,
            &execution.target_run,
            &execution.replay_job,
            &execution.plan,
        );
        println!("{}", serde_json::to_string_pretty(&view).unwrap());
        return;
    }

    println!("Source run: {}", run_id.0);
    println!("Branch run: {}", execution.target_run.run_id.0);
    println!(
        "Branch status: {}",
        status_badge(views::run_status_label(execution.target_run.status))
    );
    println!("Dirty spans:");
    for dirty in &execution.plan.dirty_spans {
        let reasons: Vec<String> = dirty.reasons.iter().map(|r| format!("{r:?}")).collect();
        println!("  {} [{}]", dirty.span_id.0, reasons.join(", "));
    }
    let diff = service
        .cached_diff(
            &execution.branch.source_run_id,
            &execution.branch.target_run_id,
        )
        .expect("cached diff");
    println!(
        "Diff: changed_spans={} first_divergent={}",
        diff.changed_span_count,
        diff.first_divergent_span_id
            .map(|id| id.0)
            .unwrap_or_else(|| "none".into())
    );
}

// ---------------------------------------------------------------------------
// Tree rendering
// ---------------------------------------------------------------------------

fn print_tree_node(node: &RunTreeNode, prefix: &str, is_last: bool) {
    let connector = if is_last { "└── " } else { "├── " };
    let kind_label = span_kind_short(node.span.kind);
    let status = status_badge(views::span_status_label(node.span.status));

    let error_hint = node
        .span
        .error_summary
        .as_ref()
        .map(|e| format!(" -- {e}"))
        .unwrap_or_default();

    println!(
        "{prefix}{connector}{} [{kind_label}] {status}{error_hint}",
        node.span.name,
    );

    let child_prefix = format!("{prefix}{}", if is_last { "    " } else { "│   " });
    let count = node.children.len();
    for (i, child) in node.children.iter().enumerate() {
        print_tree_node(child, &child_prefix, i == count - 1);
    }
}

fn span_kind_short(kind: SpanKind) -> &'static str {
    match kind {
        SpanKind::Run => "run",
        SpanKind::PlannerStep => "plan",
        SpanKind::LlmCall => "llm",
        SpanKind::ToolCall => "tool",
        SpanKind::ShellCommand => "shell",
        SpanKind::FileRead => "fread",
        SpanKind::FileWrite => "fwrite",
        SpanKind::BrowserAction => "browser",
        SpanKind::Retrieval => "retrieval",
        SpanKind::MemoryLookup => "memory",
        SpanKind::HumanInput => "human",
        SpanKind::GuardrailCheck => "guard",
        SpanKind::Subgraph => "sub",
        SpanKind::AdapterInternal => "adapter",
    }
}

fn status_badge(label: &str) -> String {
    match label {
        "completed" => format!("\x1b[32m{label}\x1b[0m"), // green
        "failed" => format!("\x1b[31m{label}\x1b[0m"),    // red
        "blocked" => format!("\x1b[33m{label}\x1b[0m"),   // yellow
        "running" => format!("\x1b[36m{label}\x1b[0m"),   // cyan
        "canceled" => format!("\x1b[90m{label}\x1b[0m"),  // gray
        "interrupted" => format!("\x1b[33m{label}\x1b[0m"), // yellow
        _ => label.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

fn parse_patch_type(s: &str) -> PatchType {
    match s {
        "prompt_edit" => PatchType::PromptEdit,
        "tool_output_override" => PatchType::ToolOutputOverride,
        "env_var_override" => PatchType::EnvVarOverride,
        "model_config_edit" => PatchType::ModelConfigEdit,
        "retrieval_context_override" => PatchType::RetrievalContextOverride,
        "snapshot_override" => PatchType::SnapshotOverride,
        other => {
            eprintln!("unknown patch type: {other}");
            std::process::exit(1);
        }
    }
}

fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn seed_demo_run<S: Storage>(storage: Arc<S>) -> Result<RunId, String> {
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
