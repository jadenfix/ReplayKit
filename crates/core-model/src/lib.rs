use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

pub type Document = BTreeMap<String, Value>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RunId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TraceId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SpanId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EventId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ArtifactId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SnapshotId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EdgeId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BranchId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ReplayJobId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DiffId(pub String);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Int(i64),
    Text(String),
    Array(Vec<Value>),
    Object(Document),
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => write!(f, "null"),
            Value::Bool(v) => write!(f, "{v}"),
            Value::Int(v) => write!(f, "{v}"),
            Value::Text(v) => write!(f, "{v}"),
            Value::Array(values) => write!(f, "array({})", values.len()),
            Value::Object(values) => write!(f, "object({})", values.len()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpanKind {
    Run,
    PlannerStep,
    LlmCall,
    ToolCall,
    ShellCommand,
    FileRead,
    FileWrite,
    BrowserAction,
    Retrieval,
    MemoryLookup,
    HumanInput,
    GuardrailCheck,
    Subgraph,
    AdapterInternal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeKind {
    ControlParent,
    DataDependsOn,
    RetryOf,
    Replaces,
    BranchOf,
    MaterializesSnapshot,
    ReadsArtifact,
    WritesArtifact,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplayPolicy {
    RecordOnly,
    RerunnableSupported,
    CacheableIfFingerprintMatches,
    PureReusable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunStatus {
    Running,
    Completed,
    Failed,
    Interrupted,
    Canceled,
    Blocked,
    Imported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpanStatus {
    Running,
    Completed,
    Failed,
    Skipped,
    Blocked,
    Canceled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PatchType {
    PromptEdit,
    ToolOutputOverride,
    EnvVarOverride,
    ModelConfigEdit,
    RetrievalContextOverride,
    SnapshotOverride,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplayMode {
    Recorded,
    Forked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplayJobStatus {
    Queued,
    Validating,
    Running,
    Blocked,
    Failed,
    Completed,
    Canceled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirtyReason {
    PatchedInput,
    FingerprintChanged,
    UpstreamOutputChanged,
    ExecutorVersionChanged,
    PolicyForcedRerun,
    MissingReusableArtifact,
    DependencyUnknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureClass {
    ModelFailure,
    ToolFailure,
    ShellFailure,
    FileSystemFailure,
    RetrievalFailure,
    BrowserFailure,
    GuardrailFailure,
    HumanDependency,
    ReplayUnsupported,
    IntegrityFailure,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactType {
    Prompt,
    ModelRequest,
    ModelResponse,
    ToolInput,
    ToolOutput,
    ShellStdout,
    ShellStderr,
    FileDiff,
    FileBlob,
    DomSnapshot,
    Screenshot,
    StateSnapshot,
    RetrievalResult,
    MemoryState,
    HumanMessage,
    DebugLog,
    PatchManifest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum IdKind {
    Run,
    Trace,
    Span,
    Event,
    Artifact,
    Snapshot,
    Edge,
    Branch,
    ReplayJob,
    Diff,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostMetadata {
    pub os: String,
    pub arch: String,
    pub hostname: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostMetrics {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost_micros: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummary {
    pub span_count: u64,
    pub artifact_count: u64,
    pub error_count: u64,
    pub token_count: u64,
    pub estimated_cost_micros: u64,
    pub final_output_preview: Option<String>,
    pub failure_class: Option<FailureClass>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: RunId,
    pub trace_id: TraceId,
    pub source_run_id: Option<RunId>,
    pub root_span_id: Option<SpanId>,
    pub branch_id: Option<BranchId>,
    pub title: String,
    pub entrypoint: String,
    pub adapter_name: String,
    pub adapter_version: String,
    pub status: RunStatus,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    pub git_sha: Option<String>,
    pub environment_fingerprint: Option<String>,
    pub host: HostMetadata,
    pub labels: Vec<String>,
    pub summary: RunSummary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanRecord {
    pub run_id: RunId,
    pub span_id: SpanId,
    pub trace_id: TraceId,
    pub parent_span_id: Option<SpanId>,
    pub sequence_no: u64,
    pub kind: SpanKind,
    pub name: String,
    pub status: SpanStatus,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    pub replay_policy: ReplayPolicy,
    pub executor_kind: Option<String>,
    pub executor_version: Option<String>,
    pub input_artifact_ids: Vec<ArtifactId>,
    pub output_artifact_ids: Vec<ArtifactId>,
    pub snapshot_id: Option<SnapshotId>,
    pub input_fingerprint: Option<String>,
    pub output_fingerprint: Option<String>,
    pub environment_fingerprint: Option<String>,
    pub attributes: Document,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    pub cost: CostMetrics,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRecord {
    pub event_id: EventId,
    pub run_id: RunId,
    pub span_id: SpanId,
    pub sequence_no: u64,
    pub timestamp: u64,
    pub kind: String,
    pub payload: Document,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub artifact_id: ArtifactId,
    pub run_id: RunId,
    pub span_id: Option<SpanId>,
    pub artifact_type: ArtifactType,
    pub mime: String,
    pub sha256: String,
    pub byte_len: usize,
    pub blob_path: String,
    pub summary: Document,
    pub redaction: Document,
    pub created_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRecord {
    pub snapshot_id: SnapshotId,
    pub run_id: RunId,
    pub span_id: Option<SpanId>,
    pub kind: String,
    pub artifact_id: ArtifactId,
    pub summary: Document,
    pub created_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanEdgeRecord {
    pub edge_id: EdgeId,
    pub run_id: RunId,
    pub from_span_id: SpanId,
    pub to_span_id: SpanId,
    pub kind: EdgeKind,
    pub attributes: Document,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchManifest {
    pub patch_type: PatchType,
    pub target_artifact_id: Option<ArtifactId>,
    pub replacement: Value,
    pub note: Option<String>,
    pub created_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchRecord {
    pub branch_id: BranchId,
    pub source_run_id: RunId,
    pub target_run_id: RunId,
    pub fork_span_id: SpanId,
    pub patch_manifest_artifact_id: ArtifactId,
    pub created_at: u64,
    pub created_by: Option<String>,
    pub status: RunStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayJobRecord {
    pub replay_job_id: ReplayJobId,
    pub source_run_id: RunId,
    pub target_run_id: Option<RunId>,
    pub mode: ReplayMode,
    pub status: ReplayJobStatus,
    pub created_at: u64,
    pub started_at: Option<u64>,
    pub ended_at: Option<u64>,
    pub progress: Document,
    pub error_summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirtySpanRecord {
    pub span_id: SpanId,
    pub reasons: Vec<DirtyReason>,
    pub triggered_by: Vec<SpanId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunDiffRecord {
    pub diff_id: DiffId,
    pub source_run_id: RunId,
    pub target_run_id: RunId,
    pub first_divergent_span_id: Option<SpanId>,
    pub changed_span_count: usize,
    pub changed_artifact_count: usize,
    pub source_status: RunStatus,
    pub target_status: RunStatus,
    pub summary: Document,
    pub created_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchRequest {
    pub source_run_id: RunId,
    pub fork_span_id: SpanId,
    pub patch_manifest: PatchManifest,
    pub created_by: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchPlan {
    pub source_run_id: RunId,
    pub fork_span_id: SpanId,
    pub dirty_spans: Vec<DirtySpanRecord>,
    pub blocked_spans: Vec<SpanId>,
    pub reusable_spans: Vec<SpanId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTreeNode {
    pub span: SpanRecord,
    pub children: Vec<RunTreeNode>,
}

impl RunRecord {
    pub fn new(
        run_id: RunId,
        trace_id: TraceId,
        title: impl Into<String>,
        entrypoint: impl Into<String>,
        adapter_name: impl Into<String>,
        adapter_version: impl Into<String>,
        started_at: u64,
    ) -> Self {
        Self {
            run_id,
            trace_id,
            source_run_id: None,
            root_span_id: None,
            branch_id: None,
            title: title.into(),
            entrypoint: entrypoint.into(),
            adapter_name: adapter_name.into(),
            adapter_version: adapter_version.into(),
            status: RunStatus::Running,
            started_at,
            ended_at: None,
            git_sha: None,
            environment_fingerprint: None,
            host: HostMetadata::default(),
            labels: Vec::new(),
            summary: RunSummary::default(),
        }
    }
}

impl RunSummary {
    pub fn from_run_state(
        run_status: RunStatus,
        spans: &[SpanRecord],
        artifacts: &[ArtifactRecord],
        final_output_preview: Option<String>,
    ) -> Self {
        let mut summary = Self {
            span_count: spans.len() as u64,
            artifact_count: artifacts.len() as u64,
            error_count: 0,
            token_count: 0,
            estimated_cost_micros: 0,
            final_output_preview,
            failure_class: None,
        };

        for span in spans {
            summary.token_count = summary.token_count.saturating_add(
                span.cost
                    .input_tokens
                    .saturating_add(span.cost.output_tokens),
            );
            summary.estimated_cost_micros = summary
                .estimated_cost_micros
                .saturating_add(span.cost.estimated_cost_micros);
            if span_contributes_error(span) {
                summary.error_count = summary.error_count.saturating_add(1);
            }
        }

        summary.failure_class = derived_failure_class(run_status, spans);
        if summary.error_count == 0 && run_status_implies_error(run_status) {
            summary.error_count = 1;
        }

        summary
    }
}

impl SpanRecord {
    pub fn is_terminal(&self) -> bool {
        self.ended_at.is_some()
    }
}

impl IdKind {
    pub fn prefix(self) -> &'static str {
        match self {
            IdKind::Run => "run",
            IdKind::Trace => "trace",
            IdKind::Span => "span",
            IdKind::Event => "event",
            IdKind::Artifact => "artifact",
            IdKind::Snapshot => "snapshot",
            IdKind::Edge => "edge",
            IdKind::Branch => "branch",
            IdKind::ReplayJob => "job",
            IdKind::Diff => "diff",
        }
    }
}

fn span_contributes_error(span: &SpanRecord) -> bool {
    matches!(
        span.status,
        SpanStatus::Failed | SpanStatus::Blocked | SpanStatus::Canceled
    )
}

fn run_status_implies_error(status: RunStatus) -> bool {
    matches!(
        status,
        RunStatus::Failed | RunStatus::Interrupted | RunStatus::Canceled | RunStatus::Blocked
    )
}

fn derived_failure_class(run_status: RunStatus, spans: &[SpanRecord]) -> Option<FailureClass> {
    spans
        .iter()
        .filter(|span| span_contributes_error(span))
        .max_by_key(|span| {
            let class = classify_failure(span);
            (failure_specificity(class), span.sequence_no)
        })
        .map(classify_failure)
        .or_else(|| run_status_implies_error(run_status).then_some(FailureClass::Unknown))
}

fn classify_failure(span: &SpanRecord) -> FailureClass {
    if span.status == SpanStatus::Blocked
        && span
            .error_summary
            .as_deref()
            .is_some_and(|summary| summary.starts_with("replay blocked"))
    {
        return FailureClass::ReplayUnsupported;
    }

    match span.kind {
        SpanKind::LlmCall => FailureClass::ModelFailure,
        SpanKind::ToolCall => FailureClass::ToolFailure,
        SpanKind::ShellCommand => FailureClass::ShellFailure,
        SpanKind::FileRead | SpanKind::FileWrite => FailureClass::FileSystemFailure,
        SpanKind::BrowserAction => FailureClass::BrowserFailure,
        SpanKind::Retrieval | SpanKind::MemoryLookup => FailureClass::RetrievalFailure,
        SpanKind::HumanInput => FailureClass::HumanDependency,
        SpanKind::GuardrailCheck => FailureClass::GuardrailFailure,
        SpanKind::Run | SpanKind::PlannerStep | SpanKind::Subgraph | SpanKind::AdapterInternal => {
            FailureClass::Unknown
        }
    }
}

fn failure_specificity(class: FailureClass) -> u8 {
    match class {
        FailureClass::Unknown => 0,
        FailureClass::ReplayUnsupported => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(kind: SpanKind, sequence_no: u64, status: SpanStatus) -> SpanRecord {
        SpanRecord {
            run_id: RunId("run-1".into()),
            span_id: SpanId(format!("span-{sequence_no}")),
            trace_id: TraceId("trace-1".into()),
            parent_span_id: None,
            sequence_no,
            kind,
            name: format!("span-{sequence_no}"),
            status,
            started_at: sequence_no,
            ended_at: Some(sequence_no + 1),
            replay_policy: ReplayPolicy::RecordOnly,
            executor_kind: None,
            executor_version: None,
            input_artifact_ids: Vec::new(),
            output_artifact_ids: Vec::new(),
            snapshot_id: None,
            input_fingerprint: None,
            output_fingerprint: None,
            environment_fingerprint: None,
            attributes: Document::new(),
            error_code: None,
            error_summary: None,
            cost: CostMetrics::default(),
        }
    }

    #[test]
    fn run_summary_aggregates_costs_and_failure_class() {
        let mut completed = span(SpanKind::PlannerStep, 1, SpanStatus::Completed);
        completed.cost = CostMetrics {
            input_tokens: 3,
            output_tokens: 5,
            estimated_cost_micros: 7,
        };

        let mut failed = span(SpanKind::ToolCall, 2, SpanStatus::Failed);
        failed.cost = CostMetrics {
            input_tokens: 11,
            output_tokens: 13,
            estimated_cost_micros: 17,
        };
        failed.error_summary = Some("tool blew up".into());

        let summary = RunSummary::from_run_state(
            RunStatus::Failed,
            &[completed, failed],
            &[ArtifactRecord {
                artifact_id: ArtifactId("artifact-1".into()),
                run_id: RunId("run-1".into()),
                span_id: None,
                artifact_type: ArtifactType::ToolOutput,
                mime: "application/json".into(),
                sha256: "artifact".into(),
                byte_len: 1,
                blob_path: "memory://artifact".into(),
                summary: Document::new(),
                redaction: Document::new(),
                created_at: 1,
            }],
            Some("failed".into()),
        );

        assert_eq!(summary.span_count, 2);
        assert_eq!(summary.artifact_count, 1);
        assert_eq!(summary.error_count, 1);
        assert_eq!(summary.token_count, 32);
        assert_eq!(summary.estimated_cost_micros, 24);
        assert_eq!(summary.failure_class, Some(FailureClass::ToolFailure));
        assert_eq!(summary.final_output_preview.as_deref(), Some("failed"));
    }

    #[test]
    fn run_summary_marks_unknown_for_interrupted_run_without_failed_spans() {
        let summary = RunSummary::from_run_state(
            RunStatus::Interrupted,
            &[span(SpanKind::Run, 1, SpanStatus::Completed)],
            &[],
            Some("aborted".into()),
        );

        assert_eq!(summary.error_count, 1);
        assert_eq!(summary.failure_class, Some(FailureClass::Unknown));
        assert_eq!(summary.final_output_preview.as_deref(), Some("aborted"));
    }
}
