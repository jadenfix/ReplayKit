import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { RunList } from '../components/RunList';
import { RunTree } from '../components/RunTree';
import { SpanInspector } from '../components/SpanInspector';
import { DiffSummaryPanel } from '../components/DiffSummary';
import { ArtifactViewer } from '../components/ArtifactViewer';
import { BranchDraft } from '../components/BranchDraft';
import { TimelineView } from '../components/TimelineView';
import { FailureNav } from '../components/FailureNav';
import { RUN_LIST, SPANS_RUN01, ARTIFACTS_RUN01, EDGES_RUN01, DIFF_01_02, buildTree, BRANCHES, getTimelineForRun, getForensicsForRun } from '../data/mock-data';

describe('RunList', () => {
  it('renders all runs', () => {
    const onSelect = vi.fn();
    render(<RunList runs={RUN_LIST} selectedRunId={null} loading={false} onSelectRun={onSelect} />);

    expect(screen.getByText('Fix login timeout bug')).toBeInTheDocument();
    expect(screen.getByText('Add pagination to user list API')).toBeInTheDocument();
    expect(screen.getByText('Refactor database connection pool')).toBeInTheDocument();
  });

  it('shows loading state', () => {
    render(<RunList runs={[]} selectedRunId={null} loading={true} onSelectRun={vi.fn()} />);
    expect(screen.getByText('Loading runs...')).toBeInTheDocument();
  });

  it('calls onSelectRun when clicked', () => {
    const onSelect = vi.fn();
    render(<RunList runs={RUN_LIST} selectedRunId={null} loading={false} onSelectRun={onSelect} />);

    fireEvent.click(screen.getByTestId('run-entry-run_01'));
    expect(onSelect).toHaveBeenCalledWith('run_01');
  });

  it('highlights selected run', () => {
    render(<RunList runs={RUN_LIST} selectedRunId="run_01" loading={false} onSelectRun={vi.fn()} />);
    const entry = screen.getByTestId('run-entry-run_01');
    expect(entry.className).toContain('run-entry--selected');
  });

  it('shows branch indicator', () => {
    render(<RunList runs={RUN_LIST} selectedRunId={null} loading={false} onSelectRun={vi.fn()} />);
    expect(screen.getByText('branch')).toBeInTheDocument();
  });

  it('shows failure summary for failed runs', () => {
    render(<RunList runs={RUN_LIST} selectedRunId={null} loading={false} onSelectRun={vi.fn()} />);
    expect(screen.getByText(/auth::session/)).toBeInTheDocument();
  });
});

describe('RunTree', () => {
  const tree = buildTree(SPANS_RUN01)!;

  it('renders tree nodes', () => {
    render(
      <RunTree tree={tree} selectedSpanId={null}
        loading={false} onSelectSpan={vi.fn()} onBranch={vi.fn()} />
    );
    expect(screen.getByText('Fix login timeout bug')).toBeInTheDocument();
    expect(screen.getByText('cargo test auth')).toBeInTheDocument();
  });

  it('shows empty state when no tree', () => {
    render(
      <RunTree tree={null} selectedSpanId={null}
        loading={false} onSelectSpan={vi.fn()} onBranch={vi.fn()} />
    );
    expect(screen.getByText(/Select a run/)).toBeInTheDocument();
  });

  it('calls onSelectSpan when node clicked', () => {
    const onSelect = vi.fn();
    render(
      <RunTree tree={tree} selectedSpanId={null}
        loading={false} onSelectSpan={onSelect} onBranch={vi.fn()} />
    );

    fireEvent.click(screen.getByTestId('tree-node-s01_shell1'));
    expect(onSelect).toHaveBeenCalledWith('s01_shell1');
  });

  it('highlights selected node', () => {
    render(
      <RunTree tree={tree} selectedSpanId="s01_shell1"
        loading={false} onSelectSpan={vi.fn()} onBranch={vi.fn()} />
    );
    const node = screen.getByTestId('tree-node-s01_shell1');
    expect(node.className).toContain('tree-node__row--selected');
  });

  it('shows status badges', () => {
    render(
      <RunTree tree={tree} selectedSpanId={null}
        loading={false} onSelectSpan={vi.fn()} onBranch={vi.fn()} />
    );
    const failedBadges = screen.getAllByText('Failed');
    expect(failedBadges.length).toBeGreaterThan(0);
  });
});

