// ── Provider abstraction: mock now, live API later ──────────────────

import type {
  RunListItem, RunRecord, SpanRecord, SpanTreeNode,
  ArtifactRecord, SpanEdgeRecord, BranchRecord,
  DiffSummary, BranchDraftState, ReplayPolicy, PatchType,
  TimelineView, ForensicsReport, SpanDiff,
} from '../types';

type ApiRunSummary = {
  run_id: string;
  title: string;
  adapter_name: string;
  status: RunRecord['status'];
  started_at: number;
  ended_at: number | null;
  error_count: number;
  source_run_id: string | null;
  failure_class?: string | null;
  final_output_preview?: string | null;
};

type ApiTreeNode = {
  span_id: string;
  name: string;
  kind: SpanRecord['kind'];
  status: SpanRecord['status'];
  replay_policy: string;
  started_at: number;
  ended_at: number | null;
  error_summary: string | null;
  children: ApiTreeNode[];
};

type ApiRunTree = {
  run_id: string;
  title: string;
  status: RunRecord['status'];
  nodes: ApiTreeNode[];
};

type ApiSpanDetail = {
  span_id: string;
  run_id: string;
  parent_span_id: string | null;
  sequence_no: number;
  name: string;
  kind: SpanRecord['kind'];
  status: SpanRecord['status'];
  replay_policy: string;
  executor_kind: string | null;
  executor_version: string | null;
  input_artifact_ids: string[];
  output_artifact_ids: string[];
  input_fingerprint: string | null;
  output_fingerprint: string | null;
  environment_fingerprint: string | null;
  error_code: string | null;
  error_summary: string | null;
  started_at: number;
  ended_at: number | null;
  attributes: Record<string, unknown>;
};

type ApiArtifactPreview = {
  artifact_id: string;
  artifact_type_label: string;
  mime: string;
  byte_len: number;
  summary: Record<string, unknown>;
};

type ApiArtifactContent = {
  artifact_id: string;
  content: string;
};

type ApiDependency = {
  edge_id: string;
  from_span_id: string;
  to_span_id: string;
  kind: SpanEdgeRecord['kind'];
};

type ApiBranchSummary = {
  branch_id: string;
  source_run_id: string;
  target_run_id: string;
  fork_span_id: string;
  patch_type: string;
  patch_summary: string;
  created_at: number;
  status: BranchRecord['status'];
};

type ApiBranchExecution = {
  branch_id: string;
  source_run_id: string;
  target_run_id: string;
  target_status: BranchRecord['status'];
};

type ApiDiffSummary = {
  diff_id: string;
  source_run_id: string;
  target_run_id: string;
  source_status: RunRecord['status'];
  target_status: RunRecord['status'];
  changed_span_count: number;
  changed_artifact_count: number;
  first_divergent_span_id: string | null;
  span_diffs?: ApiSpanDiff[];
  latency_ms_delta?: number | null;
  token_delta?: number | null;
  final_output_changed?: boolean;
  summary: Record<string, unknown>;
};

type ApiSpanDiff = {
  span_id_source: string;
  span_id_target: string;
  name: string;
  status_change?: string | null;
  duration_ms_delta?: number | null;
  output_changed: boolean;
  dirty_reason?: string | null;
};

type ApiTimelineView = {
  run_id: string;
  title: string;
  status: RunRecord['status'];
  total_started_at: number;
  total_ended_at: number | null;
  entries: Array<{
    span_id: string;
    name: string;
    kind: SpanRecord['kind'];
    status: SpanRecord['status'];
    status_label: string;
    started_at: number;
    ended_at: number | null;
    depth: number;
    parent_span_id: string | null;
    error_summary: string | null;
  }>;
};

type ApiForensicsReport = {
  run_id: string;
  has_failure: boolean;
  first_failed_span_id: string | null;
  deepest_failed_span_id: string | null;
  deepest_failing_dependency_id: string | null;
  failure_path: string[];
  blocked_spans: Array<{ span_id: string; name: string; reason: string | null }>;
  retry_groups: Array<{ span_ids: string[]; final_status: SpanRecord['status']; final_status_label: string }>;
};

// ── Provider interface ──────────────────────────────────────────────

