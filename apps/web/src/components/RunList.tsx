import type { RunListItem } from '../types';
import { StatusBadge } from './StatusBadge';
import { formatDuration, formatTime } from '../utils/format';

interface RunListProps {
  runs: RunListItem[];
  selectedRunId: string | null;
  loading: boolean;
  onSelectRun: (runId: string) => void;
}

export function RunList({ runs, selectedRunId, loading, onSelectRun }: RunListProps) {
  if (loading) {
    return (
      <div className="run-list">
        <div className="run-list__header">
          <h2>Runs</h2>
        </div>
        <div className="run-list__loading">Loading runs...</div>
      </div>
    );
  }

  return (
    <div className="run-list">
      <div className="run-list__header">
        <h2>Runs</h2>
        <span className="run-list__count">{runs.length}</span>
      </div>
      <div className="run-list__items">
        {runs.map(run => (
          <RunListEntry
            key={run.run_id}
            run={run}
            selected={run.run_id === selectedRunId}
            onSelect={onSelectRun}
          />
        ))}
      </div>
    </div>
  );
}

function RunListEntry({
  run,
  selected,
  onSelect,
}: {
  run: RunListItem;
  selected: boolean;
  onSelect: (id: string) => void;
}) {
  return (
    <button
      className={`run-entry ${selected ? 'run-entry--selected' : ''} run-entry--${run.status.toLowerCase()}`}
      onClick={() => onSelect(run.run_id)}
      data-testid={`run-entry-${run.run_id}`}
    >
      <div className="run-entry__header">
        <span className="run-entry__title">{run.title}</span>
        <StatusBadge status={run.status} />
      </div>
      <div className="run-entry__meta">
        <span className="run-entry__time">{formatTime(run.started_at)}</span>
        <span className="run-entry__duration">{formatDuration(run.duration_ms)}</span>
        <span className="run-entry__adapter">{run.adapter_name.replace('replaykit-', '')}</span>
        {run.error_count > 0 && (
          <span className="run-entry__errors">{run.error_count} errors</span>
        )}
        {run.source_run_id && (
          <span className="run-entry__branch" title={`Branched from ${run.source_run_id}`}>branch</span>
        )}
      </div>
      {run.failure_summary && (
        <div className="run-entry__failure">{run.failure_summary}</div>
      )}
    </button>
  );
}