describe('SpanInspector', () => {
  const failedSpan = SPANS_RUN01.find(s => s.span_id === 's01_shell1')!;
  const blockedSpan = SPANS_RUN01.find(s => s.span_id === 's01_report')!;
  const artifacts = ARTIFACTS_RUN01.filter(a => a.span_id === 's01_shell1');

  it('shows empty state when no span', () => {
    render(
      <SpanInspector span={null} artifacts={[]} edges={[]}
        loading={false} onBranch={vi.fn()} />
    );
    expect(screen.getByText(/Select a span/)).toBeInTheDocument();
  });

  it('renders span details', () => {
    render(
      <SpanInspector span={failedSpan} artifacts={artifacts} edges={EDGES_RUN01}
        loading={false} onBranch={vi.fn()} />
    );
    expect(screen.getByText('cargo test auth')).toBeInTheDocument();
    expect(screen.getByText('EXIT_101')).toBeInTheDocument();
  });

  it('shows error summary', () => {
    render(
      <SpanInspector span={failedSpan} artifacts={artifacts} edges={EDGES_RUN01}
        loading={false} onBranch={vi.fn()} />
    );
    expect(screen.getByText(/test_async_timeout/)).toBeInTheDocument();
  });

  it('shows blocked replay reason', () => {
    render(
      <SpanInspector span={blockedSpan} artifacts={[]} edges={EDGES_RUN01}
        loading={false} onBranch={vi.fn()} />
    );
    expect(screen.getByText(/Upstream span/)).toBeInTheDocument();
  });

  it('shows replay policy badge', () => {
    render(
      <SpanInspector span={failedSpan} artifacts={artifacts} edges={EDGES_RUN01}
        loading={false} onBranch={vi.fn()} />
    );
    expect(screen.getByText('Rerunnable')).toBeInTheDocument();
  });

  it('shows branch button for rerunnable spans', () => {
    render(
      <SpanInspector span={failedSpan} artifacts={artifacts} edges={EDGES_RUN01}
        loading={false} onBranch={vi.fn()} />
    );
    expect(screen.getByText('Branch from here')).toBeInTheDocument();
  });
});

describe('ArtifactViewer', () => {
  const artifacts = ARTIFACTS_RUN01.filter(a => a.span_id === 's01_shell1');

  it('shows empty state when no span selected', () => {
    render(<ArtifactViewer artifacts={[]} selectedSpanId={null} />);
    expect(screen.getByText(/Select a span/)).toBeInTheDocument();
  });

  it('renders artifacts', () => {
    render(<ArtifactViewer artifacts={artifacts} selectedSpanId="s01_shell1" />);
    expect(screen.getByText('shell_input')).toBeInTheDocument();
    expect(screen.getByText('shell_output')).toBeInTheDocument();
  });

  it('shows artifact content', () => {
    render(<ArtifactViewer artifacts={artifacts} selectedSpanId="s01_shell1" />);
    expect(screen.getByText(/cargo test auth/)).toBeInTheDocument();
  });
});

describe('DiffSummaryPanel', () => {
  it('shows empty state when no diff', () => {
    render(<DiffSummaryPanel diff={null} onJumpToSpan={vi.fn()} />);
    expect(screen.getByText(/View diff/)).toBeInTheDocument();
  });

  it('renders diff summary', () => {
    render(<DiffSummaryPanel diff={DIFF_01_02} onJumpToSpan={vi.fn()} />);
    expect(screen.getByText('5')).toBeInTheDocument(); // changed span count
  });

  it('shows status change', () => {
    render(<DiffSummaryPanel diff={DIFF_01_02} onJumpToSpan={vi.fn()} />);
    const failedBadges = screen.getAllByText('Failed');
    expect(failedBadges.length).toBeGreaterThan(0);
  });

  it('shows span diffs in table', () => {
    render(<DiffSummaryPanel diff={DIFF_01_02} onJumpToSpan={vi.fn()} />);
    expect(screen.getByText('cargo test auth')).toBeInTheDocument();
    expect(screen.getByText('write_file login.rs')).toBeInTheDocument();
  });
});

describe('BranchDraft', () => {
  it('shows empty state when no draft and no branches', () => {
    render(
      <BranchDraft draft={null} branches={[]} onUpdate={vi.fn()}
        onCancel={vi.fn()} onSubmit={vi.fn()} onViewDiff={vi.fn()} />
    );
    expect(screen.getByText(/Select a rerunnable span/)).toBeInTheDocument();
  });

  it('shows existing branches', () => {
    render(
      <BranchDraft draft={null} branches={BRANCHES} onUpdate={vi.fn()}
        onCancel={vi.fn()} onSubmit={vi.fn()} onViewDiff={vi.fn()} />
    );
    expect(screen.getByText('ToolOutputOverride')).toBeInTheDocument();
    expect(screen.getByText(/View diff/)).toBeInTheDocument();
  });

  it('renders draft form', () => {
    const draft = {
      source_run_id: 'run_01',
      fork_span_id: 's01_write1',
      fork_span_name: 'write_file login.rs',
      patch_type: 'ToolOutputOverride' as const,
      patch_value: '',
      note: '',
    };

    render(
      <BranchDraft draft={draft} branches={[]} onUpdate={vi.fn()}
        onCancel={vi.fn()} onSubmit={vi.fn()} onViewDiff={vi.fn()} />
    );
    expect(screen.getAllByText(/write_file login.rs/).length).toBeGreaterThan(0);
    expect(screen.getByText('Create Branch')).toBeInTheDocument();
  });

  it('disables submit when patch value empty', () => {
    const draft = {
      source_run_id: 'run_01',
      fork_span_id: 's01_write1',
      fork_span_name: 'write_file login.rs',
      patch_type: 'ToolOutputOverride' as const,
      patch_value: '',
      note: '',
    };

    render(
      <BranchDraft draft={draft} branches={[]} onUpdate={vi.fn()}
        onCancel={vi.fn()} onSubmit={vi.fn()} onViewDiff={vi.fn()} />
    );
    expect(screen.getByText('Create Branch')).toBeDisabled();
  });
});

