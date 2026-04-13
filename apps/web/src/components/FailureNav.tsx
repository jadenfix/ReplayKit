import type { SpanTreeNode, SpanRecord, SpanEdgeRecord } from '../types';
import { findDeepestFailure, findDeepestFailingDependency, collectSpansFromTree } from '../utils';

interface FailureNavProps {
  tree: SpanTreeNode | null;
  edges: SpanEdgeRecord[];
  onJumpToSpan: (spanId: string) => void;
}

export function FailureNav({ tree, edges, onJumpToSpan }: FailureNavProps) {
  if (!tree) return null;

  const hasFailed = tree.span.status === 'Failed';
  const hasBlocked = hasBlockedSpan(tree);

  if (!hasFailed && !hasBlocked) return null;

  const firstFailure = findDeepestFailure(tree);
  const spans = collectSpansFromTree(tree);

  const deepestDep = firstFailure
    ? findDeepestFailingDependency(firstFailure.span.span_id, spans, edges)
    : null;

  const blockedSpans = collectBlocked(tree);

  return (
    <div className="failure-nav" data-testid="failure-nav">
      <div className="failure-nav__title">Failure Navigation</div>

      {firstFailure && (
        <button
          className="failure-nav__btn failure-nav__btn--failure"
          onClick={() => onJumpToSpan(firstFailure.span.span_id)}
        >
          <span className="failure-nav__icon">{'\u26A0'}</span>
          <span className="failure-nav__text">
            Jump to first failure: <strong>{firstFailure.span.name}</strong>
          </span>
        </button>
      )}

      {deepestDep && (
        <button
          className="failure-nav__btn failure-nav__btn--dependency"
          onClick={() => onJumpToSpan(deepestDep.span_id)}
        >
          <span className="failure-nav__icon">{'\u2B07'}</span>
          <span className="failure-nav__text">
            Deepest failing dependency: <strong>{deepestDep.name}</strong>
          </span>
        </button>
      )}

      {blockedSpans.length > 0 && (
        <div className="failure-nav__blocked">
          <div className="failure-nav__blocked-label">Blocked spans ({blockedSpans.length})</div>
          {blockedSpans.map(s => (
            <button
              key={s.span_id}
              className="failure-nav__btn failure-nav__btn--blocked"
              onClick={() => onJumpToSpan(s.span_id)}
            >
              <span className="failure-nav__icon">{'\u23F8'}</span>
              <span className="failure-nav__text">
                {s.name}
                {s.blocked_replay_reason && (
                  <span className="failure-nav__reason"> - {s.blocked_replay_reason}</span>
                )}
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function hasBlockedSpan(node: SpanTreeNode): boolean {
  if (node.span.status === 'Blocked') return true;
  return node.children.some(c => hasBlockedSpan(c));
}

function collectBlocked(node: SpanTreeNode): SpanRecord[] {
  const result: SpanRecord[] = [];
  if (node.span.status === 'Blocked') result.push(node.span);
  for (const child of node.children) {
    result.push(...collectBlocked(child));
  }
  return result;
}
