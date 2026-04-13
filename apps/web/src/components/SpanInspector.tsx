import type { SpanRecord, ArtifactRecord, SpanEdgeRecord } from '../types';
import { StatusBadge, KindIcon, PolicyBadge, formatDuration, formatTime } from './StatusBadge';

interface SpanInspectorProps {
  span: SpanRecord | null;
  artifacts: ArtifactRecord[];
  edges: SpanEdgeRecord[];
  loading: boolean;
  onBranch: (span: SpanRecord) => void;
}

export function SpanInspector({ span, artifacts, edges, loading, onBranch }: SpanInspectorProps) {
  if (loading) {
    return (
      <div className="inspector">
        <div className="inspector__header"><h2>Inspector</h2></div>
        <div className="inspector__loading">Loading...</div>
      </div>
    );
  }

  if (!span) {
    return (
      <div className="inspector">
        <div className="inspector__header"><h2>Inspector</h2></div>
        <div className="inspector__empty">Select a span to inspect</div>
      </div>
    );
  }

  const duration = span.ended_at && span.started_at
    ? span.ended_at - span.started_at
    : null;

  const spanEdges = edges.filter(
    e => e.from_span_id === span.span_id || e.to_span_id === span.span_id
  );
  const canBranch = span.replay_policy !== 'RecordOnly';

  const inputArtifacts = artifacts.filter(a => span.input_artifact_ids.includes(a.artifact_id));
  const outputArtifacts = artifacts.filter(a => span.output_artifact_ids.includes(a.artifact_id));

  return (
    <div className="inspector" data-testid="span-inspector">
      <div className="inspector__header">
        <h2>Inspector</h2>
      </div>

      <div className="inspector__content">
        {/* Identity */}
        <section className="inspector__section">
          <div className="inspector__identity">
            <KindIcon kind={span.kind} />
            <span className="inspector__name">{span.name}</span>
          </div>
          <div className="inspector__badges">
            <StatusBadge status={span.status} />
            <PolicyBadge policy={span.replay_policy} />
            {canBranch && (
              <button className="inspector__branch-btn" onClick={() => onBranch(span)}>
                Branch from here
              </button>
            )}
          </div>
        </section>

        {/* Error */}
        {span.error_summary && (
          <section className="inspector__section inspector__section--error">
            <h3>Error</h3>
            {span.error_code && <div className="inspector__error-code">{span.error_code}</div>}
            {span.failure_class && (
              <div className="inspector__failure-class">{span.failure_class}</div>
            )}
            <pre className="inspector__error-text">{span.error_summary}</pre>
          </section>
        )}

        {/* Blocked */}
        {span.blocked_replay_reason && (
          <section className="inspector__section inspector__section--blocked">
            <h3>Blocked Replay</h3>
            <p className="inspector__blocked-reason">{span.blocked_replay_reason}</p>
          </section>
        )}

        {/* Dirty Reasons */}
        {span.dirty_reasons.length > 0 && (
          <section className="inspector__section inspector__section--dirty">
            <h3>Dirty Reasons</h3>
            <ul className="inspector__dirty-list">
              {span.dirty_reasons.map(r => (
                <li key={r} className="inspector__dirty-reason">{formatDirtyReason(r)}</li>
              ))}
            </ul>
          </section>
        )}

        {/* Timing */}
        <section className="inspector__section">
          <h3>Timing</h3>
          <dl className="inspector__dl">
            <dt>Started</dt>
            <dd>{formatTime(span.started_at)}</dd>
            {span.ended_at && (
              <>
                <dt>Ended</dt>
                <dd>{formatTime(span.ended_at)}</dd>
              </>
            )}
            <dt>Duration</dt>
            <dd>{formatDuration(duration)}</dd>
          </dl>
        </section>

        {/* Executor */}
        {span.executor_kind && (
          <section className="inspector__section">
            <h3>Executor</h3>
            <dl className="inspector__dl">
              <dt>Kind</dt>
              <dd><code>{span.executor_kind}</code></dd>
              {span.executor_version && (
                <>
                  <dt>Version</dt>
                  <dd><code>{span.executor_version}</code></dd>
                </>
              )}
            </dl>
          </section>
        )}

        {/* Fingerprints */}
        {(span.input_fingerprint || span.environment_fingerprint) && (
          <section className="inspector__section">
            <h3>Fingerprints</h3>
            <dl className="inspector__dl">
              {span.input_fingerprint && (
                <>
                  <dt>Input</dt>
                  <dd><code className="inspector__fp">{span.input_fingerprint}</code></dd>
                </>
              )}
              {span.environment_fingerprint && (
                <>
                  <dt>Environment</dt>
                  <dd><code className="inspector__fp">{span.environment_fingerprint}</code></dd>
                </>
              )}
            </dl>
          </section>
        )}

        {/* Input Artifacts */}
        {inputArtifacts.length > 0 && (
          <section className="inspector__section">
            <h3>Input Artifacts ({inputArtifacts.length})</h3>
            {inputArtifacts.map(a => (
              <ArtifactSummary key={a.artifact_id} artifact={a} />
            ))}
          </section>
        )}

        {/* Output Artifacts */}
        {outputArtifacts.length > 0 && (
          <section className="inspector__section">
            <h3>Output Artifacts ({outputArtifacts.length})</h3>
            {outputArtifacts.map(a => (
              <ArtifactSummary key={a.artifact_id} artifact={a} />
            ))}
          </section>
        )}

        {/* Dependencies */}
        {spanEdges.length > 0 && (
          <section className="inspector__section">
            <h3>Dependencies ({spanEdges.length})</h3>
            <ul className="inspector__edge-list">
              {spanEdges.map((e, i) => (
                <li key={i} className={`inspector__edge inspector__edge--${e.kind.toLowerCase()}`}>
                  <span className="inspector__edge-kind">{formatEdgeKind(e.kind)}</span>
                  <span className="inspector__edge-dir">
                    {e.from_span_id === span.span_id ? `\u2192 ${e.to_span_id}` : `\u2190 ${e.from_span_id}`}
                  </span>
                </li>
              ))}
            </ul>
          </section>
        )}

        {/* IDs */}
        <section className="inspector__section inspector__section--ids">
          <h3>Identifiers</h3>
          <dl className="inspector__dl">
            <dt>Span ID</dt>
            <dd><code>{span.span_id}</code></dd>
            <dt>Run ID</dt>
            <dd><code>{span.run_id}</code></dd>
            {span.parent_span_id && (
              <>
                <dt>Parent</dt>
                <dd><code>{span.parent_span_id}</code></dd>
              </>
            )}
            <dt>Sequence</dt>
            <dd>{span.sequence_no}</dd>
          </dl>
        </section>
      </div>
    </div>
  );
}

function ArtifactSummary({ artifact }: { artifact: ArtifactRecord }) {
  return (
    <div className="artifact-summary">
      <div className="artifact-summary__header">
        <span className="artifact-summary__type">{artifact.type}</span>
        <span className="artifact-summary__mime">{artifact.mime}</span>
        <span className="artifact-summary__size">{artifact.byte_len}B</span>
      </div>
      {artifact.summary && (
        <div className="artifact-summary__text">{artifact.summary}</div>
      )}
    </div>
  );
}

function formatEdgeKind(kind: string): string {
  return kind.replace(/([A-Z])/g, ' $1').trim();
}

function formatDirtyReason(reason: string): string {
  const labels: Record<string, string> = {
    PatchedInput: 'Input was patched (branch modification)',
    FingerprintChanged: 'Input fingerprint changed',
    UpstreamOutputChanged: 'Upstream output changed (cascade from patch)',
    ExecutorVersionChanged: 'Executor version changed',
    PolicyForcedRerun: 'Policy forced re-execution',
    MissingReusableArtifact: 'Reusable artifact missing',
    DependencyUnknown: 'Dependency info incomplete, conservatively dirty',
  };
  return labels[reason] || reason;
}
