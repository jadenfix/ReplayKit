import { describe, it, expect } from 'vitest';
import {
  buildTree, findFirstFailure, findDeepestFailingDependency,
  getSpansForRun, getEdgesForRun, SPANS_RUN01, EDGES_RUN01,
} from '../data/mock-data';

describe('buildTree', () => {
  it('builds tree with correct root', () => {
    const tree = buildTree(SPANS_RUN01);
    expect(tree).not.toBeNull();
    expect(tree!.span.span_id).toBe('s01_root');
    expect(tree!.depth).toBe(0);
  });

  it('nests children correctly', () => {
    const tree = buildTree(SPANS_RUN01)!;
    // Root should have 5 children: plan, search, fix, validate, report
    expect(tree.children.length).toBe(5);
    expect(tree.children[0].span.name).toBe('Analyze issue');
    expect(tree.children[0].depth).toBe(1);
  });

  it('handles deeply nested spans', () => {
    const tree = buildTree(SPANS_RUN01)!;
    const searchStep = tree.children.find(c => c.span.name === 'Search codebase');
    expect(searchStep).toBeDefined();
    expect(searchStep!.children.length).toBe(3); // 3 tool calls
  });

  it('returns null for empty spans', () => {
    expect(buildTree([])).toBeNull();
  });
});

describe('findFirstFailure', () => {
  it('finds the deepest failed span', () => {
    const tree = buildTree(SPANS_RUN01)!;
    const failure = findFirstFailure(tree);
    expect(failure).not.toBeNull();
    // Should find the shell command (deepest failure), not the planner step
    expect(failure!.span.span_id).toBe('s01_shell1');
  });

  it('returns null for successful runs', () => {
    const spans = getSpansForRun('run_03');
    const tree = buildTree(spans)!;
    expect(findFirstFailure(tree)).toBeNull();
  });
});

describe('findDeepestFailingDependency', () => {
  it('finds upstream failing dependency via data edges', () => {
    // s01_llm3 depends on s01_shell1 (which failed)
    const dep = findDeepestFailingDependency('s01_llm3', SPANS_RUN01, EDGES_RUN01);
    expect(dep).not.toBeNull();
    expect(dep!.span_id).toBe('s01_shell1');
  });

  it('returns null when no failing dependency', () => {
    const dep = findDeepestFailingDependency('s01_tool1', SPANS_RUN01, EDGES_RUN01);
    expect(dep).toBeNull();
  });
});

describe('data integrity', () => {
  it('all spans reference valid parent', () => {
    const ids = new Set(SPANS_RUN01.map(s => s.span_id));
    for (const s of SPANS_RUN01) {
      if (s.parent_span_id) {
        expect(ids.has(s.parent_span_id), `${s.span_id} has invalid parent ${s.parent_span_id}`).toBe(true);
      }
    }
  });

  it('edges reference valid spans', () => {
    const ids = new Set(SPANS_RUN01.map(s => s.span_id));
    for (const e of EDGES_RUN01) {
      expect(ids.has(e.from_span_id), `edge from unknown ${e.from_span_id}`).toBe(true);
      expect(ids.has(e.to_span_id), `edge to unknown ${e.to_span_id}`).toBe(true);
    }
  });

  it('run_02 has same span count as run_01', () => {
    const spans01 = getSpansForRun('run_01');
    const spans02 = getSpansForRun('run_02');
    expect(spans02.length).toBe(spans01.length);
  });

  it('run_02 edges reference run_02 span ids', () => {
    const edges = getEdgesForRun('run_02');
    const ids = new Set(getSpansForRun('run_02').map(s => s.span_id));
    for (const e of edges) {
      expect(ids.has(e.from_span_id), `run_02 edge from unknown ${e.from_span_id}`).toBe(true);
      expect(ids.has(e.to_span_id), `run_02 edge to unknown ${e.to_span_id}`).toBe(true);
    }
  });
});
