import { useReducer, useCallback, useEffect } from 'react';
import type {
  AppState, BottomTab, BranchDraftState, CenterView,
  SpanRecord, RunRecord, SpanTreeNode,
  ArtifactRecord, SpanEdgeRecord, BranchRecord,
  DiffSummary, RunListItem, PatchType,
  TimelineView, ForensicsReport,
} from '../types';
import type { ReplayKitProvider } from '../providers';

// ── Actions ─────────────────────────────────────────────────────────

type Action =
  | { type: 'RUNS_LOADING' }
  | { type: 'RUNS_LOADED'; runs: RunListItem[] }
  | { type: 'TREE_LOADED'; runId: string; tree: SpanTreeNode | null; run: RunRecord | null; branches: BranchRecord[]; edges: SpanEdgeRecord[] }
  | { type: 'SPAN_LOADED'; runId: string; spanId: string; span: SpanRecord | null; artifacts: ArtifactRecord[]; edges: SpanEdgeRecord[] }
  | { type: 'SELECT_RUN'; runId: string }
  | { type: 'SELECT_SPAN'; spanId: string }
  | { type: 'SET_BOTTOM_TAB'; tab: BottomTab }
  | { type: 'START_BRANCH_DRAFT'; span: SpanRecord }
  | { type: 'UPDATE_BRANCH_DRAFT'; draft: Partial<BranchDraftState> }
  | { type: 'CANCEL_BRANCH_DRAFT' }
  | { type: 'BRANCH_CREATED'; branch: BranchRecord }
  | { type: 'DIFF_LOADED'; diff: DiffSummary | null }
  | { type: 'SET_CENTER_VIEW'; view: CenterView }
  | { type: 'TIMELINE_LOADED'; runId: string; timeline: TimelineView | null }
  | { type: 'FORENSICS_LOADED'; runId: string; forensics: ForensicsReport | null }
  | { type: 'SET_ERROR'; error: string | null };

const RUNS_RETRY_DELAYS_MS = [500, 1000];

function errorMessage(err: unknown, fallback: string): string {
  if (err instanceof Error && err.message.trim()) {
    return err.message;
  }
  return fallback;
}

// ── Initial state ───────────────────────────────────────────────────

const INIT: AppState = {
  runs: [],
  selectedRunId: null,
  runTree: null,
  runRecord: null,
  selectedSpanId: null,
  spanDetail: null,
  spanArtifacts: [],
  spanEdges: [],
  diffSummary: null,
  branches: [],
  bottomTab: 'artifacts',
  branchDraft: null,
  centerView: 'tree',
  timeline: null,
  forensics: null,
  error: null,
  loading: { runs: true, tree: false, detail: false, timeline: false },
};

// ── Reducer ─────────────────────────────────────────────────────────

