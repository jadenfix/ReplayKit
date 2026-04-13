import type { TimelineView as TimelineViewType } from '../types';
import { KindIcon, StatusBadge } from './StatusBadge';
import { formatDuration } from '../utils/format';

interface TimelineViewProps {
  timeline: TimelineViewType | null;
  selectedSpanId: string | null;
  loading: boolean;
  onSelectSpan: (spanId: string) => void;
}

export function TimelineView({ timeline, selectedSpanId, loading, onSelectSpan }: TimelineViewProps) {
  if (loading) return <div className="timeline__loading">Loading timeline…</div>;
  if (!timeline) return <div className="timeline__empty">Select a run to view its timeline</div>;
  if (timeline.entries.length === 0) return <div className="timeline__empty">No spans recorded</div>;

  const minTime = timeline.total_started_at;
  const maxTime = timeline.total_ended_at ?? Math.max(...timeline.entries.map(e => e.ended_at ?? e.started_at));
  const totalDuration = Math.max(maxTime - minTime, 1);

  return (
    <div className="timeline">
      <div className="timeline__header">
        <span className="timeline__title">Timeline</span>
        <span className="timeline__duration">{formatDuration(totalDuration * 1000)}</span>
      </div>
      <div className="timeline__entries">
        {timeline.entries.map(entry => {
          const left = ((entry.started_at - minTime) / totalDuration) * 100;
          const end = entry.ended_at ?? maxTime;
          const width = Math.max(((end - entry.started_at) / totalDuration) * 100, 0.5);
          const selected = entry.span_id === selectedSpanId;
          const failed = entry.status === 'Failed';
          const blocked = entry.status === 'Blocked';

          return (
            <div
              key={entry.span_id}
              className={
                'timeline-entry' +
                (selected ? ' timeline-entry--selected' : '') +
                (failed ? ' timeline-entry--failed' : '') +
                (blocked ? ' timeline-entry--blocked' : '')
              }
              onClick={() => onSelectSpan(entry.span_id)}
            >
              <div
                className="timeline-entry__label"
                style={{ paddingLeft: `${entry.depth * 16 + 8}px` }}
              >
                <KindIcon kind={entry.kind} />
                <span className="timeline-entry__name" title={entry.name}>{entry.name}</span>
                <StatusBadge status={entry.status} />
              </div>
              <div className="timeline-entry__chart">
                <div
                  className={`timeline-entry__bar timeline-entry__bar--${entry.status.toLowerCase()}`}
                  style={{ left: `${left}%`, width: `${width}%` }}
                  title={entry.error_summary ?? `${entry.name} (${entry.status})`}
                />
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
