import type { BranchDraftState, PatchType, BranchRecord } from '../types';

interface BranchDraftProps {
  draft: BranchDraftState | null;
  branches: BranchRecord[];
  onUpdate: (partial: Partial<BranchDraftState>) => void;
  onCancel: () => void;
  onSubmit: () => void;
  onViewDiff: (sourceRunId: string, targetRunId: string) => void;
}

const PATCH_TYPES: { value: PatchType; label: string; desc: string }[] = [
  { value: 'PromptEdit', label: 'Prompt Edit', desc: 'Modify the prompt/input text sent to the model' },
  { value: 'ToolOutputOverride', label: 'Tool Output Override', desc: 'Replace the output of a tool/command' },
  { value: 'EnvVarOverride', label: 'Env Var Override', desc: 'Change environment variables for this span' },
  { value: 'ModelConfigEdit', label: 'Model Config Edit', desc: 'Change model, temperature, or other config' },
  { value: 'RetrievalContextOverride', label: 'Retrieval Override', desc: 'Replace retrieved documents/context' },
  { value: 'SnapshotOverride', label: 'Snapshot Override', desc: 'Replace the snapshot state' },
];

export function BranchDraft({ draft, branches, onUpdate, onCancel, onSubmit, onViewDiff }: BranchDraftProps) {
  if (!draft && branches.length === 0) {
    return (
      <div className="branch-panel">
        <div className="branch-panel__empty">
          Select a rerunnable span and click "branch" to create an exploratory branch
        </div>
      </div>
    );
  }

  return (
    <div className="branch-panel" data-testid="branch-panel">
      {/* Existing branches */}
      {branches.length > 0 && (
        <div className="branch-panel__existing">
          <h3>Branches ({branches.length})</h3>
          {branches.map(b => (
            <div key={b.branch_id} className="branch-card">
              <div className="branch-card__header">
                <span className="branch-card__type">{b.patch_type}</span>
                <span className={`badge badge--${b.status.toLowerCase()}`}>{b.status}</span>
              </div>
              <div className="branch-card__summary">{b.patch_summary}</div>
              <div className="branch-card__meta">
                <span>Fork: {b.fork_span_id}</span>
                <span>Target: {b.target_run_id}</span>
              </div>
              <button
                className="branch-card__diff-btn"
                onClick={() => onViewDiff(b.source_run_id, b.target_run_id)}
              >
                View diff
              </button>
            </div>
          ))}
        </div>
      )}

      {/* Draft form */}
      {draft && (
        <div className="branch-draft">
          <h3>New Branch</h3>
          <div className="branch-draft__target">
            Branching from: <strong>{draft.fork_span_name}</strong>
          </div>

          <label className="branch-draft__label">
            Patch Type
            <select
              className="branch-draft__select"
              value={draft.patch_type}
              onChange={e => onUpdate({ patch_type: e.target.value as PatchType })}
            >
              {PATCH_TYPES.map(pt => (
                <option key={pt.value} value={pt.value}>{pt.label}</option>
              ))}
            </select>
          </label>
          <div className="branch-draft__hint">
            {PATCH_TYPES.find(p => p.value === draft.patch_type)?.desc}
          </div>

          <label className="branch-draft__label">
            Patch Value
            <textarea
              className="branch-draft__textarea"
              value={draft.patch_value}
              onChange={e => onUpdate({ patch_value: e.target.value })}
              placeholder="Enter the replacement value..."
              rows={6}
            />
          </label>

          <label className="branch-draft__label">
            Note (optional)
            <input
              className="branch-draft__input"
              type="text"
              value={draft.note}
              onChange={e => onUpdate({ note: e.target.value })}
              placeholder="Why are you branching?"
            />
          </label>

          <div className="branch-draft__impact">
            <h4>Expected Impact</h4>
            <p>
              Patching <strong>{draft.fork_span_name}</strong> with <code>{draft.patch_type}</code> will
              mark this span dirty and cascade to all downstream data-dependent spans.
              Spans with compatible replay policies will be selectively re-executed.
              Spans marked <code>RecordOnly</code> downstream will block replay.
            </p>
          </div>

          <div className="branch-draft__actions">
            <button className="btn btn--secondary" onClick={onCancel}>Cancel</button>
            <button
              className="btn btn--primary"
              onClick={onSubmit}
              disabled={!draft.patch_value.trim()}
            >
              Create Branch
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