function reducer(state: AppState, action: Action): AppState {
  switch (action.type) {
    case 'RUNS_LOADING':
      return { ...state, loading: { ...state.loading, runs: true } };
    case 'RUNS_LOADED':
      return { ...state, runs: action.runs, loading: { ...state.loading, runs: false } };
    case 'SELECT_RUN':
      return {
        ...state,
        selectedRunId: action.runId,
        selectedSpanId: null,
        spanDetail: null,
        spanArtifacts: [],
        runTree: null,
        timeline: null,
        forensics: null,
        diffSummary: null,
        branchDraft: null,
        centerView: 'tree',
        error: null,
        loading: { ...state.loading, tree: true, timeline: true },
      };
    case 'TREE_LOADED':
      if (action.runId !== state.selectedRunId) return state;
      return {
        ...state,
        runTree: action.tree,
        runRecord: action.run,
        branches: action.branches,
        spanEdges: action.edges,
        loading: { ...state.loading, tree: false },
      };
    case 'SELECT_SPAN':
      return {
        ...state,
        selectedSpanId: action.spanId,
        loading: { ...state.loading, detail: true },
      };
    case 'SPAN_LOADED':
      if (action.runId !== state.selectedRunId || action.spanId !== state.selectedSpanId) return state;
      return {
        ...state,
        spanDetail: action.span,
        spanArtifacts: action.artifacts,
        spanEdges: action.edges,
        loading: { ...state.loading, detail: false },
      };
    case 'SET_BOTTOM_TAB':
      return { ...state, bottomTab: action.tab };
    case 'START_BRANCH_DRAFT':
      return {
        ...state,
        bottomTab: 'branch',
        branchDraft: {
          source_run_id: state.selectedRunId!,
          fork_span_id: action.span.span_id,
          fork_span_name: action.span.name,
          patch_type: defaultPatchType(action.span),
          patch_value: '',
          note: '',
        },
      };
    case 'UPDATE_BRANCH_DRAFT':
      return state.branchDraft
        ? { ...state, branchDraft: { ...state.branchDraft, ...action.draft } }
        : state;
    case 'CANCEL_BRANCH_DRAFT':
      return { ...state, branchDraft: null, bottomTab: 'artifacts' };
    case 'BRANCH_CREATED':
      return {
        ...state,
        branchDraft: null,
        branches: [...state.branches, action.branch],
        bottomTab: 'artifacts',
      };
    case 'DIFF_LOADED':
      return { ...state, diffSummary: action.diff, bottomTab: 'diff' };
    case 'SET_CENTER_VIEW':
      return { ...state, centerView: action.view };
    case 'TIMELINE_LOADED':
      if (action.runId !== state.selectedRunId) return state;
      return { ...state, timeline: action.timeline, loading: { ...state.loading, timeline: false } };
    case 'FORENSICS_LOADED':
      if (action.runId !== state.selectedRunId) return state;
      return { ...state, forensics: action.forensics };
    case 'SET_ERROR':
      return { ...state, error: action.error };
    default:
      return state;
  }
}

function defaultPatchType(span: SpanRecord): PatchType {
  if (span.kind === 'LlmCall') return 'PromptEdit';
  if (span.kind === 'ToolCall' || span.kind === 'ShellCommand') return 'ToolOutputOverride';
  if (span.kind === 'FileWrite' || span.kind === 'FileRead') return 'ToolOutputOverride';
  if (span.kind === 'Retrieval') return 'RetrievalContextOverride';
  return 'ToolOutputOverride';
}

// ── Hook ────────────────────────────────────────────────────────────

