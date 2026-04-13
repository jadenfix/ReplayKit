import type { SpanTreeNode, SpanRecord, SpanEdgeRecord, ForensicsReport } from '../types';
import { findDeepestFailure, findDeepestFailingDependency, collectSpansFromTree } from '../utils';

interface FailureNavProps {
  tree: SpanTreeNode | null;
  edges: SpanEdgeRecord[];
  forensics: ForensicsReport | null;
  onJumpToSpan: (spanId: string) => void;
}

export function FailureNav({ tree, edges, forensics, onJumpToSpan }: FailureNavProps) {
  if (!tree) return null;

  // Use backend forensics when available, otherwise fall back to client-side
  if (forensics && forensics.has_failure) {
    return (
      <div className="failure-nav" data-testid="failure-nav">
        <div className="failure-nav__title">Failure Navigation</div>

        {forensics.deepest_failed_span_id && (
          <button
            className="failure-nav__btn failure-nav__btn--failure"
            onClick={() => onJumpToSpan(forensics.deepest_failed_span_id!)}
          >
            <span className="failure-nav__icon">{'\u26A0'}</span>
            <span className="failure-nav__text">
              Jump to deepest failure
            </span>
          </button>
        )}

        {forensics.deepest_failing_dependency_id && (
          <button
            className="failure-nav__btn failure-nav__btn--dependency"
            onClick={() => onJumpToSpan(forensics.deepest_failing_dependency_id!)}
          >
            <span className="failure-nav__icon">{'\u2B07'}</span>
            <span className="failure-nav__text">
              Deepest failing dependency
            </span>
          </button>
        )}

        {forensics.failure_path.length > 0 && (
          <div className="failure-nav__chain">
            <div className="failure-nav__chain-title">Failure path</div>
            {forensics.failure_path.map((spanId, i) => (
              <button
                key={spanId}
                className={
                  'failure-nav__chain-entry' +
                  (i === forensics.failure_path.length - 1 ? ' failure-nav__chain-entry--root-cause' : '')
                }
                onClick={() => onJumpToSpan(spanId)}
              >
                <span className="failure-nav__chain-depth">{'\u2514'}</span>
                <span className="failure-nav__text">{spanId}</span>
              </button>
            ))}
          </div>
        )}

        {forensics.blocked_spans.length > 0 && (
          <div className="failure-nav__blocked">
            <div className="failure-nav__blocked-label">Blocked spans ({forensics.blocked_spans.length})</div>
            {forensics.blocked_spans.map(bs => (
              <button
                key={bs.span_id}
                className="failure-nav__btn failure-nav__btn--blocked"
                onClick={() => onJumpToSpan(bs.span_id)}
              >
                <span className="failure-nav__icon">{'\u23F8'}</span>
                <span className="failure-nav__text">
                  {bs.name}
                  {bs.reason && (
                    <span className="failure-nav__reason"> - {bs.reason}</span>
                  )}
                </span>
              </button>
            ))}
          </div>
        )}

        {forensics.retry_groups.length > 0 && (
          <div className="failure-nav__retries">
            <div className="failure-nav__blocked-label">Retry groups ({forensics.retry_groups.length})</div>
            {forensics.retry_groups.map((group, i) => (
              <div key={i} className="failure-nav__retry-group">
                <span className="failure-nav__icon">{'\u21BB'}</span>
                <span className="failure-nav__text">
                  {group.span_ids.length} attempts, final: {group.final_status_label}
                </span>
              </div>
            ))}
          </div>
        )}
      </div>
    );
  }

  // Client-side fallback
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
