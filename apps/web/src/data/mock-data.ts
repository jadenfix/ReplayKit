// ── Realistic mock data for ReplayKit web alpha ────────────────────
// Tells a story: agent tried to fix a bug, tests failed, user branches
// from the failing span, patches the code, branch succeeds.

import type {
  RunRecord, SpanRecord, ArtifactRecord, SpanEdgeRecord,
  BranchRecord, RunListItem, SpanTreeNode, DiffSummary,
  TimelineView, ForensicsReport, TimelineEntryView,
} from '../types';

// ── Helpers ─────────────────────────────────────────────────────────

const T0 = 1712966400000; // 2024-04-13T00:00:00Z
const ms = (m: number, s = 0) => T0 + m * 60000 + s * 1000;

// ── Run Records ─────────────────────────────────────────────────────

export const RUNS: RunRecord[] = [
  {
    run_id: 'run_01', trace_id: 'trace_01', source_run_id: null,
    title: 'Fix login timeout bug', entrypoint: 'agent.main',
    adapter_name: 'replaykit-rust-tracing', adapter_version: '0.1.0',
    status: 'Failed', started_at: ms(0), ended_at: ms(3, 24),
    git_sha: 'a1b2c3d', environment_fingerprint: 'env_fp_01',
    labels: ['bugfix', 'auth'],
  },
  {
    run_id: 'run_02', trace_id: 'trace_02', source_run_id: 'run_01',
    title: 'Fix login timeout bug (branch)', entrypoint: 'agent.main',
    adapter_name: 'replaykit-rust-tracing', adapter_version: '0.1.0',
    status: 'Completed', started_at: ms(5), ended_at: ms(7, 12),
    git_sha: 'a1b2c3d', environment_fingerprint: 'env_fp_01',
    labels: ['bugfix', 'auth', 'branch'],
  },
  {
    run_id: 'run_03', trace_id: 'trace_03', source_run_id: null,
    title: 'Add pagination to user list API', entrypoint: 'agent.main',
    adapter_name: 'replaykit-rust-tracing', adapter_version: '0.1.0',
    status: 'Completed', started_at: ms(10), ended_at: ms(14, 48),
    git_sha: 'e5f6a7b', environment_fingerprint: 'env_fp_01',
    labels: ['feature', 'api'],
  },
  {
    run_id: 'run_04', trace_id: 'trace_04', source_run_id: null,
    title: 'Refactor database connection pool', entrypoint: 'agent.main',
    adapter_name: 'replaykit-rust-tracing', adapter_version: '0.1.0',
    status: 'Running', started_at: ms(20), ended_at: null,
    git_sha: 'c8d9e0f', environment_fingerprint: 'env_fp_02',
    labels: ['refactor', 'database'],
  },
];

// ── Spans for run_01 (Failed) ───────────────────────────────────────

const BASE_SPAN: Pick<SpanRecord, 'run_id' | 'trace_id' | 'environment_fingerprint' | 'snapshot_id' | 'dirty_reasons' | 'blocked_replay_reason' | 'attributes'> = {
  run_id: 'run_01', trace_id: 'trace_01',
  environment_fingerprint: 'env_fp_01', snapshot_id: null,
  dirty_reasons: [], blocked_replay_reason: null, attributes: {},
};