export interface ReplayKitProvider {
  listRuns(): Promise<RunListItem[]>;
  getRunRecord(runId: string): Promise<RunRecord | null>;
  getRunTree(runId: string): Promise<SpanTreeNode | null>;
  getSpanDetail(runId: string, spanId: string): Promise<SpanRecord | null>;
  getSpanArtifacts(runId: string, spanId: string): Promise<ArtifactRecord[]>;
  getSpanEdges(runId: string): Promise<SpanEdgeRecord[]>;
  getBranches(runId: string): Promise<BranchRecord[]>;
  getDiffSummary(sourceRunId: string, targetRunId: string): Promise<DiffSummary | null>;
  createBranch(draft: BranchDraftState): Promise<BranchRecord>;
  getTimeline(runId: string): Promise<TimelineView | null>;
  getForensics(runId: string): Promise<ForensicsReport | null>;
}

// ── Mock provider ───────────────────────────────────────────────────

import {
  RUN_LIST, getRunRecord, getSpansForRun, buildTree,
  getArtifactsForSpan, getEdgesForRun, BRANCHES,
  getDiffForRuns, getTimelineForRun, getForensicsForRun,
} from '../data/mock-data';

function delay(ms = 80): Promise<void> {
  return new Promise(r => setTimeout(r, ms));
}

export class MockProvider implements ReplayKitProvider {
  async listRuns(): Promise<RunListItem[]> {
    await delay();
    return RUN_LIST;
  }

  async getRunRecord(runId: string): Promise<RunRecord | null> {
    await delay(40);
    return getRunRecord(runId) ?? null;
  }

  async getRunTree(runId: string): Promise<SpanTreeNode | null> {
    await delay(60);
    const spans = getSpansForRun(runId);
    return buildTree(spans);
  }

  async getSpanDetail(runId: string, spanId: string): Promise<SpanRecord | null> {
    await delay(30);
    const spans = getSpansForRun(runId);
    return spans.find(s => s.span_id === spanId) ?? null;
  }

  async getSpanArtifacts(runId: string, spanId: string): Promise<ArtifactRecord[]> {
    await delay(50);
    return getArtifactsForSpan(runId, spanId);
  }

  async getSpanEdges(runId: string): Promise<SpanEdgeRecord[]> {
    await delay(30);
    return getEdgesForRun(runId);
  }

  async getBranches(runId: string): Promise<BranchRecord[]> {
    await delay(30);
    return BRANCHES.filter(b => b.source_run_id === runId || b.target_run_id === runId);
  }

  async getDiffSummary(sourceRunId: string, targetRunId: string): Promise<DiffSummary | null> {
    await delay(60);
    return getDiffForRuns(sourceRunId, targetRunId);
  }

  async createBranch(draft: BranchDraftState): Promise<BranchRecord> {
    await delay(200);
    return {
      branch_id: `branch_new_${Date.now()}`,
      source_run_id: draft.source_run_id,
      target_run_id: `run_new_${Date.now()}`,
      fork_span_id: draft.fork_span_id,
      patch_type: draft.patch_type,
      patch_summary: draft.note || `${draft.patch_type} on ${draft.fork_span_name}`,
      created_at: Date.now(),
      status: 'Running',
    };
  }

  async getTimeline(runId: string): Promise<TimelineView | null> {
    await delay(60);
    return getTimelineForRun(runId);
  }

  async getForensics(runId: string): Promise<ForensicsReport | null> {
    await delay(50);
    return getForensicsForRun(runId);
  }
}

// ── Live provider (stub) ────────────────────────────────────────────

export class LiveProvider implements ReplayKitProvider {
  private baseUrl: string;
  constructor(baseUrl = 'http://localhost:3210') {
    this.baseUrl = baseUrl;
  }

  private async fetch<T>(path: string): Promise<T> {
    const res = await fetch(`${this.baseUrl}${path}`);
    if (!res.ok) throw new Error(`API error: ${res.status} ${res.statusText}`);
    return res.json() as Promise<T>;
  }