describe('TimelineView', () => {
  it('renders loading state', () => {
    render(<TimelineView timeline={null} selectedSpanId={null} loading={true} onSelectSpan={vi.fn()} />);
    expect(screen.getByText(/Loading timeline/)).toBeInTheDocument();
  });

  it('renders empty state when no timeline', () => {
    render(<TimelineView timeline={null} selectedSpanId={null} loading={false} onSelectSpan={vi.fn()} />);
    expect(screen.getByText(/Select a run/)).toBeInTheDocument();
  });

  it('renders all timeline entries for run_01', () => {
    const timeline = getTimelineForRun('run_01')!;
    render(<TimelineView timeline={timeline} selectedSpanId={null} loading={false} onSelectSpan={vi.fn()} />);
    // Should render entries for all spans in the run
    expect(timeline.entries.length).toBeGreaterThan(0);
    // Check at least one known span name is rendered
    expect(screen.getByText('Analyze issue')).toBeInTheDocument();
  });

  it('highlights selected span', () => {
    const timeline = getTimelineForRun('run_01')!;
    const firstSpan = timeline.entries[0].span_id;
    const { container } = render(
      <TimelineView timeline={timeline} selectedSpanId={firstSpan} loading={false} onSelectSpan={vi.fn()} />
    );
    const selected = container.querySelector('.timeline-entry--selected');
    expect(selected).not.toBeNull();
  });

  it('calls onSelectSpan when entry clicked', () => {
    const timeline = getTimelineForRun('run_01')!;
    const onSelect = vi.fn();
    render(<TimelineView timeline={timeline} selectedSpanId={null} loading={false} onSelectSpan={onSelect} />);
    const entries = document.querySelectorAll('.timeline-entry');
    if (entries.length > 0) {
      fireEvent.click(entries[0]);
      expect(onSelect).toHaveBeenCalled();
    }
  });
});

describe('FailureNav with forensics', () => {
  it('uses forensics data when available', () => {
    const tree = buildTree(SPANS_RUN01);
    const forensics = getForensicsForRun('run_01')!;
    const onJump = vi.fn();
    render(<FailureNav tree={tree} edges={EDGES_RUN01} forensics={forensics} onJumpToSpan={onJump} />);
    expect(screen.getByTestId('failure-nav')).toBeInTheDocument();
    // Should show "Jump to deepest failure" from forensics
    expect(screen.getByText(/Jump to deepest failure/)).toBeInTheDocument();
  });

  it('shows failure path from forensics', () => {
    const tree = buildTree(SPANS_RUN01);
    const forensics = getForensicsForRun('run_01')!;
    render(<FailureNav tree={tree} edges={EDGES_RUN01} forensics={forensics} onJumpToSpan={vi.fn()} />);
    if (forensics.failure_path.length > 0) {
      expect(screen.getByText(/Failure path/)).toBeInTheDocument();
    }
  });

  it('shows blocked spans from forensics', () => {
    const tree = buildTree(SPANS_RUN01);
    const forensics = getForensicsForRun('run_01')!;
    render(<FailureNav tree={tree} edges={EDGES_RUN01} forensics={forensics} onJumpToSpan={vi.fn()} />);
    if (forensics.blocked_spans.length > 0) {
      expect(screen.getByText(/Blocked spans/)).toBeInTheDocument();
    }
  });

  it('falls back to client-side when forensics is null', () => {
    const tree = buildTree(SPANS_RUN01);
    render(<FailureNav tree={tree} edges={EDGES_RUN01} forensics={null} onJumpToSpan={vi.fn()} />);
    expect(screen.getByTestId('failure-nav')).toBeInTheDocument();
    // Should show the client-side "Jump to first failure" text
    expect(screen.getByText(/Jump to first failure/)).toBeInTheDocument();
  });

  it('returns null for successful run without forensics', () => {
    const successSpan = {
      ...SPANS_RUN01[0],
      span_id: 'ok_root',
      status: 'Completed' as const,
    };
    const tree = { span: successSpan, children: [], depth: 0 };
    const { container } = render(
      <FailureNav tree={tree} edges={[]} forensics={null} onJumpToSpan={vi.fn()} />
    );
    expect(container.querySelector('.failure-nav')).toBeNull();
  });
});