export const SPANS_RUN01: SpanRecord[] = [
  {
    ...BASE_SPAN, span_id: 's01_root', parent_span_id: null,
    sequence_no: 0, kind: 'Run', name: 'Fix login timeout bug',
    status: 'Failed', started_at: ms(0), ended_at: ms(3, 24),
    replay_policy: 'RecordOnly', executor_kind: null, executor_version: null,
    input_artifact_ids: ['art_prompt_01'], output_artifact_ids: [],
    input_fingerprint: 'fp_root', error_code: null,
    error_summary: 'Downstream test failure in auth module',
    failure_class: 'ShellFailure',
  },
  {
    ...BASE_SPAN, span_id: 's01_plan', parent_span_id: 's01_root',
    sequence_no: 1, kind: 'PlannerStep', name: 'Analyze issue',
    status: 'Completed', started_at: ms(0, 1), ended_at: ms(0, 18),
    replay_policy: 'RecordOnly', executor_kind: null, executor_version: null,
    input_artifact_ids: [], output_artifact_ids: [],
    input_fingerprint: 'fp_plan', error_code: null, error_summary: null, failure_class: null,
  },
  {
    ...BASE_SPAN, span_id: 's01_llm1', parent_span_id: 's01_plan',
    sequence_no: 2, kind: 'LlmCall', name: 'Understand bug report',
    status: 'Completed', started_at: ms(0, 2), ended_at: ms(0, 16),
    replay_policy: 'RerunnableSupported',
    executor_kind: 'llm.claude-sonnet', executor_version: '4.5',
    input_artifact_ids: ['art_prompt_01'], output_artifact_ids: ['art_analysis_01'],
    input_fingerprint: 'fp_llm1', error_code: null, error_summary: null, failure_class: null,
  },
  {
    ...BASE_SPAN, span_id: 's01_search', parent_span_id: 's01_root',
    sequence_no: 3, kind: 'PlannerStep', name: 'Search codebase',
    status: 'Completed', started_at: ms(0, 19), ended_at: ms(1, 5),
    replay_policy: 'RecordOnly', executor_kind: null, executor_version: null,
    input_artifact_ids: [], output_artifact_ids: [],
    input_fingerprint: 'fp_search', error_code: null, error_summary: null, failure_class: null,
  },
  {
    ...BASE_SPAN, span_id: 's01_tool1', parent_span_id: 's01_search',
    sequence_no: 4, kind: 'ToolCall', name: 'search_repository',
    status: 'Completed', started_at: ms(0, 20), ended_at: ms(0, 28),
    replay_policy: 'RerunnableSupported',
    executor_kind: 'tool.search_repository', executor_version: '1',
    input_artifact_ids: ['art_search_q'], output_artifact_ids: ['art_search_res'],
    input_fingerprint: 'fp_tool1', error_code: null, error_summary: null, failure_class: null,
  },
  {
    ...BASE_SPAN, span_id: 's01_tool2', parent_span_id: 's01_search',
    sequence_no: 5, kind: 'ToolCall', name: 'read_file login.rs',
    status: 'Completed', started_at: ms(0, 30), ended_at: ms(0, 32),
    replay_policy: 'PureReusable',
    executor_kind: 'tool.read_file', executor_version: '1',
    input_artifact_ids: ['art_path_login'], output_artifact_ids: ['art_login_content'],
    input_fingerprint: 'fp_tool2', error_code: null, error_summary: null, failure_class: null,
  },
  {
    ...BASE_SPAN, span_id: 's01_tool3', parent_span_id: 's01_search',
    sequence_no: 6, kind: 'ToolCall', name: 'read_file session.rs',
    status: 'Completed', started_at: ms(0, 34), ended_at: ms(0, 36),
    replay_policy: 'PureReusable',
    executor_kind: 'tool.read_file', executor_version: '1',
    input_artifact_ids: ['art_path_session'], output_artifact_ids: ['art_session_content'],
    input_fingerprint: 'fp_tool3', error_code: null, error_summary: null, failure_class: null,
  },
  {
    ...BASE_SPAN, span_id: 's01_fix', parent_span_id: 's01_root',
    sequence_no: 7, kind: 'PlannerStep', name: 'Implement fix',
    status: 'Completed', started_at: ms(1, 6), ended_at: ms(2, 10),
    replay_policy: 'RecordOnly', executor_kind: null, executor_version: null,
    input_artifact_ids: [], output_artifact_ids: [],
    input_fingerprint: 'fp_fix', error_code: null, error_summary: null, failure_class: null,
  },
  {
    ...BASE_SPAN, span_id: 's01_llm2', parent_span_id: 's01_fix',
    sequence_no: 8, kind: 'LlmCall', name: 'Generate fix plan',
    status: 'Completed', started_at: ms(1, 8), ended_at: ms(1, 42),
    replay_policy: 'RerunnableSupported',
    executor_kind: 'llm.claude-sonnet', executor_version: '4.5',
    input_artifact_ids: ['art_analysis_01', 'art_login_content', 'art_session_content'],
    output_artifact_ids: ['art_fix_plan'],
    input_fingerprint: 'fp_llm2', error_code: null, error_summary: null, failure_class: null,
  },
  {
    ...BASE_SPAN, span_id: 's01_write1', parent_span_id: 's01_fix',
    sequence_no: 9, kind: 'FileWrite', name: 'write_file login.rs',
    status: 'Completed', started_at: ms(1, 44), ended_at: ms(1, 45),
    replay_policy: 'RerunnableSupported',
    executor_kind: 'tool.write_file', executor_version: '1',
    input_artifact_ids: ['art_fix_content'], output_artifact_ids: ['art_write_confirm'],
    input_fingerprint: 'fp_write1', error_code: null, error_summary: null, failure_class: null,
  },
  {
    ...BASE_SPAN, span_id: 's01_validate', parent_span_id: 's01_root',
    sequence_no: 10, kind: 'PlannerStep', name: 'Validate fix',
    status: 'Failed', started_at: ms(2, 11), ended_at: ms(3, 2),
    replay_policy: 'RecordOnly', executor_kind: null, executor_version: null,
    input_artifact_ids: [], output_artifact_ids: [],
    input_fingerprint: 'fp_validate', error_code: null,
    error_summary: 'Test suite failed: 2 of 8 tests failed',
    failure_class: 'ShellFailure',
  },
  {
    ...BASE_SPAN, span_id: 's01_shell1', parent_span_id: 's01_validate',
    sequence_no: 11, kind: 'ShellCommand', name: 'cargo test auth',
    status: 'Failed', started_at: ms(2, 12), ended_at: ms(2, 48),
    replay_policy: 'RerunnableSupported',
    executor_kind: 'shell.cargo', executor_version: '1.78',
    input_artifact_ids: ['art_test_cmd'], output_artifact_ids: ['art_test_output'],
    input_fingerprint: 'fp_shell1', error_code: 'EXIT_101',
    error_summary: 'test auth::session::test_async_timeout ... FAILED\ntest auth::session::test_concurrent_login ... FAILED',
    failure_class: 'ShellFailure',
  },
  {
    ...BASE_SPAN, span_id: 's01_llm3', parent_span_id: 's01_validate',
    sequence_no: 12, kind: 'LlmCall', name: 'Analyze test failure',
    status: 'Completed', started_at: ms(2, 50), ended_at: ms(3, 0),
    replay_policy: 'RerunnableSupported',
    executor_kind: 'llm.claude-sonnet', executor_version: '4.5',
    input_artifact_ids: ['art_test_output'], output_artifact_ids: ['art_failure_analysis'],
    input_fingerprint: 'fp_llm3', error_code: null, error_summary: null, failure_class: null,
  },
  {
    ...BASE_SPAN, span_id: 's01_report', parent_span_id: 's01_root',
    sequence_no: 13, kind: 'PlannerStep', name: 'Report results',
    status: 'Blocked', started_at: ms(3, 3), ended_at: ms(3, 3),
    replay_policy: 'RecordOnly', executor_kind: null, executor_version: null,
    input_artifact_ids: [], output_artifact_ids: [],
    input_fingerprint: 'fp_report', error_code: null,
    error_summary: null, failure_class: null,
    blocked_replay_reason: 'Upstream span "Validate fix" failed; cannot produce meaningful report',
  },
  {
    ...BASE_SPAN, span_id: 's01_llm4', parent_span_id: 's01_report',
    sequence_no: 14, kind: 'LlmCall', name: 'Generate summary',
    status: 'Blocked', started_at: ms(3, 3), ended_at: ms(3, 3),
    replay_policy: 'RerunnableSupported',
    executor_kind: 'llm.claude-sonnet', executor_version: '4.5',
    input_artifact_ids: [], output_artifact_ids: [],
    input_fingerprint: 'fp_llm4', error_code: null,
    error_summary: null, failure_class: null,
    blocked_replay_reason: 'Blocked: parent step "Report results" is blocked due to upstream failure',
  },
];

