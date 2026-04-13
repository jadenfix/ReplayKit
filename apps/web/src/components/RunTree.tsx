import { useState, useCallback } from 'react';
import type { SpanTreeNode, SpanRecord } from '../types';
import { StatusBadge, KindIcon, formatDuration } from './StatusBadge';

interface RunTreeProps {
  tree: SpanTreeNode | null;
  selectedSpanId: string | null;
  loading: boolean;
  onSelectSpan: (spanId: string) => void;
  onBranch: (span: SpanRecord) => void;
}

export function RunTree({ tree, selectedSpanId, loading, onSelectSpan, onBranch }: RunTreeProps) {
  if (loading) {
    return (
      <div className="run-tree">
        <div className="run-tree__header"><h2>Span Tree</h2></div>
        <div className="run-tree__loading">Loading tree...</div>
      </div>
    );
  }

  if (!tree) {
    return (
      <div className="run-tree">
        <div className="run-tree__header"><h2>Span Tree</h2></div>
        <div className="run-tree__empty">Select a run to view its execution tree</div>
      </div>
    );
  }

  return (
    <div className="run-tree">
      <div className="run-tree__header">
        <h2>Span Tree</h2>
        <span className="run-tree__hint">Click a span to inspect</span>
      </div>
      <div className="run-tree__nodes" role="tree">
        <TreeNode
          node={tree}
          selectedSpanId={selectedSpanId}
          onSelectSpan={onSelectSpan}
          onBranch={onBranch}
        />
      </div>
    </div>
  );
}

function TreeNode({
  node,
  selectedSpanId,
  onSelectSpan,
  onBranch,
}: {
  node: SpanTreeNode;
  selectedSpanId: string | null;
  onSelectSpan: (id: string) => void;
  onBranch: (span: SpanRecord) => void;
}) {
  const [expanded, setExpanded] = useState(true);
  const hasChildren = node.children.length > 0;
  const isSelected = node.span.span_id === selectedSpanId;
  const isFailed = node.span.status === 'Failed';
  const isBlocked = node.span.status === 'Blocked';
  const isRunning = node.span.status === 'Running';
  const canBranch = node.span.replay_policy !== 'RecordOnly';

  const duration = node.span.ended_at && node.span.started_at
    ? node.span.ended_at - node.span.started_at
    : null;

  const toggle = useCallback((e: React.MouseEvent) => {
    e.stopPropagation();
    setExpanded(v => !v);
  }, []);

  const handleBranch = useCallback((e: React.MouseEvent) => {
    e.stopPropagation();
    onBranch(node.span);
  }, [node.span, onBranch]);

  return (
    <div className="tree-node" role="treeitem" aria-expanded={expanded}>
      <div
        className={[
          'tree-node__row',
          isSelected && 'tree-node__row--selected',
          isFailed && 'tree-node__row--failed',
          isBlocked && 'tree-node__row--blocked',
          isRunning && 'tree-node__row--running',
        ].filter(Boolean).join(' ')}
        style={{ paddingLeft: `${node.depth * 20 + 8}px` }}
        onClick={() => onSelectSpan(node.span.span_id)}
        data-testid={`tree-node-${node.span.span_id}`}
      >
        {hasChildren ? (
          <button className="tree-node__toggle" onClick={toggle} aria-label={expanded ? 'Collapse' : 'Expand'}>
            {expanded ? '\u25BE' : '\u25B8'}
          </button>
        ) : (
          <span className="tree-node__toggle tree-node__toggle--leaf">&nbsp;</span>
        )}
        <KindIcon kind={node.span.kind} />
        <span className="tree-node__name">{node.span.name}</span>
        <span className="tree-node__spacer" />
        {node.span.dirty_reasons.length > 0 && (
          <span className="tree-node__dirty" title={node.span.dirty_reasons.join(', ')}>dirty</span>
        )}
        <StatusBadge status={node.span.status} />
        <span className="tree-node__duration">{formatDuration(duration)}</span>
        {canBranch && (
          <button
            className="tree-node__branch-btn"
            onClick={handleBranch}
            title="Branch from this span"
          >
            branch
          </button>
        )}
      </div>
      {expanded && hasChildren && (
        <div className="tree-node__children" role="group">
          {node.children.map(child => (
            <TreeNode
              key={child.span.span_id}
              node={child}
              selectedSpanId={selectedSpanId}
              onSelectSpan={onSelectSpan}
              onBranch={onBranch}
            />
          ))}
        </div>
      )}
    </div>
  );
}
