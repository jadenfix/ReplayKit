import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { useAppState } from '../hooks/useAppState';
import type { ReplayKitProvider } from '../providers';
import type {
  ArtifactRecord,
  BranchDraftState,
  BranchRecord,
  DiffSummary,
  ForensicsReport,
  RunListItem,
  RunRecord,
  SpanEdgeRecord,
  SpanRecord,
  SpanTreeNode,
  TimelineView,
} from '../types';

function makeProvider(overrides: Partial<ReplayKitProvider> = {}): ReplayKitProvider {
  return {
    listRuns: vi.fn<() => Promise<RunListItem[]>>().mockResolvedValue([]),
    getRunRecord: vi.fn<(runId: string) => Promise<RunRecord | null>>().mockResolvedValue(null),
    getRunTree: vi.fn<(runId: string) => Promise<SpanTreeNode | null>>().mockResolvedValue(null),
    getSpanDetail: vi.fn<(runId: string, spanId: string) => Promise<SpanRecord | null>>().mockResolvedValue(null),
    getSpanArtifacts: vi.fn<(runId: string, spanId: string) => Promise<ArtifactRecord[]>>().mockResolvedValue([]),
    getSpanEdges: vi.fn<(runId: string) => Promise<SpanEdgeRecord[]>>().mockResolvedValue([]),
    getBranches: vi.fn<(runId: string) => Promise<BranchRecord[]>>().mockResolvedValue([]),
    getDiffSummary: vi.fn<(sourceRunId: string, targetRunId: string) => Promise<DiffSummary | null>>().mockResolvedValue(null),
    createBranch: vi.fn<(draft: BranchDraftState) => Promise<BranchRecord>>().mockResolvedValue({
      branch_id: 'branch-1',
      source_run_id: 'run-1',
      target_run_id: 'run-2',
      fork_span_id: 'span-1',
      patch_type: 'ToolOutputOverride',
      patch_summary: 'ok',
      created_at: 1,
      status: 'Completed',
    }),
    getTimeline: vi.fn<(runId: string) => Promise<TimelineView | null>>().mockResolvedValue(null),
    getForensics: vi.fn<(runId: string) => Promise<ForensicsReport | null>>().mockResolvedValue(null),
    ...overrides,
  };
}

const rerunnableSpan: SpanRecord = {
  run_id: 'run-1',
  span_id: 'span-1',
  trace_id: 'trace-1',
  parent_span_id: null,
  sequence_no: 1,
  kind: 'ToolCall',
  name: 'tool span',
  status: 'Failed',
  started_at: 1,
  ended_at: 2,
  replay_policy: 'RerunnableSupported',
  executor_kind: null,
  executor_version: null,
  input_artifact_ids: [],
  output_artifact_ids: [],
  snapshot_id: null,
  input_fingerprint: null,
  environment_fingerprint: null,
  error_code: null,
  error_summary: 'boom',
  failure_class: null,
  dirty_reasons: [],
  blocked_replay_reason: null,
  attributes: {},
};

describe('useAppState', () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('retries loading runs after a transient failure', async () => {
    const listRuns = vi
      .fn<() => Promise<RunListItem[]>>()
      .mockRejectedValueOnce(new Error('temporary network error'))
      .mockResolvedValueOnce([
        {
          run_id: 'run-1',
          title: 'seeded',
          status: 'Completed',
          started_at: 100,
          duration_ms: 10,
          adapter_name: 'smoke',
          failure_summary: null,
          source_run_id: null,
          span_count: 1,
          error_count: 0,
        },
      ]);
    const provider = makeProvider({ listRuns });

    const { result } = renderHook(() => useAppState(provider));

    await waitFor(() => expect(listRuns).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(listRuns).toHaveBeenCalledTimes(2), { timeout: 2000 });
    await waitFor(() => expect(result.current.state.runs).toHaveLength(1), { timeout: 2000 });
    expect(listRuns).toHaveBeenCalledTimes(2);
    expect(result.current.state.error).toBeNull();
  });

  it('surfaces branch-creation failures without throwing away the draft', async () => {
    const provider = makeProvider({
      createBranch: vi.fn<(draft: BranchDraftState) => Promise<BranchRecord>>().mockRejectedValue(
        new Error('API error: 500'),
      ),
    });

    const { result } = renderHook(() => useAppState(provider));

    await waitFor(() => expect(result.current.state.loading.runs).toBe(false));

    act(() => {
      result.current.selectRun('run-1');
    });
    act(() => {
      result.current.startBranchDraft(rerunnableSpan);
    });
    act(() => {
      result.current.updateBranchDraft({ patch_value: 'patched output' });
    });

    await act(async () => {
      await result.current.submitBranch();
    });

    expect(result.current.state.branchDraft).not.toBeNull();
    expect(result.current.state.error).toContain('Failed to create branch.');
    expect(result.current.state.error).toContain('API error: 500');
  });
});