// ── Edges for run_01 ────────────────────────────────────────────────

export const EDGES_RUN01: SpanEdgeRecord[] = [
  { run_id: 'run_01', from_span_id: 's01_root', to_span_id: 's01_plan', kind: 'ControlParent' },
  { run_id: 'run_01', from_span_id: 's01_plan', to_span_id: 's01_llm1', kind: 'ControlParent' },
  { run_id: 'run_01', from_span_id: 's01_root', to_span_id: 's01_search', kind: 'ControlParent' },
  { run_id: 'run_01', from_span_id: 's01_search', to_span_id: 's01_tool1', kind: 'ControlParent' },
  { run_id: 'run_01', from_span_id: 's01_search', to_span_id: 's01_tool2', kind: 'ControlParent' },
  { run_id: 'run_01', from_span_id: 's01_search', to_span_id: 's01_tool3', kind: 'ControlParent' },
  { run_id: 'run_01', from_span_id: 's01_root', to_span_id: 's01_fix', kind: 'ControlParent' },
  { run_id: 'run_01', from_span_id: 's01_fix', to_span_id: 's01_llm2', kind: 'ControlParent' },
  { run_id: 'run_01', from_span_id: 's01_fix', to_span_id: 's01_write1', kind: 'ControlParent' },
  { run_id: 'run_01', from_span_id: 's01_root', to_span_id: 's01_validate', kind: 'ControlParent' },
  { run_id: 'run_01', from_span_id: 's01_validate', to_span_id: 's01_shell1', kind: 'ControlParent' },
  { run_id: 'run_01', from_span_id: 's01_validate', to_span_id: 's01_llm3', kind: 'ControlParent' },
  { run_id: 'run_01', from_span_id: 's01_root', to_span_id: 's01_report', kind: 'ControlParent' },
  { run_id: 'run_01', from_span_id: 's01_report', to_span_id: 's01_llm4', kind: 'ControlParent' },
  // Data dependencies
  { run_id: 'run_01', from_span_id: 's01_llm1', to_span_id: 's01_llm2', kind: 'DataDependsOn' },
  { run_id: 'run_01', from_span_id: 's01_tool2', to_span_id: 's01_llm2', kind: 'DataDependsOn' },
  { run_id: 'run_01', from_span_id: 's01_tool3', to_span_id: 's01_llm2', kind: 'DataDependsOn' },
  { run_id: 'run_01', from_span_id: 's01_write1', to_span_id: 's01_shell1', kind: 'DataDependsOn' },
  { run_id: 'run_01', from_span_id: 's01_shell1', to_span_id: 's01_llm3', kind: 'DataDependsOn' },
];

// ── Artifacts for run_01 ────────────────────────────────────────────