export function useAppState(provider: ReplayKitProvider) {
  const [state, dispatch] = useReducer(reducer, INIT);

  // Load runs on mount
  useEffect(() => {
    let cancelled = false;
    let retryTimer: number | null = null;

    const loadRuns = async (attempt: number) => {
      try {
        const runs = await provider.listRuns();
        if (cancelled) return;
        dispatch({ type: 'SET_ERROR', error: null });
        dispatch({ type: 'RUNS_LOADED', runs });
      } catch (err) {
        if (cancelled) return;
        if (attempt < RUNS_RETRY_DELAYS_MS.length) {
          retryTimer = window.setTimeout(() => {
            void loadRuns(attempt + 1);
          }, RUNS_RETRY_DELAYS_MS[attempt]);
          return;
        }
        console.error('Failed to load runs:', err);
        dispatch({ type: 'SET_ERROR', error: 'Failed to load runs. Check API connection.' });
        dispatch({ type: 'RUNS_LOADED', runs: [] });
      }
    };

    dispatch({ type: 'SET_ERROR', error: null });
    dispatch({ type: 'RUNS_LOADING' });
    void loadRuns(0);

    return () => {
      cancelled = true;
      if (retryTimer !== null) {
        window.clearTimeout(retryTimer);
      }
    };
  }, [provider]);

  // Load tree when run selected
  useEffect(() => {
    if (!state.selectedRunId) return;
    const runId = state.selectedRunId;
    Promise.all([
      provider.getRunTree(runId),
      provider.getRunRecord(runId),
      provider.getBranches(runId),
      provider.getSpanEdges(runId),
    ]).then(([tree, run, branches, edges]) => {
      dispatch({ type: 'TREE_LOADED', runId, tree, run, branches, edges });
    }).catch(err => {
      console.error('Failed to load run tree:', err);
      dispatch({ type: 'SET_ERROR', error: 'Failed to load run tree. Check API connection.' });
      dispatch({ type: 'TREE_LOADED', runId, tree: null, run: null, branches: [], edges: [] });
    });
  }, [provider, state.selectedRunId]);

  // Load timeline in parallel with tree
  useEffect(() => {
    if (!state.selectedRunId) return;
    const runId = state.selectedRunId;
    provider.getTimeline(runId).then(timeline => {
      dispatch({ type: 'TIMELINE_LOADED', runId, timeline });
    }).catch(err => {
      console.error('Failed to load timeline:', err);
      dispatch({ type: 'SET_ERROR', error: 'Failed to load timeline.' });
      dispatch({ type: 'TIMELINE_LOADED', runId, timeline: null });
    });
  }, [provider, state.selectedRunId]);

  // Load forensics in parallel with tree
  useEffect(() => {
    if (!state.selectedRunId) return;
    const runId = state.selectedRunId;
    provider.getForensics(runId).then(forensics => {
      dispatch({ type: 'FORENSICS_LOADED', runId, forensics });
    }).catch(err => {
      console.error('Failed to load forensics:', err);
      dispatch({ type: 'SET_ERROR', error: 'Failed to load forensics.' });
      dispatch({ type: 'FORENSICS_LOADED', runId, forensics: null });
    });
  }, [provider, state.selectedRunId]);

  // Load span detail when span selected
  useEffect(() => {
    if (!state.selectedRunId || !state.selectedSpanId) return;
    const runId = state.selectedRunId;
    const spanId = state.selectedSpanId;
    Promise.all([
      provider.getSpanDetail(runId, spanId),
      provider.getSpanArtifacts(runId, spanId),
      provider.getSpanEdges(runId),
    ]).then(([span, artifacts, edges]) => {
      dispatch({ type: 'SPAN_LOADED', runId, spanId, span, artifacts, edges });
    }).catch(err => {
      console.error('Failed to load span detail:', err);
      dispatch({ type: 'SET_ERROR', error: 'Failed to load span detail.' });
      dispatch({ type: 'SPAN_LOADED', runId, spanId, span: null, artifacts: [], edges: [] });
    });
  }, [provider, state.selectedRunId, state.selectedSpanId]);

  const selectRun = useCallback((runId: string) => {
    dispatch({ type: 'SELECT_RUN', runId });
  }, []);

  const selectSpan = useCallback((spanId: string) => {
    dispatch({ type: 'SELECT_SPAN', spanId });
  }, []);

  const setBottomTab = useCallback((tab: BottomTab) => {
    dispatch({ type: 'SET_BOTTOM_TAB', tab });
  }, []);

  const startBranchDraft = useCallback((span: SpanRecord) => {
    dispatch({ type: 'START_BRANCH_DRAFT', span });
  }, []);

  const updateBranchDraft = useCallback((draft: Partial<BranchDraftState>) => {
    dispatch({ type: 'UPDATE_BRANCH_DRAFT', draft });
  }, []);

  const cancelBranchDraft = useCallback(() => {
    dispatch({ type: 'CANCEL_BRANCH_DRAFT' });
  }, []);

  const submitBranch = useCallback(async () => {
    if (!state.branchDraft) return;
    try {
      const branch = await provider.createBranch(state.branchDraft);
      dispatch({ type: 'SET_ERROR', error: null });
      dispatch({ type: 'BRANCH_CREATED', branch });
    } catch (err) {
      console.error('Failed to create branch:', err);
      dispatch({
        type: 'SET_ERROR',
        error: `Failed to create branch. ${errorMessage(err, 'Unknown error.')}`,
      });
    }
  }, [provider, state.branchDraft]);

  const loadDiff = useCallback(async (sourceRunId: string, targetRunId: string) => {
    const diff = await provider.getDiffSummary(sourceRunId, targetRunId);
    dispatch({ type: 'DIFF_LOADED', diff });
  }, [provider]);

  const jumpToSpan = useCallback((spanId: string) => {
    dispatch({ type: 'SELECT_SPAN', spanId });
  }, []);

  const setCenterView = useCallback((view: CenterView) => {
    dispatch({ type: 'SET_CENTER_VIEW', view });
  }, []);

  const clearError = useCallback(() => {
    dispatch({ type: 'SET_ERROR', error: null });
  }, []);

  return {
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
    clearError,
  };
}
