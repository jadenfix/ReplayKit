// ── Provider abstraction: mock now, live API later ──────────────────

import type {
  RunListItem, RunRecord, SpanRecord, SpanTreeNode,
  ArtifactRecord, SpanEdgeRecord, BranchRecord,
  DiffSummary, BranchDraftState,
} from '../types';

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
}

// ── Mock provider ───────────────────────────────────────────────────

import {
  RUN_LIST, getRunRecord, getSpansForRun, buildTree,
  getArtifactsForSpan, getEdgesForRun, BRANCHES,
  getDiffForRuns,
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
}

// ── Live provider (stub) ────────────────────────────────────────────

export class LiveProvider implements ReplayKitProvider {
  private baseUrl: string;
  constructor(baseUrl = 'http://localhost:9201') {
    this.baseUrl = baseUrl;
  }

  private async fetch<T>(path: string): Promise<T> {
    const res = await fetch(`${this.baseUrl}${path}`);
    if (!res.ok) throw new Error(`API error: ${res.status} ${res.statusText}`);
    return res.json() as Promise<T>;
  }

  async listRuns() {
    return this.fetch<RunListItem[]>('/api/runs');
  }

  async getRunRecord(runId: string) {
    return this.fetch<RunRecord | null>(`/api/runs/${runId}`);
  }

  async getRunTree(runId: string) {
    return this.fetch<SpanTreeNode | null>(`/api/runs/${runId}/tree`);
  }

  async getSpanDetail(runId: string, spanId: string) {
    return this.fetch<SpanRecord | null>(`/api/runs/${runId}/spans/${spanId}`);
  }

  async getSpanArtifacts(runId: string, spanId: string) {
    return this.fetch<ArtifactRecord[]>(`/api/runs/${runId}/spans/${spanId}/artifacts`);
  }

  async getSpanEdges(runId: string) {
    return this.fetch<SpanEdgeRecord[]>(`/api/runs/${runId}/edges`);
  }

  async getBranches(runId: string) {
    return this.fetch<BranchRecord[]>(`/api/runs/${runId}/branches`);
  }

  async getDiffSummary(sourceRunId: string, targetRunId: string) {
    return this.fetch<DiffSummary | null>(`/api/diffs/${sourceRunId}/${targetRunId}`);
  }

  async createBranch(draft: BranchDraftState) {
    const res = await fetch(`${this.baseUrl}/api/branches`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(draft),
    });
    if (!res.ok) throw new Error(`API error: ${res.status}`);
    return res.json() as Promise<BranchRecord>;
  }
}

// ── Factory ─────────────────────────────────────────────────────────

export function createProvider(): ReplayKitProvider {
  const apiUrl = (typeof window !== 'undefined')
    ? new URLSearchParams(window.location.search).get('api')
    : null;

  if (apiUrl) return new LiveProvider(apiUrl);
  return new MockProvider();
}
