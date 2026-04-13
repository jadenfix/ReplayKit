// ── Provider-agnostic tree and failure utilities ────────────────────
// These operate on pure domain types, NOT on mock data.

import type { SpanTreeNode, SpanRecord, SpanEdgeRecord } from './types';

/**
 * Finds the deepest failed span in the tree by walking children first.
 * If a failed node has a failed child, returns the child (deeper failure).
 */
export function findDeepestFailure(tree: SpanTreeNode): SpanTreeNode | null {
  if (tree.span.status === 'Failed') {
    for (const child of tree.children) {
      const deeper = findDeepestFailure(child);
      if (deeper) return deeper;
    }
    return tree;
  }
  for (const child of tree.children) {
    const found = findDeepestFailure(child);
    if (found) return found;
  }
  return null;
}

/**
 * Follows DataDependsOn edges backwards from `spanId` to find the deepest
 * upstream span that also failed.
 */
export function findDeepestFailingDependency(
  spanId: string,
  spans: SpanRecord[],
  edges: SpanEdgeRecord[],
): SpanRecord | null {
  const dataEdges = edges.filter(e => e.kind === 'DataDependsOn' && e.to_span_id === spanId);
  const spanMap = new Map(spans.map(s => [s.span_id, s]));

  for (const edge of dataEdges) {
    const dep = spanMap.get(edge.from_span_id);
    if (dep && dep.status === 'Failed') {
      const deeper = findDeepestFailingDependency(dep.span_id, spans, edges);
      return deeper ?? dep;
    }
  }
  return null;
}

/**
 * Flatten a tree into a list of all spans (for dependency lookups).
 */
export function collectSpansFromTree(node: SpanTreeNode): SpanRecord[] {
  const result: SpanRecord[] = [node.span];
  for (const child of node.children) {
    result.push(...collectSpansFromTree(child));
  }
  return result;
}
