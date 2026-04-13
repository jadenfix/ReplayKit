import { describe, it, expect } from 'vitest';
import { MockProvider } from '../providers';

describe('MockProvider', () => {
  const provider = new MockProvider();

  it('returns run list with expected items', async () => {
    const runs = await provider.listRuns();
    expect(runs.length).toBe(4);
    expect(runs.map(r => r.run_id)).toContain('run_01');
    expect(runs.map(r => r.run_id)).toContain('run_02');
  });

  it('returns correct run record', async () => {
    const run = await provider.getRunRecord('run_01');
    expect(run).not.toBeNull();
    expect(run!.title).toBe('Fix login timeout bug');
    expect(run!.status).toBe('Failed');
  });

  it('returns null for unknown run', async () => {
    const run = await provider.getRunRecord('nonexistent');
    expect(run).toBeNull();
  });

  it('builds tree from spans', async () => {
    const tree = await provider.getRunTree('run_01');
    expect(tree).not.toBeNull();
    expect(tree!.span.kind).toBe('Run');
    expect(tree!.children.length).toBeGreaterThan(0);
  });

  it('returns span detail', async () => {
    const span = await provider.getSpanDetail('run_01', 's01_shell1');
    expect(span).not.toBeNull();
    expect(span!.name).toBe('cargo test auth');
    expect(span!.status).toBe('Failed');
    expect(span!.failure_class).toBe('ShellFailure');
  });

  it('returns artifacts for span', async () => {
    const arts = await provider.getSpanArtifacts('run_01', 's01_llm1');
    expect(arts.length).toBeGreaterThan(0);
    expect(arts.some(a => a.type === 'prompt')).toBe(true);
  });

  it('returns edges', async () => {
    const edges = await provider.getSpanEdges('run_01');
    expect(edges.length).toBeGreaterThan(0);
    expect(edges.some(e => e.kind === 'DataDependsOn')).toBe(true);
  });

  it('returns branches for run with branch', async () => {
    const branches = await provider.getBranches('run_01');
    expect(branches.length).toBe(1);
    expect(branches[0].target_run_id).toBe('run_02');
  });

  it('returns diff summary', async () => {
    const diff = await provider.getDiffSummary('run_01', 'run_02');
    expect(diff).not.toBeNull();
    expect(diff!.status_change.from).toBe('Failed');
    expect(diff!.status_change.to).toBe('Completed');
    expect(diff!.changed_span_count).toBe(5);
  });

  it('creates a branch from draft', async () => {
    const branch = await provider.createBranch({
      source_run_id: 'run_01',
      fork_span_id: 's01_write1',
      fork_span_name: 'write_file login.rs',
      patch_type: 'ToolOutputOverride',
      patch_value: '{ "ok": true }',
      note: 'test branch',
    });
    expect(branch.source_run_id).toBe('run_01');
    expect(branch.fork_span_id).toBe('s01_write1');
    expect(branch.status).toBe('Running');
  });

  it('run_02 tree shows dirty spans from branch', async () => {
    const tree = await provider.getRunTree('run_02');
    expect(tree).not.toBeNull();

    function findDirty(node: NonNullable<typeof tree>): string[] {
      const result: string[] = [];
      if (node.span.dirty_reasons.length > 0) result.push(node.span.span_id);
      for (const c of node.children) result.push(...findDirty(c));
      return result;
    }

    const dirty = findDirty(tree!);
    expect(dirty.length).toBeGreaterThan(0);
  });

  it('returns timeline for run_01', async () => {
    const timeline = await provider.getTimeline('run_01');
    expect(timeline).not.toBeNull();
    expect(timeline!.run_id).toBe('run_01');
    expect(timeline!.entries.length).toBeGreaterThan(0);
    // Entries should be sorted by started_at
    for (let i = 1; i < timeline!.entries.length; i++) {
      expect(timeline!.entries[i].started_at).toBeGreaterThanOrEqual(timeline!.entries[i - 1].started_at);
    }
  });

  it('returns null timeline for unknown run', async () => {
    const timeline = await provider.getTimeline('nonexistent');
    expect(timeline).toBeNull();
  });

  it('returns forensics for failed run', async () => {
    const forensics = await provider.getForensics('run_01');
    expect(forensics).not.toBeNull();
    expect(forensics!.has_failure).toBe(true);
    expect(forensics!.first_failed_span_id).toBeTruthy();
    expect(forensics!.deepest_failed_span_id).toBeTruthy();
    expect(forensics!.failure_path.length).toBeGreaterThan(0);
  });

  it('returns forensics without failures for successful run', async () => {
    const forensics = await provider.getForensics('run_03');
    expect(forensics).not.toBeNull();
    expect(forensics!.has_failure).toBe(false);
    expect(forensics!.first_failed_span_id).toBeNull();
    expect(forensics!.failure_path).toHaveLength(0);
  });

  it('returns null forensics for unknown run', async () => {
    const forensics = await provider.getForensics('nonexistent');
    expect(forensics).toBeNull();
  });
});