  async listRuns() {
    const runs = await this.fetch<ApiRunSummary[]>('/api/v1/runs');
    return runs.map(run => ({
      run_id: run.run_id,
      title: run.title,
      status: run.status,
      started_at: run.started_at,
      duration_ms: run.ended_at !== null ? Math.max(run.ended_at - run.started_at, 0) : null,
      adapter_name: run.adapter_name,
      failure_summary: run.failure_class ?? run.final_output_preview ?? null,
      source_run_id: run.source_run_id,
      span_count: 0,
      error_count: run.error_count,
    }));
  }

  async getRunRecord(runId: string) {
    const run = await this.fetch<ApiRunSummary>(`/api/v1/runs/${runId}`);
    return {
      run_id: run.run_id,
      trace_id: '',
      source_run_id: run.source_run_id,
      title: run.title,
      entrypoint: '',
      adapter_name: run.adapter_name,
      adapter_version: '',
      status: run.status,
      started_at: run.started_at,
      ended_at: run.ended_at,
      git_sha: null,
      environment_fingerprint: null,
      labels: [],
    };
  }

  async getRunTree(runId: string) {
    const tree = await this.fetch<ApiRunTree>(`/api/v1/runs/${runId}/tree`);
    if (tree.nodes.length === 0) return null;
    if (tree.nodes.length === 1) return mapTreeNode(runId, tree.nodes[0], 0, null);

    return {
      span: {
        run_id: runId,
        span_id: `${runId}::root`,
        trace_id: '',
        parent_span_id: null,
        sequence_no: 0,
        kind: 'Run' as const,
        name: tree.title,
        status: mapRunStatusToSpanStatus(tree.status),
        started_at: 0,
        ended_at: null,
        replay_policy: 'RecordOnly' as const,
        executor_kind: null,
        executor_version: null,
        input_artifact_ids: [],
        output_artifact_ids: [],
        snapshot_id: null,
        input_fingerprint: null,
        environment_fingerprint: null,
        error_code: null,
        error_summary: null,
        failure_class: null,
        dirty_reasons: [],
        blocked_replay_reason: null,
        attributes: {},
      },
      depth: 0,
      children: tree.nodes.map(node => mapTreeNode(runId, node, 1, `${runId}::root`)),
    };
  }

  async getSpanDetail(runId: string, spanId: string) {
    const span = await this.fetch<ApiSpanDetail>(`/api/v1/runs/${runId}/spans/${spanId}`);
    return mapSpanDetail(span);
  }

  async getSpanArtifacts(runId: string, spanId: string) {
    const previews = await this.fetch<ApiArtifactPreview[]>(
      `/api/v1/runs/${runId}/spans/${spanId}/artifacts`,
    );
    const contents = await Promise.all(previews.map(async preview => {
      try {
        return await this.fetch<ApiArtifactContent>(
          `/api/v1/runs/${runId}/artifacts/${preview.artifact_id}/content`,
        );
      } catch {
        return null;
      }
    }));

    return previews.map((preview, index) => ({
      artifact_id: preview.artifact_id,
      run_id: runId,
      span_id: spanId,
      type: preview.artifact_type_label,
      mime: preview.mime,
      byte_len: preview.byte_len,
      summary: summarizeDocument(preview.summary),
      content: contents[index]?.content ?? summarizeDocument(preview.summary) ?? '',
    }));
  }

  async getSpanEdges(runId: string) {
    const edges = await this.fetch<ApiDependency[]>(`/api/v1/runs/${runId}/edges`);
    return edges.map(edge => ({
      run_id: runId,
      from_span_id: edge.from_span_id,
      to_span_id: edge.to_span_id,
      kind: edge.kind,
    }));
  }

  async getBranches(runId: string) {
    const branches = await this.fetch<ApiBranchSummary[]>(`/api/v1/runs/${runId}/branches`);
    return branches.map(branch => ({
      branch_id: branch.branch_id,
      source_run_id: branch.source_run_id,
      target_run_id: branch.target_run_id,
      fork_span_id: branch.fork_span_id,
      patch_type: mapPatchType(branch.patch_type),
      patch_summary: branch.patch_summary,
      created_at: branch.created_at,
      status: branch.status,
    }));
  }

