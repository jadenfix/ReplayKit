import { useReducer, useCallback, useEffect, useRef } from 'react';
import type {
  AppState, BottomTab, BranchDraftState,
  SpanRecord, RunRecord, SpanTreeNode,
  ArtifactRecord, SpanEdgeRecord, BranchRecord,
  DiffSummary, RunListItem, PatchType,
} from '../types';
import type { ReplayKitProvider } from '../providers';

// ── Actions ─────────────────────────────────────────────────────────

type Action =
  | { type: 'RUNS_LOADING' }
  | { type: 'RUNS_LOADED'; runs: RunListItem[] }
  | { type: 'TREE_LOADED'; tree: SpanTreeNode | null; run: RunRecord | null; branches: BranchRecord[]; edges: SpanEdgeRecord[] }
  | { type: 'SPAN_LOADED'; span: SpanRecord | null; artifacts: ArtifactRecord[]; edges: SpanEdgeRecord[] }
  | { type: 'SELECT_RUN'; runId: string }
  | { type: 'SELECT_SPAN'; spanId: string }
  | { type: 'SET_BOTTOM_TAB'; tab: BottomTab }
  | { type: 'START_BRANCH_DRAFT'; span: SpanRecord }
  | { type: 'UPDATE_BRANCH_DRAFT'; draft: Partial<BranchDraftState> }
  | { type: 'CANCEL_BRANCH_DRAFT' }
  | { type: 'BRANCH_CREATED'; branch: BranchRecord }
  | { type: 'DIFF_LOADED'; diff: DiffSummary | null };

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
  loading: { runs: true, tree: false, detail: false },
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
        diffSummary: null,
        branchDraft: null,
        loading: { ...state.loading, tree: true },
      };
    case 'TREE_LOADED':
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
  const providerRef = useRef(provider);
  providerRef.current = provider;

  // Load runs on mount
  useEffect(() => {
    dispatch({ type: 'RUNS_LOADING' });
    providerRef.current.listRuns().then(runs => {
      dispatch({ type: 'RUNS_LOADED', runs });
    }).catch(err => {
      console.error('Failed to load runs:', err);
      dispatch({ type: 'RUNS_LOADED', runs: [] });
    });
  }, []);

  // Load tree when run selected
  useEffect(() => {
    if (!state.selectedRunId) return;
    const runId = state.selectedRunId;
    Promise.all([
      providerRef.current.getRunTree(runId),
      providerRef.current.getRunRecord(runId),
      providerRef.current.getBranches(runId),
      providerRef.current.getSpanEdges(runId),
    ]).then(([tree, run, branches, edges]) => {
      dispatch({ type: 'TREE_LOADED', tree, run, branches, edges });
    }).catch(err => {
      console.error('Failed to load run tree:', err);
      dispatch({ type: 'TREE_LOADED', tree: null, run: null, branches: [], edges: [] });
    });
  }, [state.selectedRunId]);

  // Load span detail when span selected
  useEffect(() => {
    if (!state.selectedRunId || !state.selectedSpanId) return;
    const runId = state.selectedRunId;
    const spanId = state.selectedSpanId;
    Promise.all([
      providerRef.current.getSpanDetail(runId, spanId),
      providerRef.current.getSpanArtifacts(runId, spanId),
      providerRef.current.getSpanEdges(runId),
    ]).then(([span, artifacts, edges]) => {
      dispatch({ type: 'SPAN_LOADED', span, artifacts, edges });
    }).catch(err => {
      console.error('Failed to load span detail:', err);
      dispatch({ type: 'SPAN_LOADED', span: null, artifacts: [], edges: [] });
    });
  }, [state.selectedRunId, state.selectedSpanId]);

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
    const branch = await providerRef.current.createBranch(state.branchDraft);
    dispatch({ type: 'BRANCH_CREATED', branch });
  }, [state.branchDraft]);

  const loadDiff = useCallback(async (sourceRunId: string, targetRunId: string) => {
    const diff = await providerRef.current.getDiffSummary(sourceRunId, targetRunId);
    dispatch({ type: 'DIFF_LOADED', diff });
  }, []);

  const jumpToSpan = useCallback((spanId: string) => {
    dispatch({ type: 'SELECT_SPAN', spanId });
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
  };
}