export const ARTIFACTS_RUN01: ArtifactRecord[] = [
  {
    artifact_id: 'art_prompt_01', run_id: 'run_01', span_id: 's01_llm1',
    type: 'prompt', mime: 'text/plain', byte_len: 312,
    summary: 'Bug report prompt',
    content: `You are a coding agent. The user reports a login timeout bug:

"Login requests hang for 30+ seconds before timing out. The issue started
after the session refactor in commit a1b2c3d. Affects async login paths only.
Sync login works fine."

Analyze this bug report and identify likely root causes.`,
  },
  {
    artifact_id: 'art_analysis_01', run_id: 'run_01', span_id: 's01_llm1',
    type: 'llm_response', mime: 'text/plain', byte_len: 847,
    summary: 'Bug analysis output',
    content: `Analysis of login timeout bug:

Root cause hypothesis: The session refactor likely changed the async session
initialization to not properly await the connection pool. This would cause
the login handler to hang waiting for a session that never completes setup.

Key files to investigate:
1. src/auth/login.rs - the login handler, specifically async paths
2. src/auth/session.rs - session initialization after refactor

The fact that sync login works but async doesn't points to a missing .await
or incorrect Future handling in the session setup path.

Recommended approach:
1. Read login.rs and session.rs
2. Check the async session initialization
3. Add proper timeout handling and fix the await chain`,
  },
  {
    artifact_id: 'art_search_q', run_id: 'run_01', span_id: 's01_tool1',
    type: 'tool_input', mime: 'application/json', byte_len: 48,
    summary: 'Search query',
    content: JSON.stringify({ query: "login timeout session async", max_results: 10 }, null, 2),
  },
  {
    artifact_id: 'art_search_res', run_id: 'run_01', span_id: 's01_tool1',
    type: 'tool_output', mime: 'application/json', byte_len: 256,
    summary: 'Search results: 3 files',
    content: JSON.stringify({
      matches: [
        { file: "src/auth/login.rs", line: 42, snippet: "async fn handle_login(req: LoginRequest) -> Result<Session>" },
        { file: "src/auth/session.rs", line: 18, snippet: "pub async fn init_session(pool: &Pool) -> Session" },
        { file: "src/auth/session.rs", line: 67, snippet: "fn setup_timeout(duration: Duration) -> Timeout" },
      ]
    }, null, 2),
  },
  {
    artifact_id: 'art_path_login', run_id: 'run_01', span_id: 's01_tool2',
    type: 'tool_input', mime: 'text/plain', byte_len: 20,
    summary: null, content: 'src/auth/login.rs',
  },
  {
    artifact_id: 'art_login_content', run_id: 'run_01', span_id: 's01_tool2',
    type: 'file_content', mime: 'text/x-rust', byte_len: 1024,
    summary: 'login.rs source',
    content: `use crate::auth::session::{init_session, Session};
use crate::pool::Pool;

pub async fn handle_login(req: LoginRequest, pool: &Pool) -> Result<Session> {
    let credentials = validate_credentials(&req)?;

    // BUG: init_session is async but we're not handling
    // the timeout properly for the async path
    let session = init_session(pool).await;

    session.authenticate(credentials).await?;

    Ok(session)
}

pub fn handle_login_sync(req: LoginRequest, pool: &Pool) -> Result<Session> {
    let credentials = validate_credentials(&req)?;
    let session = pool.get_session_sync();
    session.authenticate_sync(credentials)?;
    Ok(session)
}`,
  },
  {
    artifact_id: 'art_path_session', run_id: 'run_01', span_id: 's01_tool3',
    type: 'tool_input', mime: 'text/plain', byte_len: 22,
    summary: null, content: 'src/auth/session.rs',
  },
  {
    artifact_id: 'art_session_content', run_id: 'run_01', span_id: 's01_tool3',
    type: 'file_content', mime: 'text/x-rust', byte_len: 890,
    summary: 'session.rs source',
    content: `use tokio::time::{timeout, Duration};

pub async fn init_session(pool: &Pool) -> Session {
    // After refactor: this spawns a background task but doesn't
    // properly propagate the timeout to the connection acquisition
    let conn = pool.acquire().await;
    Session::new(conn)
}

pub fn setup_timeout(duration: Duration) -> Timeout {
    timeout(duration, async {
        // This timeout wrapper exists but isn't used in init_session
    })
}`,
  },
  {
    artifact_id: 'art_fix_plan', run_id: 'run_01', span_id: 's01_llm2',
    type: 'llm_response', mime: 'text/plain', byte_len: 420,
    summary: 'Fix plan',
    content: `Fix plan for login timeout:

1. Wrap init_session with timeout in handle_login
2. Add timeout parameter to init_session itself
3. Propagate timeout to pool.acquire()

The fix needs to handle both the session init timeout AND the
concurrent login case where multiple async logins compete for
pool connections.`,
  },
  {
    artifact_id: 'art_fix_content', run_id: 'run_01', span_id: 's01_write1',
    type: 'file_diff', mime: 'text/x-diff', byte_len: 380,
    summary: 'Patch to login.rs',
    content: `--- a/src/auth/login.rs
+++ b/src/auth/login.rs
@@ -5,7 +5,9 @@
 pub async fn handle_login(req: LoginRequest, pool: &Pool) -> Result<Session> {
     let credentials = validate_credentials(&req)?;

-    let session = init_session(pool).await;
+    let session = tokio::time::timeout(
+        Duration::from_secs(5),
+        init_session(pool),
+    ).await.map_err(|_| AuthError::Timeout)?;

     session.authenticate(credentials).await?;`,
  },
  {
    artifact_id: 'art_write_confirm', run_id: 'run_01', span_id: 's01_write1',
    type: 'tool_output', mime: 'application/json', byte_len: 42,
    summary: 'Write confirmed',
    content: JSON.stringify({ ok: true, path: "src/auth/login.rs", bytes_written: 380 }, null, 2),
  },
  {
    artifact_id: 'art_test_cmd', run_id: 'run_01', span_id: 's01_shell1',
    type: 'shell_input', mime: 'text/plain', byte_len: 24,
    summary: null, content: 'cargo test auth -- --nocapture',
  },
  {
    artifact_id: 'art_test_output', run_id: 'run_01', span_id: 's01_shell1',
    type: 'shell_output', mime: 'text/x-shell-log', byte_len: 1200,
    summary: '2 of 8 tests failed',
    content: `running 8 tests
test auth::login::test_validate_credentials ... ok
test auth::login::test_handle_login_sync ... ok
test auth::login::test_handle_login_basic ... ok
test auth::login::test_invalid_credentials ... ok
test auth::session::test_init_session ... ok
test auth::session::test_setup_timeout ... ok
test auth::session::test_async_timeout ... FAILED
test auth::session::test_concurrent_login ... FAILED

failures:

---- auth::session::test_async_timeout stdout ----
thread 'test_async_timeout' panicked at 'assertion failed: timeout fires
before session init completes'
note: The timeout was added to handle_login but init_session itself still
blocks indefinitely on pool.acquire() when pool is exhausted.

---- auth::session::test_concurrent_login stdout ----
thread 'test_concurrent_login' panicked at 'assertion failed: concurrent
logins should not deadlock'
note: When multiple async logins run concurrently, they can exhaust the
pool and deadlock because init_session doesn't release on timeout.

failures:
    auth::session::test_async_timeout
    auth::session::test_concurrent_login

test result: FAILED. 6 passed; 2 failed; 0 ignored`,
  },
  {
    artifact_id: 'art_failure_analysis', run_id: 'run_01', span_id: 's01_llm3',
    type: 'llm_response', mime: 'text/plain', byte_len: 520,
    summary: 'Root cause: incomplete timeout propagation',
    content: `Test failure analysis:

The fix was incomplete. Adding timeout to handle_login is necessary but
not sufficient. The root issues are:

1. init_session's pool.acquire() has no internal timeout - it will block
   forever if the pool is exhausted
2. The timeout wrapper in handle_login will cancel the Future but won't
   release the pool connection attempt, leading to connection leak
3. Concurrent logins can deadlock because exhausted pool + no release = stuck

Required additional changes:
- Add timeout to pool.acquire() inside init_session
- Add connection release on cancellation (drop guard)
- Add pool size limit check before attempting acquire`,
  },
];