  async getDiffSummary(sourceRunId: string, targetRunId: string) {
    const diff = await this.fetch<ApiDiffSummary>(
      `/api/v1/runs/${sourceRunId}/diff/${targetRunId}`,
    );
    const spanDiffs: SpanDiff[] = (diff.span_diffs ?? []).map(sd => ({
      span_id_source: sd.span_id_source,
      span_id_target: sd.span_id_target,
      name: sd.name,
      status_change: sd.status_change ? parseStatusChange(sd.status_change) : null,
      duration_ms_delta: sd.duration_ms_delta ?? null,
      output_changed: sd.output_changed,
      dirty_reason: (sd.dirty_reason as SpanDiff['dirty_reason']) ?? null,
    }));
    return {
      diff_id: diff.diff_id,
      source_run_id: diff.source_run_id,
      target_run_id: diff.target_run_id,
      first_divergent_span_id: diff.first_divergent_span_id ?? '',
      status_change: { from: diff.source_status, to: diff.target_status },
      latency_ms_delta: diff.latency_ms_delta ?? readInt(diff.summary, 'latency_ms_delta'),
      token_delta: diff.token_delta ?? readInt(diff.summary, 'token_delta'),
      changed_span_count: diff.changed_span_count,
      changed_artifact_count: diff.changed_artifact_count,
      final_output_changed: diff.final_output_changed ?? readBool(diff.summary, 'final_output_changed'),
      span_diffs: spanDiffs,
    };
  }

  async createBranch(draft: BranchDraftState) {
    const res = await fetch(`${this.baseUrl}/api/v1/branches`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        source_run_id: draft.source_run_id,
        fork_span_id: draft.fork_span_id,
        patch_type: toApiPatchType(draft.patch_type),
        replacement: draft.patch_value,
        note: draft.note || null,
      }),
    });
    if (!res.ok) throw new Error(`API error: ${res.status}`);
    const execution = await res.json() as ApiBranchExecution;
    return {
      branch_id: execution.branch_id,
      source_run_id: execution.source_run_id,
      target_run_id: execution.target_run_id,
      fork_span_id: draft.fork_span_id,
      patch_type: draft.patch_type,
      patch_summary: draft.note || `${draft.patch_type} on ${draft.fork_span_name}`,
      created_at: Date.now(),
      status: execution.target_status,
    };
  }

  async getTimeline(runId: string): Promise<TimelineView | null> {
    try {
      const data = await this.fetch<ApiTimelineView>(`/api/v1/runs/${runId}/timeline`);
      return {
        run_id: data.run_id,
        title: data.title,
        status: data.status,
        total_started_at: data.total_started_at,
        total_ended_at: data.total_ended_at,
        entries: data.entries,
      };
    } catch {
      return null;
    }
  }

  async getForensics(runId: string): Promise<ForensicsReport | null> {
    try {
      const data = await this.fetch<ApiForensicsReport>(`/api/v1/runs/${runId}/forensics`);
      return {
        run_id: data.run_id,
        has_failure: data.has_failure,
        first_failed_span_id: data.first_failed_span_id,
        deepest_failed_span_id: data.deepest_failed_span_id,
        deepest_failing_dependency_id: data.deepest_failing_dependency_id,
        failure_path: data.failure_path,
        blocked_spans: data.blocked_spans,
        retry_groups: data.retry_groups,
      };
    } catch {
      return null;
    }
  }
}

function parseStatusChange(s: string): { from: SpanRecord['status']; to: SpanRecord['status'] } | null {
  const parts = s.split(' -> ');
  if (parts.length === 2) {
    return { from: parts[0] as SpanRecord['status'], to: parts[1] as SpanRecord['status'] };
  }
  return null;
}

function mapTreeNode(
  runId: string,
  node: ApiTreeNode,
  depth: number,
  parentSpanId: string | null,
): SpanTreeNode {
  return {
    span: {
      run_id: runId,
      span_id: node.span_id,
      trace_id: '',
      parent_span_id: parentSpanId,
      sequence_no: depth,
      kind: node.kind,
      name: node.name,
      status: node.status,
      started_at: node.started_at,
      ended_at: node.ended_at,
      replay_policy: mapReplayPolicy(node.replay_policy),
      executor_kind: null,
      executor_version: null,
      input_artifact_ids: [],
      output_artifact_ids: [],
      snapshot_id: null,
      input_fingerprint: null,
      environment_fingerprint: null,
      error_code: null,
      error_summary: node.error_summary,
      failure_class: null,
      dirty_reasons: [],
      blocked_replay_reason: null,
      attributes: {},
    },
    depth,
    children: node.children.map(child => mapTreeNode(runId, child, depth + 1, node.span_id)),
  };
}

