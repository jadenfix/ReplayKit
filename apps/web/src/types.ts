// ── ReplayKit Domain Types ──────────────────────────────────────────
// Mirrors the canonical model from architecture.md.
// Uses string unions (not TS enums) for erasableSyntaxOnly compat.

export type RunStatus =
  | 'Running' | 'Completed' | 'Failed'
  | 'Interrupted' | 'Canceled' | 'Blocked' | 'Imported';

export type SpanStatus =
  | 'Running' | 'Completed' | 'Failed'
  | 'Skipped' | 'Blocked' | 'Canceled';

export type SpanKind =
  | 'Run' | 'PlannerStep' | 'LlmCall' | 'ToolCall'
  | 'ShellCommand' | 'FileRead' | 'FileWrite' | 'BrowserAction'
  | 'Retrieval' | 'MemoryLookup' | 'HumanInput' | 'GuardrailCheck'
  | 'Subgraph' | 'AdapterInternal';

export type ReplayPolicy =
  | 'RecordOnly' | 'RerunnableSupported'
  | 'CacheableIfFingerprintMatches' | 'PureReusable';

export type EdgeKind =
  | 'ControlParent' | 'DataDependsOn' | 'RetryOf'
  | 'Replaces' | 'BranchOf' | 'MaterializesSnapshot'
  | 'ReadsArtifact' | 'WritesArtifact';

export type PatchType =
  | 'PromptEdit' | 'ToolOutputOverride' | 'EnvVarOverride'
  | 'ModelConfigEdit' | 'RetrievalContextOverride' | 'SnapshotOverride';

export type DirtyReason =
  | 'PatchedInput' | 'FingerprintChanged' | 'UpstreamOutputChanged'
  | 'ExecutorVersionChanged' | 'PolicyForcedRerun'
  | 'MissingReusableArtifact' | 'DependencyUnknown';

export type FailureClass =
  | 'ModelFailure' | 'ToolFailure' | 'ShellFailure'
  | 'FileSystemFailure' | 'RetrievalFailure' | 'BrowserFailure'
  | 'GuardrailFailure' | 'HumanDependency' | 'ReplayUnsupported'
  | 'IntegrityFailure' | 'Unknown';

// ── Records ─────────────────────────────────────────────────────────

export interface RunRecord {
  run_id: string;
  trace_id: string;
  source_run_id: string | null;
  title: string;
  entrypoint: string;
  adapter_name: string;
  adapter_version: string;
  status: RunStatus;
  started_at: number;
  ended_at: number | null;
  git_sha: string | null;
  environment_fingerprint: string | null;
  labels: string[];
}

export interface SpanRecord {
  run_id: string;
  span_id: string;
  trace_id: string;
  parent_span_id: string | null;
  sequence_no: number;
  kind: SpanKind;
  name: string;
  status: SpanStatus;
  started_at: number;
  ended_at: number | null;
  replay_policy: ReplayPolicy;
  executor_kind: string | null;
  executor_version: string | null;
  input_artifact_ids: string[];
  output_artifact_ids: string[];
  snapshot_id: string | null;
  input_fingerprint: string | null;
  environment_fingerprint: string | null;
  error_code: string | null;
  error_summary: string | null;
  failure_class: FailureClass | null;
  dirty_reasons: DirtyReason[];
  blocked_replay_reason: string | null;
  attributes: Record<string, unknown>;
}

export interface ArtifactRecord {
  artifact_id: string;
  run_id: string;
  span_id: string | null;
  type: string;
  mime: string;
  byte_len: number;
  summary: string | null;
  content: string;
}

export interface SpanEdgeRecord {
  run_id: string;
  from_span_id: string;
  to_span_id: string;
  kind: EdgeKind;
}

export interface BranchRecord {
  branch_id: string;
  source_run_id: string;
  target_run_id: string;
  fork_span_id: string;
  patch_type: PatchType;
  patch_summary: string;
  created_at: number;
  status: RunStatus;
}

// ── View Models ─────────────────────────────────────────────────────

export interface RunListItem {
  run_id: string;
  title: string;
  status: RunStatus;
  started_at: number;
  duration_ms: number | null;
  adapter_name: string;
  failure_summary: string | null;
  source_run_id: string | null;
  span_count: number;
  error_count: number;
}

export interface SpanTreeNode {
  span: SpanRecord;
  children: SpanTreeNode[];
  depth: number;
}

export interface DiffSummary {
  diff_id: string;
  source_run_id: string;
  target_run_id: string;
  first_divergent_span_id: string | null;
  status_change: { from: RunStatus; to: RunStatus } | null;
  latency_ms_delta: number | null;
  token_delta: number | null;
  changed_span_count: number;
  changed_artifact_count: number;
  final_output_changed: boolean;
  span_diffs: SpanDiff[];
}

export interface SpanDiff {
  span_id_source: string;
  span_id_target: string;
  name: string;
  status_change: { from: SpanStatus; to: SpanStatus } | null;
  duration_ms_delta: number | null;
  output_changed: boolean;
  dirty_reason: DirtyReason | null;
}

// ── Timeline ───────────────────────────────────────────────────────

export interface TimelineView {
  run_id: string;
  title: string;
  status: RunStatus;
  total_started_at: number;
  total_ended_at: number | null;
  entries: TimelineEntryView[];
}

export interface TimelineEntryView {
  span_id: string;
  name: string;
  kind: SpanKind;
  status: SpanStatus;
  status_label: string;
  started_at: number;
  ended_at: number | null;
  depth: number;
  parent_span_id: string | null;
  error_summary: string | null;
}

// ── Forensics ──────────────────────────────────────────────────────

export interface ForensicsReport {
  run_id: string;
  has_failure: boolean;
  first_failed_span_id: string | null;
  deepest_failed_span_id: string | null;
  deepest_failing_dependency_id: string | null;
  failure_path: string[];
  blocked_spans: ForensicsBlockedSpan[];
  retry_groups: ForensicsRetryGroup[];
}

export interface ForensicsBlockedSpan {
  span_id: string;
  name: string;
  reason: string | null;
}

export interface ForensicsRetryGroup {
  span_ids: string[];
  final_status: SpanStatus;
  final_status_label: string;
}

// ── Branch Draft ────────────────────────────────────────────────────

export interface BranchDraftState {
  source_run_id: string;
  fork_span_id: string;
  fork_span_name: string;
  patch_type: PatchType;
  patch_value: string;
  note: string;
}

// ── App State ───────────────────────────────────────────────────────

export type BottomTab = 'artifacts' | 'diff' | 'branch';
export type CenterView = 'tree' | 'timeline';

export interface AppState {
  runs: RunListItem[];
  selectedRunId: string | null;
  runTree: SpanTreeNode | null;
  runRecord: RunRecord | null;
  selectedSpanId: string | null;
  spanDetail: SpanRecord | null;
  spanArtifacts: ArtifactRecord[];
  spanEdges: SpanEdgeRecord[];
  diffSummary: DiffSummary | null;
  branches: BranchRecord[];
  bottomTab: BottomTab;
  branchDraft: BranchDraftState | null;
  centerView: CenterView;
  timeline: TimelineView | null;
  forensics: ForensicsReport | null;
  error: string | null;
  loading: { runs: boolean; tree: boolean; detail: boolean; timeline: boolean };
}