// ── Spans for run_03 (simple successful run) ────────────────────────

const BASE_SPAN_03: Pick<SpanRecord, 'run_id' | 'trace_id' | 'environment_fingerprint' | 'snapshot_id' | 'dirty_reasons' | 'blocked_replay_reason' | 'attributes'> = {
  run_id: 'run_03', trace_id: 'trace_03',
  environment_fingerprint: 'env_fp_01', snapshot_id: null,
  dirty_reasons: [], blocked_replay_reason: null, attributes: {},
};

export const SPANS_RUN03: SpanRecord[] = [
  {
    ...BASE_SPAN_03, span_id: 's03_root', parent_span_id: null,
    sequence_no: 0, kind: 'Run', name: 'Add pagination to user list API',
    status: 'Completed', started_at: ms(10), ended_at: ms(14, 48),
    replay_policy: 'RecordOnly', executor_kind: null, executor_version: null,
    input_artifact_ids: [], output_artifact_ids: [],
    input_fingerprint: 'fp_r3_root', error_code: null, error_summary: null, failure_class: null,
  },
  {
    ...BASE_SPAN_03, span_id: 's03_plan', parent_span_id: 's03_root',
    sequence_no: 1, kind: 'PlannerStep', name: 'Plan pagination implementation',
    status: 'Completed', started_at: ms(10, 1), ended_at: ms(10, 30),
    replay_policy: 'RecordOnly', executor_kind: null, executor_version: null,
    input_artifact_ids: [], output_artifact_ids: [],
    input_fingerprint: 'fp_r3_plan', error_code: null, error_summary: null, failure_class: null,
  },
  {
    ...BASE_SPAN_03, span_id: 's03_llm1', parent_span_id: 's03_plan',
    sequence_no: 2, kind: 'LlmCall', name: 'Design pagination API',
    status: 'Completed', started_at: ms(10, 2), ended_at: ms(10, 28),
    replay_policy: 'RerunnableSupported',
    executor_kind: 'llm.claude-sonnet', executor_version: '4.5',
    input_artifact_ids: [], output_artifact_ids: [],
    input_fingerprint: 'fp_r3_llm1', error_code: null, error_summary: null, failure_class: null,
  },
  {
    ...BASE_SPAN_03, span_id: 's03_impl', parent_span_id: 's03_root',
    sequence_no: 3, kind: 'PlannerStep', name: 'Implement changes',
    status: 'Completed', started_at: ms(10, 31), ended_at: ms(13, 0),
    replay_policy: 'RecordOnly', executor_kind: null, executor_version: null,
    input_artifact_ids: [], output_artifact_ids: [],
    input_fingerprint: 'fp_r3_impl', error_code: null, error_summary: null, failure_class: null,
  },
  {
    ...BASE_SPAN_03, span_id: 's03_write1', parent_span_id: 's03_impl',
    sequence_no: 4, kind: 'FileWrite', name: 'write_file users.rs',
    status: 'Completed', started_at: ms(11, 0), ended_at: ms(11, 2),
    replay_policy: 'RerunnableSupported',
    executor_kind: 'tool.write_file', executor_version: '1',
    input_artifact_ids: [], output_artifact_ids: [],
    input_fingerprint: 'fp_r3_w1', error_code: null, error_summary: null, failure_class: null,
  },
  {
    ...BASE_SPAN_03, span_id: 's03_write2', parent_span_id: 's03_impl',
    sequence_no: 5, kind: 'FileWrite', name: 'write_file pagination.rs',
    status: 'Completed', started_at: ms(11, 5), ended_at: ms(11, 7),
    replay_policy: 'RerunnableSupported',
    executor_kind: 'tool.write_file', executor_version: '1',
    input_artifact_ids: [], output_artifact_ids: [],
    input_fingerprint: 'fp_r3_w2', error_code: null, error_summary: null, failure_class: null,
  },
  {
    ...BASE_SPAN_03, span_id: 's03_test', parent_span_id: 's03_root',
    sequence_no: 6, kind: 'ShellCommand', name: 'cargo test users',
    status: 'Completed', started_at: ms(13, 5), ended_at: ms(14, 30),
    replay_policy: 'RerunnableSupported',
    executor_kind: 'shell.cargo', executor_version: '1.78',
    input_artifact_ids: [], output_artifact_ids: [],
    input_fingerprint: 'fp_r3_test', error_code: null, error_summary: null, failure_class: null,
  },
];

