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
