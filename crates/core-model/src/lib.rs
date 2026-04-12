use std::collections::BTreeMap;
use std::fmt;

pub type Document = BTreeMap<String, Value>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TraceId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpanId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ArtifactId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SnapshotId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EdgeId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BranchId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReplayJobId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DiffId(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Text(String),
    Array(Vec<Value>),
    Object(Document),
}

impl Default for Value {
    fn default() -> Self {
        Self::Null
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayPolicy {
    RecordOnly,
    RerunnableSupported,
    CacheableIfFingerprintMatches,
    PureReusable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Completed,
    Failed,
    Interrupted,
    Canceled,
    Blocked,
    Imported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanStatus {
    Running,
    Completed,
    Failed,
    Skipped,
    Blocked,
    Canceled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchType {
    PromptEdit,
    ToolOutputOverride,
    EnvVarOverride,
    ModelConfigEdit,
    RetrievalContextOverride,
    SnapshotOverride,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayMode {
    Recorded,
    Forked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayJobStatus {
    Queued,
    Validating,
    Running,
    Blocked,
    Failed,
    Completed,
    Canceled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirtyReason {
    PatchedInput,
    FingerprintChanged,
    UpstreamOutputChanged,
    ExecutorVersionChanged,
    PolicyForcedRerun,
    MissingReusableArtifact,
    DependencyUnknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostMetadata {
    pub os: String,
    pub arch: String,
    pub hostname: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CostMetrics {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost_micros: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunSummary {
    pub span_count: u64,
    pub artifact_count: u64,
    pub error_count: u64,
    pub token_count: u64,
    pub estimated_cost_micros: u64,
    pub final_output_preview: Option<String>,
    pub failure_class: Option<FailureClass>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventRecord {
    pub event_id: EventId,
    pub run_id: RunId,
    pub span_id: SpanId,
    pub sequence_no: u64,
    pub timestamp: u64,
    pub kind: String,
    pub payload: Document,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotRecord {
    pub snapshot_id: SnapshotId,
    pub run_id: RunId,
    pub span_id: Option<SpanId>,
    pub kind: String,
    pub artifact_id: ArtifactId,
    pub summary: Document,
    pub created_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpanEdgeRecord {
    pub edge_id: EdgeId,
    pub run_id: RunId,
    pub from_span_id: SpanId,
    pub to_span_id: SpanId,
    pub kind: EdgeKind,
    pub attributes: Document,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchManifest {
    pub patch_type: PatchType,
    pub target_artifact_id: Option<ArtifactId>,
    pub replacement: Value,
    pub note: Option<String>,
    pub created_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirtySpanRecord {
    pub span_id: SpanId,
    pub reasons: Vec<DirtyReason>,
    pub triggered_by: Vec<SpanId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchRequest {
    pub source_run_id: RunId,
    pub fork_span_id: SpanId,
    pub patch_manifest: PatchManifest,
    pub created_by: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchPlan {
    pub source_run_id: RunId,
    pub fork_span_id: SpanId,
    pub dirty_spans: Vec<DirtySpanRecord>,
    pub blocked_spans: Vec<SpanId>,
    pub reusable_spans: Vec<SpanId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

impl SpanRecord {
    pub fn is_terminal(&self) -> bool {
        self.ended_at.is_some()
    }
}