// ── Spans for run_04 (running) ──────────────────────────────────────

const BASE_SPAN_04: Pick<SpanRecord, 'run_id' | 'trace_id' | 'environment_fingerprint' | 'snapshot_id' | 'dirty_reasons' | 'blocked_replay_reason' | 'attributes'> = {
  run_id: 'run_04', trace_id: 'trace_04',
  environment_fingerprint: 'env_fp_02', snapshot_id: null,
  dirty_reasons: [], blocked_replay_reason: null, attributes: {},
};

export const SPANS_RUN04: SpanRecord[] = [
  {
    ...BASE_SPAN_04, span_id: 's04_root', parent_span_id: null,
    sequence_no: 0, kind: 'Run', name: 'Refactor database connection pool',
    status: 'Running', started_at: ms(20), ended_at: null,
    replay_policy: 'RecordOnly', executor_kind: null, executor_version: null,
    input_artifact_ids: [], output_artifact_ids: [],
    input_fingerprint: 'fp_r4_root', error_code: null, error_summary: null, failure_class: null,
  },
  {
    ...BASE_SPAN_04, span_id: 's04_plan', parent_span_id: 's04_root',
    sequence_no: 1, kind: 'PlannerStep', name: 'Analyze current pool implementation',
    status: 'Completed', started_at: ms(20, 1), ended_at: ms(20, 45),
    replay_policy: 'RecordOnly', executor_kind: null, executor_version: null,
    input_artifact_ids: [], output_artifact_ids: [],
    input_fingerprint: 'fp_r4_plan', error_code: null, error_summary: null, failure_class: null,
  },
  {
    ...BASE_SPAN_04, span_id: 's04_llm1', parent_span_id: 's04_plan',
    sequence_no: 2, kind: 'LlmCall', name: 'Generate refactor plan',
    status: 'Running', started_at: ms(20, 46), ended_at: null,
    replay_policy: 'RerunnableSupported',
    executor_kind: 'llm.claude-sonnet', executor_version: '4.5',
    input_artifact_ids: [], output_artifact_ids: [],
    input_fingerprint: 'fp_r4_llm1', error_code: null, error_summary: null, failure_class: null,
  },
];

// ── Branches ────────────────────────────────────────────────────────

export const BRANCHES: BranchRecord[] = [
  {
    branch_id: 'branch_01',
    source_run_id: 'run_01',
    target_run_id: 'run_02',
    fork_span_id: 's01_write1',
    patch_type: 'ToolOutputOverride',
    patch_summary: 'Override write_file output with complete async fix including pool timeout and drop guard',
    created_at: ms(4, 30),
    status: 'Completed',
  },
];

// ── Diff between run_01 and run_02 ──────────────────────────────────

export const DIFF_01_02: DiffSummary = {
  diff_id: 'diff_01',
  source_run_id: 'run_01',
  target_run_id: 'run_02',
  first_divergent_span_id: 's01_write1',
  status_change: { from: 'Failed', to: 'Completed' },
  latency_ms_delta: -1200,
  token_delta: 142,
  changed_span_count: 5,
  changed_artifact_count: 8,
  final_output_changed: true,
  span_diffs: [
    {
      span_id_source: 's01_write1', span_id_target: 's02_write1',
      name: 'write_file login.rs',
      status_change: null,
      duration_ms_delta: 100,
      output_changed: true,
      dirty_reason: 'PatchedInput',
    },
    {
      span_id_source: 's01_shell1', span_id_target: 's02_shell1',
      name: 'cargo test auth',
      status_change: { from: 'Failed', to: 'Completed' },
      duration_ms_delta: -800,
      output_changed: true,
      dirty_reason: 'UpstreamOutputChanged',
    },
    {
      span_id_source: 's01_llm3', span_id_target: 's02_llm3',
      name: 'Analyze test results',
      status_change: null,
      duration_ms_delta: -200,
      output_changed: true,
      dirty_reason: 'UpstreamOutputChanged',
    },
    {
      span_id_source: 's01_report', span_id_target: 's02_report',
      name: 'Report results',
      status_change: { from: 'Blocked', to: 'Completed' },
      duration_ms_delta: 4200,
      output_changed: true,
      dirty_reason: 'UpstreamOutputChanged',
    },
    {
      span_id_source: 's01_llm4', span_id_target: 's02_llm4',
      name: 'Generate summary',
      status_change: { from: 'Blocked', to: 'Completed' },
      duration_ms_delta: 3800,
      output_changed: true,
      dirty_reason: 'UpstreamOutputChanged',
    },
  ],
};

