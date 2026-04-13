use replaykit_core_model::{
    ArtifactRecord, ArtifactType, BranchPlan, BranchRecord, CostMetrics, DirtySpanRecord, Document,
    EdgeKind, FailureClass, ReplayJobRecord, ReplayJobStatus, ReplayMode, RunDiffRecord, RunRecord,
    RunStatus, RunTreeNode, SpanEdgeRecord, SpanKind, SpanRecord, SpanStatus,
};
use serde::Serialize;

// ---------------------------------------------------------------------------
// Run views
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct RunSummaryView {
    pub run_id: String,
    pub title: String,
    pub status: RunStatus,
    pub status_label: &'static str,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    pub span_count: u64,
    pub error_count: u64,
    pub token_count: u64,
    pub estimated_cost_micros: u64,
    pub failure_class: Option<FailureClass>,
    pub final_output_preview: Option<String>,
    pub is_branch: bool,
    pub source_run_id: Option<String>,
}

impl RunSummaryView {
    pub fn from_record(r: &RunRecord) -> Self {
        Self {
            run_id: r.run_id.0.clone(),
            title: r.title.clone(),
            status: r.status,
            status_label: run_status_label(r.status),
            started_at: r.started_at,
            ended_at: r.ended_at,
            span_count: r.summary.span_count,
            error_count: r.summary.error_count,
            token_count: r.summary.token_count,
            estimated_cost_micros: r.summary.estimated_cost_micros,
            failure_class: r.summary.failure_class,
            final_output_preview: r.summary.final_output_preview.clone(),
            is_branch: r.source_run_id.is_some(),
            source_run_id: r.source_run_id.as_ref().map(|id| id.0.clone()),
        }
    }
}