function mapSpanDetail(span: ApiSpanDetail): SpanRecord {
  return {
    run_id: span.run_id,
    span_id: span.span_id,
    trace_id: '',
    parent_span_id: span.parent_span_id,
    sequence_no: span.sequence_no,
    kind: span.kind,
    name: span.name,
    status: span.status,
    started_at: span.started_at,
    ended_at: span.ended_at,
    replay_policy: mapReplayPolicy(span.replay_policy),
    executor_kind: span.executor_kind,
    executor_version: span.executor_version,
    input_artifact_ids: span.input_artifact_ids,
    output_artifact_ids: span.output_artifact_ids,
    snapshot_id: null,
    input_fingerprint: span.input_fingerprint,
    environment_fingerprint: span.environment_fingerprint,
    error_code: span.error_code,
    error_summary: span.error_summary,
    failure_class: null,
    dirty_reasons: [],
    blocked_replay_reason: null,
    attributes: span.attributes,
  };
}

function mapReplayPolicy(policy: string): ReplayPolicy {
  switch (policy) {
    case 'record_only':
    case 'RecordOnly':
      return 'RecordOnly';
    case 'rerunnable_supported':
    case 'RerunnableSupported':
      return 'RerunnableSupported';
    case 'cacheable_if_fingerprint_matches':
    case 'CacheableIfFingerprintMatches':
      return 'CacheableIfFingerprintMatches';
    case 'pure_reusable':
    case 'PureReusable':
      return 'PureReusable';
    default:
      return 'RecordOnly';
  }
}

function mapRunStatusToSpanStatus(status: RunRecord['status']): SpanRecord['status'] {
  switch (status) {
    case 'Running':
      return 'Running';
    case 'Completed':
      return 'Completed';
    case 'Failed':
      return 'Failed';
    case 'Blocked':
      return 'Blocked';
    case 'Canceled':
      return 'Canceled';
    case 'Interrupted':
    case 'Imported':
      return 'Blocked';
  }
}

function mapPatchType(patchType: string): PatchType {
  switch (patchType) {
    case 'PromptEdit':
    case 'ToolOutputOverride':
    case 'EnvVarOverride':
    case 'ModelConfigEdit':
    case 'RetrievalContextOverride':
    case 'SnapshotOverride':
      return patchType;
    default:
      return 'ToolOutputOverride';
  }
}

function toApiPatchType(patchType: PatchType): string {
  switch (patchType) {
    case 'PromptEdit':
      return 'prompt_edit';
    case 'ToolOutputOverride':
      return 'tool_output_override';
    case 'EnvVarOverride':
      return 'env_var_override';
    case 'ModelConfigEdit':
      return 'model_config_edit';
    case 'RetrievalContextOverride':
      return 'retrieval_context_override';
    case 'SnapshotOverride':
      return 'snapshot_override';
  }
}

function summarizeDocument(summary: Record<string, unknown> | null | undefined): string | null {
  if (!summary || Object.keys(summary).length === 0) return null;
  const preferredKeys = ['note', 'replacement', 'tool', 'answer', 'prompt', 'state'];
  for (const key of preferredKeys) {
    const value = summary[key];
    if (typeof value === 'string' && value.trim().length > 0) return value;
  }
  return JSON.stringify(summary, null, 2);
}

function readInt(summary: Record<string, unknown>, key: string): number {
  const value = summary[key];
  return typeof value === 'number' ? value : 0;
}

function readBool(summary: Record<string, unknown>, key: string): boolean {
  const value = summary[key];
  return typeof value === 'boolean' ? value : false;
}

// ── Factory ─────────────────────────────────────────────────────────

export function createProvider(): ReplayKitProvider {
  const apiUrl = (typeof window !== 'undefined')
    ? new URLSearchParams(window.location.search).get('api')
    : null;

  if (apiUrl) return new LiveProvider(apiUrl);
  return new MockProvider();
}