// ── Run list view model ─────────────────────────────────────────────

export const RUN_LIST: RunListItem[] = [
  {
    run_id: 'run_04', title: 'Refactor database connection pool',
    status: 'Running', started_at: ms(20), duration_ms: null,
    adapter_name: 'replaykit-rust-tracing', failure_summary: null,
    source_run_id: null, span_count: 3, error_count: 0,
  },
  {
    run_id: 'run_03', title: 'Add pagination to user list API',
    status: 'Completed', started_at: ms(10), duration_ms: 288000,
    adapter_name: 'replaykit-rust-tracing', failure_summary: null,
    source_run_id: null, span_count: 7, error_count: 0,
  },
  {
    run_id: 'run_02', title: 'Fix login timeout bug (branch)',
    status: 'Completed', started_at: ms(5), duration_ms: 132000,
    adapter_name: 'replaykit-rust-tracing', failure_summary: null,
    source_run_id: 'run_01', span_count: 15, error_count: 0,
  },
  {
    run_id: 'run_01', title: 'Fix login timeout bug',
    status: 'Failed', started_at: ms(0), duration_ms: 204000,
    adapter_name: 'replaykit-rust-tracing',
    failure_summary: 'Test failure: auth::session (2 of 8 tests failed)',
    source_run_id: null, span_count: 15, error_count: 2,
  },
];

// ── Tree builder ────────────────────────────────────────────────────

export function buildTree(spans: SpanRecord[]): SpanTreeNode | null {
  const childrenMap = new Map<string, SpanRecord[]>();

  for (const s of spans) {
    const pid = s.parent_span_id ?? '__root__';
    const arr = childrenMap.get(pid) ?? [];
    arr.push(s);
    childrenMap.set(pid, arr);
  }

  function build(span: SpanRecord, depth: number): SpanTreeNode {
    const kids = childrenMap.get(span.span_id) ?? [];
    kids.sort((a, b) => a.sequence_no - b.sequence_no);
    return {
      span,
      children: kids.map(k => build(k, depth + 1)),
      depth,
    };
  }

  const roots = childrenMap.get('__root__') ?? [];
  if (roots.length === 0) return null;
  roots.sort((a, b) => a.sequence_no - b.sequence_no);
  return build(roots[0], 0);
}

// ── Spans and artifacts by run ──────────────────────────────────────

const SPANS_BY_RUN: Record<string, SpanRecord[]> = {
  run_01: SPANS_RUN01,
  run_03: SPANS_RUN03,
  run_04: SPANS_RUN04,
};

// For run_02, we reuse run_01 spans with slightly modified data
const SPANS_RUN02: SpanRecord[] = SPANS_RUN01.map(s => ({
  ...s,
  run_id: 'run_02',
  trace_id: 'trace_02',
  span_id: s.span_id.replace('s01_', 's02_'),
  parent_span_id: s.parent_span_id?.replace('s01_', 's02_') ?? null,
  input_artifact_ids: s.input_artifact_ids.map(id => id + '_b'),
  output_artifact_ids: s.output_artifact_ids.map(id => id + '_b'),
  // Fix the statuses for the branch run
  ...(s.span_id === 's01_root' ? { status: 'Completed' as const, error_summary: null, failure_class: null } : {}),
  ...(s.span_id === 's01_validate' ? { status: 'Completed' as const, error_summary: null, failure_class: null } : {}),
  ...(s.span_id === 's01_shell1' ? { status: 'Completed' as const, error_code: null, error_summary: null, failure_class: null, dirty_reasons: ['UpstreamOutputChanged' as const] } : {}),
  ...(s.span_id === 's01_llm3' ? { name: 'Analyze test results', dirty_reasons: ['UpstreamOutputChanged' as const] } : {}),
  ...(s.span_id === 's01_report' ? { status: 'Completed' as const, blocked_replay_reason: null } : {}),
  ...(s.span_id === 's01_llm4' ? { status: 'Completed' as const, blocked_replay_reason: null, dirty_reasons: ['UpstreamOutputChanged' as const] } : {}),
  ...(s.span_id === 's01_write1' ? { dirty_reasons: ['PatchedInput' as const] } : {}),
}));

SPANS_BY_RUN['run_02'] = SPANS_RUN02;

const ARTIFACTS_BY_RUN: Record<string, ArtifactRecord[]> = {
  run_01: ARTIFACTS_RUN01,
  run_02: ARTIFACTS_RUN01.map(a => ({
    ...a,
    run_id: 'run_02',
    artifact_id: a.artifact_id + '_b',
    span_id: a.span_id?.replace('s01_', 's02_') ?? null,
  })),
  run_03: [],
  run_04: [],
};

const EDGES_BY_RUN: Record<string, SpanEdgeRecord[]> = {
  run_01: EDGES_RUN01,
  run_02: EDGES_RUN01.map(e => ({
    ...e, run_id: 'run_02',
    from_span_id: e.from_span_id.replace('s01_', 's02_'),
    to_span_id: e.to_span_id.replace('s01_', 's02_'),
  })),
  run_03: [],
  run_04: [],
};

