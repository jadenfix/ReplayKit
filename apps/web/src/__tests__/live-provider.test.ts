import { afterEach, describe, expect, it, vi } from 'vitest';

import { LiveProvider } from '../providers';

describe('LiveProvider', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('maps versioned run summaries into UI run list items', async () => {
    const fetchMock = vi.fn(async () => ({
      ok: true,
      json: async () => ([
        {
          run_id: 'run-1',
          title: 'demo',
          adapter_name: 'replaykit-sdk-rust-tracing',
          status: 'Failed',
          started_at: 100,
          ended_at: 160,
          error_count: 2,
          source_run_id: null,
          failure_class: 'ShellFailure',
          final_output_preview: null,
        },
      ]),
    }));
    vi.stubGlobal('fetch', fetchMock);

    const provider = new LiveProvider('http://localhost:3210');
    const runs = await provider.listRuns();

    expect(fetchMock).toHaveBeenCalledWith('http://localhost:3210/api/v1/runs');
    expect(runs).toEqual([
      {
        run_id: 'run-1',
        title: 'demo',
        status: 'Failed',
        started_at: 100,
        duration_ms: 60,
        adapter_name: 'replaykit-sdk-rust-tracing',
        failure_summary: 'ShellFailure',
        source_run_id: null,
        span_count: 0,
        error_count: 2,
      },
    ]);
  });

  it('posts branch drafts to the versioned API and maps execution responses', async () => {
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => ({
      ok: true,
      json: async () => ({
        branch_id: 'branch-1',
        source_run_id: 'run-1',
        target_run_id: 'run-2',
        target_status: 'Completed',
      }),
      init,
    }));
    vi.stubGlobal('fetch', fetchMock);

    const provider = new LiveProvider('http://localhost:3210');
    const branch = await provider.createBranch({
      source_run_id: 'run-1',
      fork_span_id: 'span-1',
      fork_span_name: 'tool',
      patch_type: 'ToolOutputOverride',
      patch_value: '{"ok":true}',
      note: 'override tool output',
    });

    expect(fetchMock).toHaveBeenCalledWith(
      'http://localhost:3210/api/v1/branches',
      expect.objectContaining({
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
      }),
    );
    const [, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(JSON.parse(String(init.body))).toEqual({
      source_run_id: 'run-1',
      fork_span_id: 'span-1',
      patch_type: 'tool_output_override',
      replacement: '{"ok":true}',
      note: 'override tool output',
    });
    expect(branch).toEqual({
      branch_id: 'branch-1',
      source_run_id: 'run-1',
      target_run_id: 'run-2',
      fork_span_id: 'span-1',
      patch_type: 'ToolOutputOverride',
      patch_summary: 'override tool output',
      created_at: expect.any(Number),
      status: 'Completed',
    });
  });
});
