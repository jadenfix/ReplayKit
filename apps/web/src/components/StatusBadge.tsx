import type { RunStatus, SpanStatus, SpanKind, ReplayPolicy } from '../types';

export function StatusBadge({ status }: { status: RunStatus | SpanStatus }) {
  return <span className={`badge badge--${status.toLowerCase()}`}>{status}</span>;
}

const KIND_ICONS: Record<SpanKind, string> = {
  Run: '\u25B6',
  PlannerStep: '\u2630',
  LlmCall: '\u2728',
  ToolCall: '\u2692',
  ShellCommand: '\u25BA',
  FileRead: '\u25A1',
  FileWrite: '\u270E',
  BrowserAction: '\u2316',
  Retrieval: '\u2315',
  MemoryLookup: '\u29BF',
  HumanInput: '\u263A',
  GuardrailCheck: '\u26A0',
  Subgraph: '\u2B21',
  AdapterInternal: '\u2699',
};

export function KindIcon({ kind }: { kind: SpanKind }) {
  return (
    <span className={`kind-icon kind-icon--${kind.toLowerCase()}`} title={kind}>
      {KIND_ICONS[kind] || '\u2022'}
    </span>
  );
}

const POLICY_LABELS: Record<ReplayPolicy, string> = {
  RecordOnly: 'Record Only',
  RerunnableSupported: 'Rerunnable',
  CacheableIfFingerprintMatches: 'Cacheable',
  PureReusable: 'Pure/Reusable',
};

export function PolicyBadge({ policy }: { policy: ReplayPolicy }) {
  return (
    <span className={`policy-badge policy-badge--${policy.toLowerCase()}`} title={`Replay policy: ${POLICY_LABELS[policy]}`}>
      {POLICY_LABELS[policy]}
    </span>
  );
}

export function formatDuration(ms: number | null): string {
  if (ms === null) return '\u2014';
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.floor(ms / 60000)}m ${Math.round((ms % 60000) / 1000)}s`;
}

export function formatTime(ts: number): string {
  return new Date(ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}