// ---------------------------------------------------------------------------
// Run tree views
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct RunTreeView {
    pub run_id: String,
    pub title: String,
    pub status: RunStatus,
    pub nodes: Vec<TreeNodeView>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TreeNodeView {
    pub span_id: String,
    pub name: String,
    pub kind: SpanKind,
    pub status: SpanStatus,
    pub status_label: &'static str,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    pub error_summary: Option<String>,
    pub child_count: usize,
    pub children: Vec<TreeNodeView>,
}

impl TreeNodeView {
    pub fn from_tree_node(node: &RunTreeNode) -> Self {
        let children: Vec<TreeNodeView> = node
            .children
            .iter()
            .map(TreeNodeView::from_tree_node)
            .collect();
        Self {
            span_id: node.span.span_id.0.clone(),
            name: node.span.name.clone(),
            kind: node.span.kind,
            status: node.span.status,
            status_label: span_status_label(node.span.status),
            started_at: node.span.started_at,
            ended_at: node.span.ended_at,
            error_summary: node.span.error_summary.clone(),
            child_count: children.len(),
            children,
        }
    }
}

// ---------------------------------------------------------------------------
// Span detail view
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct SpanDetailView {
    pub span_id: String,
    pub run_id: String,
    pub parent_span_id: Option<String>,
    pub name: String,
    pub kind: SpanKind,
    pub status: SpanStatus,
    pub status_label: &'static str,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    pub replay_policy: String,
    pub executor_kind: Option<String>,
    pub executor_version: Option<String>,
    pub input_fingerprint: Option<String>,
    pub output_fingerprint: Option<String>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    pub cost: CostMetrics,
    pub input_artifact_count: usize,
    pub output_artifact_count: usize,
    pub attributes: Document,
}

impl SpanDetailView {
    pub fn from_record(s: &SpanRecord) -> Self {
        Self {
            span_id: s.span_id.0.clone(),
            run_id: s.run_id.0.clone(),
            parent_span_id: s.parent_span_id.as_ref().map(|id| id.0.clone()),
            name: s.name.clone(),
            kind: s.kind,
            status: s.status,
            status_label: span_status_label(s.status),
            started_at: s.started_at,
            ended_at: s.ended_at,
            replay_policy: format!("{:?}", s.replay_policy),
            executor_kind: s.executor_kind.clone(),
            executor_version: s.executor_version.clone(),
            input_fingerprint: s.input_fingerprint.clone(),
            output_fingerprint: s.output_fingerprint.clone(),
            error_code: s.error_code.clone(),
            error_summary: s.error_summary.clone(),
            cost: s.cost.clone(),
            input_artifact_count: s.input_artifact_ids.len(),
            output_artifact_count: s.output_artifact_ids.len(),
            attributes: s.attributes.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Artifact preview view
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct ArtifactPreviewView {
    pub artifact_id: String,
    pub artifact_type: ArtifactType,
    pub mime: String,
    pub byte_len: usize,
    pub summary: Document,
    pub created_at: u64,
}

impl ArtifactPreviewView {
    pub fn from_record(a: &ArtifactRecord) -> Self {
        Self {
            artifact_id: a.artifact_id.0.clone(),
            artifact_type: a.artifact_type,
            mime: a.mime.clone(),
            byte_len: a.byte_len,
            summary: a.summary.clone(),
            created_at: a.created_at,
        }
    }
}

// ---------------------------------------------------------------------------
// Dependency view
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct DependencyView {
    pub edge_id: String,
    pub from_span_id: String,
    pub to_span_id: String,
    pub kind: EdgeKind,
}

impl DependencyView {
    pub fn from_record(e: &SpanEdgeRecord) -> Self {
        Self {
            edge_id: e.edge_id.0.clone(),
            from_span_id: e.from_span_id.0.clone(),
            to_span_id: e.to_span_id.0.clone(),
            kind: e.kind,
        }
    }
}

// ---------------------------------------------------------------------------
// Replay job view
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct ReplayJobView {
    pub replay_job_id: String,
    pub source_run_id: String,
    pub target_run_id: Option<String>,
    pub mode: ReplayMode,
    pub status: ReplayJobStatus,
    pub status_label: &'static str,
    pub created_at: u64,
    pub started_at: Option<u64>,
    pub ended_at: Option<u64>,
    pub progress: Document,
    pub error_summary: Option<String>,
}

impl ReplayJobView {
    pub fn from_record(j: &ReplayJobRecord) -> Self {
        Self {
            replay_job_id: j.replay_job_id.0.clone(),
            source_run_id: j.source_run_id.0.clone(),
            target_run_id: j.target_run_id.as_ref().map(|id| id.0.clone()),
            mode: j.mode,
            status: j.status,
            status_label: replay_job_status_label(j.status),
            created_at: j.created_at,
            started_at: j.started_at,
            ended_at: j.ended_at,
            progress: j.progress.clone(),
            error_summary: j.error_summary.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Diff summary view
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct RunDiffSummaryView {
    pub diff_id: String,
    pub source_run_id: String,
    pub target_run_id: String,
    pub source_status: RunStatus,
    pub target_status: RunStatus,
    pub changed_span_count: usize,
    pub changed_artifact_count: usize,
    pub first_divergent_span_id: Option<String>,
    pub summary: Document,
}

impl RunDiffSummaryView {
    pub fn from_record(d: &RunDiffRecord) -> Self {
        Self {
            diff_id: d.diff_id.0.clone(),
            source_run_id: d.source_run_id.0.clone(),
            target_run_id: d.target_run_id.0.clone(),
            source_status: d.source_status,
            target_status: d.target_status,
            changed_span_count: d.changed_span_count,
            changed_artifact_count: d.changed_artifact_count,
            first_divergent_span_id: d.first_divergent_span_id.as_ref().map(|id| id.0.clone()),
            summary: d.summary.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Branch execution view (returned from create-branch command)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct BranchExecutionView {
    pub branch_id: String,
    pub source_run_id: String,
    pub target_run_id: String,
    pub target_status: RunStatus,
    pub target_status_label: &'static str,
    pub replay_job: ReplayJobView,
    pub dirty_span_count: usize,
    pub blocked_span_count: usize,
    pub reusable_span_count: usize,
}

impl BranchExecutionView {
    pub fn from_parts(
        branch: &BranchRecord,
        target_run: &RunRecord,
        job: &ReplayJobRecord,
        plan: &BranchPlan,
    ) -> Self {
        Self {
            branch_id: branch.branch_id.0.clone(),
            source_run_id: branch.source_run_id.0.clone(),
            target_run_id: branch.target_run_id.0.clone(),
            target_status: target_run.status,
            target_status_label: run_status_label(target_run.status),
            replay_job: ReplayJobView::from_record(job),
            dirty_span_count: plan.dirty_spans.len(),
            blocked_span_count: plan.blocked_spans.len(),
            reusable_span_count: plan.reusable_spans.len(),
        }
    }
}

// ---------------------------------------------------------------------------
// Branch plan view
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct BranchPlanView {
    pub source_run_id: String,
    pub fork_span_id: String,
    pub dirty_spans: Vec<DirtySpanView>,
    pub blocked_span_ids: Vec<String>,
    pub reusable_span_ids: Vec<String>,
}

impl BranchPlanView {
    pub fn from_plan(p: &BranchPlan) -> Self {
        Self {
            source_run_id: p.source_run_id.0.clone(),
            fork_span_id: p.fork_span_id.0.clone(),
            dirty_spans: p
                .dirty_spans
                .iter()
                .map(DirtySpanView::from_record)
                .collect(),
            blocked_span_ids: p.blocked_spans.iter().map(|id| id.0.clone()).collect(),
            reusable_span_ids: p.reusable_spans.iter().map(|id| id.0.clone()).collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct DirtySpanView {
    pub span_id: String,
    pub reasons: Vec<String>,
    pub triggered_by: Vec<String>,
}

impl DirtySpanView {
    pub fn from_record(d: &DirtySpanRecord) -> Self {
        Self {
            span_id: d.span_id.0.clone(),
            reasons: d.reasons.iter().map(|r| format!("{r:?}")).collect(),
            triggered_by: d.triggered_by.iter().map(|id| id.0.clone()).collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Label helpers
// ---------------------------------------------------------------------------

pub fn run_status_label(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Running => "running",
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Interrupted => "interrupted",
        RunStatus::Canceled => "canceled",
        RunStatus::Blocked => "blocked",
        RunStatus::Imported => "imported",
    }
}

pub fn span_status_label(s: SpanStatus) -> &'static str {
    match s {
        SpanStatus::Running => "running",
        SpanStatus::Completed => "completed",
        SpanStatus::Failed => "failed",
        SpanStatus::Skipped => "skipped",
        SpanStatus::Blocked => "blocked",
        SpanStatus::Canceled => "canceled",
    }
}

fn replay_job_status_label(s: ReplayJobStatus) -> &'static str {
    match s {
        ReplayJobStatus::Queued => "queued",
        ReplayJobStatus::Validating => "validating",
        ReplayJobStatus::Running => "running",
        ReplayJobStatus::Blocked => "blocked",
        ReplayJobStatus::Failed => "failed",
        ReplayJobStatus::Completed => "completed",
        ReplayJobStatus::Canceled => "canceled",
    }
}
