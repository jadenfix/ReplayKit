import type { DiffSummary as DiffSummaryType } from '../types';
import { StatusBadge, formatDuration } from './StatusBadge';

interface DiffSummaryProps {
  diff: DiffSummaryType | null;
  onJumpToSpan: (spanId: string) => void;
}

export function DiffSummaryPanel({ diff, onJumpToSpan }: DiffSummaryProps) {
  if (!diff) {
    return (
      <div className="diff-panel">
        <div className="diff-panel__empty">
          Select a branch and click "View diff" to compare runs
        </div>
      </div>
    );
  }

  return (
    <div className="diff-panel" data-testid="diff-panel">
      <div className="diff-panel__header">
        <h3>Run Diff</h3>
        <span className="diff-panel__runs">
          {diff.source_run_id} vs {diff.target_run_id}
        </span>
      </div>

      {/* Summary cards */}
      <div className="diff-panel__summary">
        <div className="diff-card diff-card--status">
          <div className="diff-card__label">Status</div>
          <div className="diff-card__value">
            <StatusBadge status={diff.status_change.from} />
            <span className="diff-card__arrow">{'\u2192'}</span>
            <StatusBadge status={diff.status_change.to} />
          </div>
        </div>

        <div className="diff-card">
          <div className="diff-card__label">Changed Spans</div>
          <div className="diff-card__value diff-card__value--number">{diff.changed_span_count}</div>
        </div>

        <div className="diff-card">
          <div className="diff-card__label">Changed Artifacts</div>
          <div className="diff-card__value diff-card__value--number">{diff.changed_artifact_count}</div>
        </div>

        <div className={`diff-card ${diff.latency_ms_delta < 0 ? 'diff-card--positive' : 'diff-card--negative'}`}>
          <div className="diff-card__label">Latency Delta</div>
          <div className="diff-card__value diff-card__value--number">
            {diff.latency_ms_delta > 0 ? '+' : diff.latency_ms_delta < 0 ? '-' : ''}{formatDuration(Math.abs(diff.latency_ms_delta))}
          </div>
        </div>

        <div className="diff-card">
          <div className="diff-card__label">Token Delta</div>
          <div className="diff-card__value diff-card__value--number">
            {diff.token_delta > 0 ? '+' : ''}{diff.token_delta}
          </div>
        </div>

        <div className={`diff-card ${diff.final_output_changed ? 'diff-card--highlight' : ''}`}>
          <div className="diff-card__label">Output Changed</div>
          <div className="diff-card__value">{diff.final_output_changed ? 'Yes' : 'No'}</div>
        </div>
      </div>

      {/* First divergence - prominent CTA */}
      {diff.first_divergent_span_id && (
        <button
          className="diff-panel__divergence-cta"
          onClick={() => onJumpToSpan(diff.first_divergent_span_id)}
        >
          <span>{'\u2192'}</span>
          Jump to first divergent span: <strong>{diff.first_divergent_span_id}</strong>
        </button>
      )}

      {/* Span-by-span diffs */}
      <div className="diff-panel__spans">
        <h4>Span Diffs ({diff.span_diffs.length})</h4>
        {diff.span_diffs.length === 0 && diff.changed_span_count > 0 && (
          <div className="diff-panel__summary-only">
            Fingerprint comparison only — detailed span diffs not available
          </div>
        )}
        <table className="diff-table">
          <thead>
            <tr>
              <th>Span</th>
              <th>Status</th>
              <th>Duration</th>
              <th>Output</th>
              <th>Reason</th>
            </tr>
          </thead>
          <tbody>
            {diff.span_diffs.map(sd => (
              <tr key={sd.span_id_source} className="diff-table__row">
                <td className="diff-table__name">{sd.name}</td>
                <td className="diff-table__status">
                  {sd.status_change ? (
                    <>
                      <StatusBadge status={sd.status_change.from} />
                      <span className="diff-table__arrow">{'\u2192'}</span>
                      <StatusBadge status={sd.status_change.to} />
                    </>
                  ) : (
                    <span className="diff-table__unchanged">unchanged</span>
                  )}
                </td>
                <td className="diff-table__duration">
                  {sd.duration_ms_delta !== null
                    ? `${sd.duration_ms_delta > 0 ? '+' : sd.duration_ms_delta < 0 ? '-' : ''}${formatDuration(Math.abs(sd.duration_ms_delta))}`
                    : '\u2014'}
                </td>
                <td className="diff-table__output">
                  {sd.output_changed
                    ? <span className="diff-table__changed">changed</span>
                    : <span className="diff-table__unchanged">same</span>}
                </td>
                <td className="diff-table__reason">
                  {sd.dirty_reason && (
                    <span className={`dirty-tag dirty-tag--${sd.dirty_reason.toLowerCase()}`}>
                      {sd.dirty_reason}
                    </span>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