export function getSpansForRun(runId: string): SpanRecord[] {
  return SPANS_BY_RUN[runId] ?? [];
}

export function getArtifactsForSpan(runId: string, spanId: string): ArtifactRecord[] {
  const arts = ARTIFACTS_BY_RUN[runId] ?? [];
  return arts.filter(a => a.span_id === spanId);
}

export function getEdgesForRun(runId: string): SpanEdgeRecord[] {
  return EDGES_BY_RUN[runId] ?? [];
}

export function getRunRecord(runId: string): RunRecord | undefined {
  return RUNS.find(r => r.run_id === runId);
}

export function getDiffForRuns(sourceId: string, targetId: string): DiffSummary | null {
  if (sourceId === 'run_01' && targetId === 'run_02') return DIFF_01_02;
  if (sourceId === 'run_02' && targetId === 'run_01') return DIFF_01_02;
  return null;
}

// ── Timeline mock data ──────────────────────────────────────────────

function buildTimelineForRun(runId: string): TimelineView | null {
  const run = RUNS.find(r => r.run_id === runId);
  if (!run) return null;
  const spans = getSpansForRun(runId);
  if (spans.length === 0) return null;

  // Compute depth from parent_span_id chains
  const depthMap = new Map<string, number>();
  for (const s of spans) {
    if (!s.parent_span_id) {
      depthMap.set(s.span_id, 0);
    }
  }
  // Multiple passes to resolve
  for (let pass = 0; pass < 5; pass++) {
    for (const s of spans) {
      if (s.parent_span_id && depthMap.has(s.parent_span_id)) {
        depthMap.set(s.span_id, (depthMap.get(s.parent_span_id) ?? 0) + 1);
      }
    }
  }

  const sorted = [...spans].sort((a, b) => a.started_at - b.started_at);
  const entries: TimelineEntryView[] = sorted.map(s => ({
    span_id: s.span_id,
    name: s.name,
    kind: s.kind,
    status: s.status,
    status_label: s.status,
    started_at: s.started_at,
    ended_at: s.ended_at,
    depth: depthMap.get(s.span_id) ?? 0,
    parent_span_id: s.parent_span_id,
    error_summary: s.error_summary,
  }));

  const minStart = Math.min(...entries.map(e => e.started_at));
  const maxEnd = Math.max(...entries.map(e => e.ended_at ?? e.started_at));

  return {
    run_id: run.run_id,
    title: run.title,
    status: run.status,
    total_started_at: minStart,
    total_ended_at: maxEnd,
    entries,
  };
}

export function getTimelineForRun(runId: string): TimelineView | null {
  return buildTimelineForRun(runId);
}

// ── Forensics mock data ────────────────────────────────────────────

export function getForensicsForRun(runId: string): ForensicsReport | null {
  const run = RUNS.find(r => r.run_id === runId);
  if (!run) return null;

  const spans = getSpansForRun(runId);
  const failedSpans = spans.filter(s => s.status === 'Failed');
  const blockedSpans = spans.filter(s => s.status === 'Blocked');

  if (failedSpans.length === 0 && blockedSpans.length === 0) {
    return {
      run_id: runId,
      has_failure: false,
      first_failed_span_id: null,
      deepest_failed_span_id: null,
      deepest_failing_dependency_id: null,
      failure_path: [],
      blocked_spans: [],
      retry_groups: [],
    };
  }

  // Find first and deepest failed
  const sortedFailed = [...failedSpans].sort((a, b) => a.started_at - b.started_at);
  const firstFailed = sortedFailed[0]?.span_id ?? null;

  // Deepest = the failed span with greatest depth
  const depthMap = new Map<string, number>();
  for (const s of spans) {
    if (!s.parent_span_id) depthMap.set(s.span_id, 0);
  }
  for (let pass = 0; pass < 5; pass++) {
    for (const s of spans) {
      if (s.parent_span_id && depthMap.has(s.parent_span_id)) {
        depthMap.set(s.span_id, (depthMap.get(s.parent_span_id) ?? 0) + 1);
      }
    }
  }
  const deepestFailed = failedSpans.reduce<SpanRecord | null>((best, s) => {
    const d = depthMap.get(s.span_id) ?? 0;
    const bestD = best ? (depthMap.get(best.span_id) ?? 0) : -1;
    return d > bestD ? s : best;
  }, null);

  // Build failure path from root to deepest
  const failurePath: string[] = [];
  if (deepestFailed) {
    let current: string | null = deepestFailed.span_id;
    while (current) {
      failurePath.unshift(current);
      const span = spans.find(s => s.span_id === current);
      current = span?.parent_span_id ?? null;
    }
  }

  return {
    run_id: runId,
    has_failure: failedSpans.length > 0,
    first_failed_span_id: firstFailed,
    deepest_failed_span_id: deepestFailed?.span_id ?? null,
    deepest_failing_dependency_id: null,
    failure_path: failurePath,
    blocked_spans: blockedSpans.map(s => ({
      span_id: s.span_id,
      name: s.name,
      reason: s.blocked_replay_reason ?? s.error_summary,
    })),
    retry_groups: [],
  };
}

// Re-export tree utilities from utils.ts for backward compat
export {
  findDeepestFailure as findFirstFailure,
  findDeepestFailingDependency,
} from '../utils';
