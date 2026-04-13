import { useMemo } from 'react';
import { createProvider } from './providers';
import { useAppState } from './hooks/useAppState';
import { Layout } from './components/Layout';
import { RunList } from './components/RunList';
import { RunTree } from './components/RunTree';
import { TimelineView } from './components/TimelineView';
import { SpanInspector } from './components/SpanInspector';
import { ArtifactViewer } from './components/ArtifactViewer';
import { BranchDraft } from './components/BranchDraft';
import { DiffSummaryPanel } from './components/DiffSummary';
import { FailureNav } from './components/FailureNav';
import type { BottomTab } from './types';

export default function App() {
  const provider = useMemo(() => createProvider(), []);
  const {
    state,
    selectRun,
    selectSpan,
    setBottomTab,
    startBranchDraft,
    updateBranchDraft,
    cancelBranchDraft,
    submitBranch,
    loadDiff,
    jumpToSpan,
    setCenterView,
  } = useAppState(provider);

  const tabs: { id: BottomTab; label: string }[] = [
    { id: 'artifacts', label: 'Artifacts' },
    { id: 'diff', label: 'Diff' },
    { id: 'branch', label: 'Branch' },
  ];

  const bottomContent = (
    <div className="bottom-tabs">
      <div className="bottom-tabs__bar">
        {tabs.map(t => (
          <button
            key={t.id}
            className={`bottom-tabs__tab ${state.bottomTab === t.id ? 'bottom-tabs__tab--active' : ''}`}
            onClick={() => setBottomTab(t.id)}
          >
            {t.label}
            {t.id === 'branch' && state.branchDraft && (
              <span className="bottom-tabs__indicator" />
            )}
          </button>
        ))}
      </div>
      <div className="bottom-tabs__content">
        {state.bottomTab === 'artifacts' && (
          <ArtifactViewer
            artifacts={state.spanArtifacts}
            selectedSpanId={state.selectedSpanId}
          />
        )}
        {state.bottomTab === 'diff' && (
          <DiffSummaryPanel
            diff={state.diffSummary}
            onJumpToSpan={jumpToSpan}
          />
        )}
        {state.bottomTab === 'branch' && (
          <BranchDraft
            draft={state.branchDraft}
            branches={state.branches}
            onUpdate={updateBranchDraft}
            onCancel={cancelBranchDraft}
            onSubmit={submitBranch}
            onViewDiff={loadDiff}
          />
        )}
      </div>
    </div>
  );

  const timelineAvailable = state.timeline !== null || state.loading.timeline;

  return (
    <Layout
      left={
        <RunList
          runs={state.runs}
          selectedRunId={state.selectedRunId}
          loading={state.loading.runs}
          onSelectRun={selectRun}
        />
      }
      center={
        <>
          {state.selectedRunId && (
            <div className="center-view-toggle">
              <button
                className={`center-view-toggle__btn ${state.centerView === 'tree' ? 'center-view-toggle__btn--active' : ''}`}
                onClick={() => setCenterView('tree')}
              >
                Tree
              </button>
              <button
                className={`center-view-toggle__btn ${state.centerView === 'timeline' ? 'center-view-toggle__btn--active' : ''}`}
                onClick={() => setCenterView('timeline')}
                disabled={!timelineAvailable}
              >
                Timeline
              </button>
            </div>
          )}
          {state.centerView === 'tree' ? (
            <RunTree
              tree={state.runTree}
              selectedSpanId={state.selectedSpanId}
              loading={state.loading.tree}
              onSelectSpan={selectSpan}
              onBranch={startBranchDraft}
            />
          ) : (
            <TimelineView
              timeline={state.timeline}
              selectedSpanId={state.selectedSpanId}
              loading={state.loading.timeline}
              onSelectSpan={selectSpan}
            />
          )}
        </>
      }
      right={
        <SpanInspector
          span={state.spanDetail}
          artifacts={state.spanArtifacts}
          edges={state.spanEdges}
          loading={state.loading.detail}
          onBranch={startBranchDraft}
        />
      }
      bottom={bottomContent}
      failureNav={
        <FailureNav
          tree={state.runTree}
          edges={state.spanEdges}
          forensics={state.forensics}
          onJumpToSpan={jumpToSpan}
        />
      }
    />
  );
}
