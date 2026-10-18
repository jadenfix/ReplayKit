//! Forked replay: plan a fork, re-execute the affected spans in a hermetic
//! sandbox, and persist the result as a branch run.
//!
//! The pieces:
//!
//! - [`ReplayEngine::plan_fork`] computes the dirty subgraph and asks the
//!   *same* executor registry that will run the fork whether each dirty span
//!   can execute, so blocked spans (with reasons) are known up front.
//! - [`ReplayEngine::execute_fork`] restores the workspace recorded before
//!   the first dirty span into a private sandbox, walks the recorded
//!   timeline, re-executes dirty spans with their patched / re-bound inputs
//!   ([`inputs`]), re-applies the recorded workspace changes of everything
//!   it does not re-execute ([`sandbox`]), re-derives the status of
//!   container spans from their replayed children, and records the replayed
//!   workspace timeline in the branch run.

pub mod executors;
pub mod inputs;
pub mod sandbox;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use replaykit_core_model::{
    ArtifactId, ArtifactRecord, ArtifactType, BlockedSpanRecord, BranchId, BranchPlan,
    BranchRecord, BranchRequest, CostMetrics, DirtyReason, DirtySpanRecord, Document, EdgeKind,
    IdKind, PatchType, ReplayJobId, ReplayJobRecord, ReplayJobStatus, ReplayMode, RunId, RunRecord,
    RunStatus, RunSummary, SnapshotId, SnapshotRecord, SpanEdgeRecord, SpanId, SpanKind,
    SpanRecord, SpanStatus, Value, attrs,
};
use replaykit_storage::blob::sha256_hex;
use replaykit_storage::workspace::{
    WORKSPACE_SNAPSHOT_KIND, WorkspaceArchive, WorkspacePhase, WorkspacePoint,
    list_workspace_points, load_workspace_archive,
};
use replaykit_storage::{Storage, StorageError};

pub use sandbox::ReplayWorkspace;

/// Summary key holding patched output text when the store has no blob store.
const INLINE_CONTENT_KEY: &str = "inline_content";

use crate::inputs::{PATCHED_ATTR, PatchInputs, bindings_of, parse_env, render_template};
use crate::sandbox::WorkspaceTimeline;

#[derive(Debug)]
pub enum ReplayError {
    Storage(StorageError),
    InvalidPatch(String),
    Blocked(String),
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplayError::Storage(err) => write!(f, "{err}"),
            ReplayError::InvalidPatch(message) | ReplayError::Blocked(message) => {
                write!(f, "{message}")
            }
        }
    }
}

impl From<StorageError> for ReplayError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

/// A recorded output of the span being replayed, with its content when the
/// blob is available.
#[derive(Clone, Debug)]
pub struct RecordedOutput {
    pub artifact: ArtifactRecord,
    pub content: Option<Vec<u8>>,
}

/// The span exactly as it was recorded in the source run.
#[derive(Clone, Debug)]
pub struct RecordedSpan {
    pub span: SpanRecord,
    pub outputs: Vec<RecordedOutput>,
}

/// A child of a container span: as recorded, and as it ended in the replay.
#[derive(Clone, Debug)]
pub struct ChildOutcome {
    pub recorded: SpanRecord,
    pub replayed: SpanRecord,
}

/// Everything an executor needs besides the (effective) span itself.
#[derive(Clone, Debug)]
pub struct ReplayExecutionContext {
    pub source_run_id: RunId,
    pub target_run_id: RunId,
    pub fork_span_id: SpanId,
    /// The hermetic sandbox. `None` only when an executor is invoked outside
    /// a fork (paths then resolve against the process working directory and
    /// file writes are not performed).
    pub workspace: Option<ReplayWorkspace>,
    /// Extra environment for this span: its recorded `env` attribute plus
    /// any env override patched onto it or one of its ancestors.
    pub env: BTreeMap<String, String>,
    /// The span as recorded, with its recorded outputs.
    pub recorded: Option<RecordedSpan>,
    /// For container spans: their children's recorded and replayed state.
    pub children: Vec<ChildOutcome>,
    /// True while planning: executors must not run anything.
    pub planning: bool,
}

impl ReplayExecutionContext {
    pub fn new(source_run_id: RunId, target_run_id: RunId, fork_span_id: SpanId) -> Self {
        Self {
            source_run_id,
            target_run_id,
            fork_span_id,
            workspace: None,
            env: BTreeMap::new(),
            recorded: None,
            children: Vec::new(),
            planning: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExecutionResult {
    pub status: SpanStatus,
    pub output_artifacts: Vec<ProducedArtifact>,
    pub output_fingerprint: Option<String>,
    pub snapshot: Option<ProducedSnapshot>,
    pub error_summary: Option<String>,
    pub cost: CostMetrics,
}

#[derive(Clone, Debug)]
pub struct ProducedArtifact {
    pub artifact_type: ArtifactType,
    pub mime: String,
    pub sha256: String,
    pub byte_len: usize,
    pub blob_path: String,
    pub content: Option<Vec<u8>>,
    pub summary: Document,
    pub redaction: Document,
    pub created_at: u64,
}

#[derive(Clone, Debug)]
pub struct ProducedSnapshot {
    pub kind: String,
    pub artifact: ProducedArtifact,
    pub summary: Document,
    pub created_at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PatchDisposition {
    /// The patch replaces the span's output; nothing to execute.
    SatisfiesSpan,
    /// The span re-executes with patched inputs.
    RequiresExecution,
}

pub trait ExecutorRegistry: Send + Sync {
    fn supports(&self, span: &SpanRecord) -> bool;

    fn execute(
        &self,
        span: &SpanRecord,
        _context: &ReplayExecutionContext,
    ) -> Result<ExecutionResult, ReplayError>;

    /// Explain why a span cannot be executed. Returns `None` if supported.
    fn why_not(&self, span: &SpanRecord) -> Option<String> {
        if self.supports(span) {
            None
        } else {
            Some(format!("no executor for span kind {:?}", span.kind))
        }
    }

    /// Pre-flight check used by the planner (and again right before
    /// execution): `Err(reason)` if executing this effective span would
    /// block. Implementations must agree with `execute`.
    fn check(&self, span: &SpanRecord, _context: &ReplayExecutionContext) -> Result<(), String> {
        if self.supports(span) {
            Ok(())
        } else {
            Err(self
                .why_not(span)
                .unwrap_or_else(|| format!("no executor for span kind {:?}", span.kind)))
        }
    }
}

#[derive(Default)]
pub struct NoopExecutorRegistry;

impl ExecutorRegistry for NoopExecutorRegistry {
    fn supports(&self, _span: &SpanRecord) -> bool {
        false
    }

    fn execute(
        &self,
        _span: &SpanRecord,
        _context: &ReplayExecutionContext,
    ) -> Result<ExecutionResult, ReplayError> {
        Err(ReplayError::Blocked(
            "no executor registered for span".into(),
        ))
    }
}

#[derive(Clone, Debug)]
pub struct BranchExecution {
    pub branch: BranchRecord,
    pub replay_job: ReplayJobRecord,
    pub target_run: RunRecord,
    pub plan: BranchPlan,
}

/// Where sandboxes live and whether they are kept after a replay.
#[derive(Clone, Debug)]
pub struct ReplayOptions {
    pub sandbox_root: PathBuf,
    /// Keep the sandbox directory after the replay (its path is stored in
    /// the replay job's `progress.sandbox`). The final sandbox state is
    /// always recorded as the branch run's end-of-run workspace snapshot.
    pub keep_sandbox: bool,
}

impl Default for ReplayOptions {
    fn default() -> Self {
        Self {
            sandbox_root: std::env::temp_dir().join("replaykit-sandboxes"),
            keep_sandbox: false,
        }
    }
}

/// Span kinds whose status is derived from their children. They execute
/// after their children, through the registry (see
/// `executors::PlannerStepExecutor`).
pub fn is_container(kind: SpanKind) -> bool {
    matches!(
        kind,
        SpanKind::Run | SpanKind::PlannerStep | SpanKind::Subgraph
    )
}

pub struct ReplayEngine<S: Storage, E: ExecutorRegistry> {
    storage: Arc<S>,
    executors: E,
    options: ReplayOptions,
}

/// Everything the planner works out once, shared by `plan_fork` and
/// `execute_fork`.
struct PlanState {
    plan: BranchPlan,
    spans: Vec<SpanRecord>,
    span_map: BTreeMap<SpanId, SpanRecord>,
    dirty: BTreeMap<SpanId, DirtySpanRecord>,
    blocked: BTreeMap<SpanId, String>,
    rollup: BTreeSet<SpanId>,
    upstream: BTreeMap<SpanId, Vec<SpanId>>,
    disposition: PatchDisposition,
    patch: PatchInputs,
    /// Span whose subtree inherits `patch.env`.
    env_scope: Option<SpanId>,
    points: Vec<WorkspacePoint>,
    base_point: Option<usize>,
    recorded_root: Option<String>,
    override_archive: Option<WorkspaceArchive>,
}

impl<S: Storage, E: ExecutorRegistry> ReplayEngine<S, E> {
    pub fn new(storage: Arc<S>, executors: E) -> Self {
        Self {
            storage,
            executors,
            options: ReplayOptions::default(),
        }
    }

    pub fn with_options(mut self, options: ReplayOptions) -> Self {
        self.options = options;
        self
    }

    pub fn executors(&self) -> &E {
        &self.executors
    }

    pub fn plan_fork(&self, request: &BranchRequest) -> Result<BranchPlan, ReplayError> {
        Ok(self.plan_state(request)?.plan)
    }

    fn plan_state(&self, request: &BranchRequest) -> Result<PlanState, ReplayError> {
        let mut spans = self.storage.list_spans(&request.source_run_id)?;
        if spans.is_empty() {
            return Err(ReplayError::InvalidPatch(format!(
                "source run {:?} has no spans",
                request.source_run_id.0
            )));
        }
        spans.sort_by_key(|span| span.sequence_no);

        let span_map = spans
            .iter()
            .cloned()
            .map(|span| (span.span_id.clone(), span))
            .collect::<BTreeMap<_, _>>();
        let fork_span = span_map.get(&request.fork_span_id).ok_or_else(|| {
            ReplayError::InvalidPatch(format!(
                "fork span {:?} was not found in run {:?}",
                request.fork_span_id.0, request.source_run_id.0
            ))
        })?;
        validate_patch_target(fork_span, request)?;
        let disposition = patch_disposition(request.patch_manifest.patch_type, fork_span.kind);
        let patch = inputs::patch_inputs(
            request.patch_manifest.patch_type,
            &request.patch_manifest.replacement,
            fork_span,
        )
        .map_err(|reason| {
            ReplayError::InvalidPatch(format!(
                "invalid {:?} replacement: {reason}",
                request.patch_manifest.patch_type
            ))
        })?;

        let edges = self.storage.list_edges(&request.source_run_id)?;
        let dirty = compute_dirty_map(&request.fork_span_id, &spans, &edges);
        let mut dirty_spans = dirty.values().cloned().collect::<Vec<_>>();
        dirty_spans.sort_by_key(|record| {
            span_map
                .get(&record.span_id)
                .map(|span| span.sequence_no)
                .unwrap_or(u64::MAX)
        });

        // upstream[x] = spans x must wait for: data dependencies and a
        // non-container parent.
        let mut upstream = BTreeMap::<SpanId, Vec<SpanId>>::new();
        for edge in &edges {
            if edge.kind == EdgeKind::DataDependsOn {
                upstream
                    .entry(edge.from_span_id.clone())
                    .or_default()
                    .push(edge.to_span_id.clone());
            }
        }
        for span in &spans {
            if let Some(parent) = span.parent_span_id.as_ref().and_then(|p| span_map.get(p))
                && !is_container(parent.kind)
            {
                upstream
                    .entry(span.span_id.clone())
                    .or_default()
                    .push(parent.span_id.clone());
            }
        }

        // Workspace timeline.
        let points = list_workspace_points(&*self.storage, &request.source_run_id)?;
        let recorded_root = self
            .storage
            .get_run(&request.source_run_id)?
            .workspace_root
            .or_else(|| points.first().and_then(sandbox_root_label));
        let first_dirty = dirty_spans
            .first()
            .and_then(|record| span_map.get(&record.span_id));
        let base_point = first_dirty.and_then(|first| base_point_index(&points, &span_map, first));

        let mut warnings = Vec::new();
        if points.is_empty() {
            warnings.push(
                "the source run has no workspace snapshots (record it with a workspace_root): \
                 replayed spans run in an empty scratch sandbox"
                    .to_string(),
            );
        } else if base_point.is_none() {
            warnings.push(
                "no workspace snapshot precedes the first dirty span: replayed spans run in an \
                 empty scratch sandbox"
                    .to_string(),
            );
        } else if let Some(first) = first_dirty
            && points[base_point.unwrap_or_default()].span_id.as_ref() != Some(&first.span_id)
        {
            warnings.push(format!(
                "no workspace snapshot was recorded right before {}; the sandbox starts from the \
                 nearest earlier snapshot",
                first.span_id.0
            ));
        }

        let override_archive = if request.patch_manifest.patch_type == PatchType::SnapshotOverride {
            Some(self.snapshot_override_archive(request, &points)?)
        } else {
            None
        };

        let env_scope = (!patch.env.is_empty()).then(|| request.fork_span_id.clone());
        let parent_of = |id: &SpanId| span_map.get(id).and_then(|s| s.parent_span_id.clone());

        // Predict blocked spans with the real registry, in sequence order.
        let planning_workspace = ReplayWorkspace {
            sandbox: PathBuf::from("/replaykit-sandbox"),
            recorded_root: recorded_root.clone(),
        };
        let mut blocked = BTreeMap::<SpanId, String>::new();
        for record in &dirty_spans {
            let Some(span) = span_map.get(&record.span_id) else {
                continue;
            };
            if is_container(span.kind) {
                continue;
            }
            if span.span_id == request.fork_span_id
                && disposition == PatchDisposition::SatisfiesSpan
            {
                continue;
            }
            if let Some(up) = upstream.get(&span.span_id).and_then(|ups| {
                ups.iter()
                    .find(|up| dirty.contains_key(*up) && blocked.contains_key(*up))
            }) {
                blocked.insert(
                    span.span_id.clone(),
                    format!("upstream span {} did not complete: it is blocked", up.0),
                );
                continue;
            }
            let patch_for_span = (span.span_id == request.fork_span_id).then_some(&patch);
            let effective = match self.effective_span(span, &request.source_run_id, patch_for_span)
            {
                Ok(effective) => effective,
                Err(reason) => {
                    blocked.insert(span.span_id.clone(), reason);
                    continue;
                }
            };
            if let Ok(bindings) = bindings_of(span) {
                for template in bindings.values() {
                    for referenced in inputs::template_references(template).unwrap_or_default() {
                        if dirty.contains_key(&SpanId(referenced.clone()))
                            && span.kind == SpanKind::LlmCall
                        {
                            warnings.push(format!(
                                "{}: its prompt is re-rendered from the replayed output of {}; \
                                 a recorded-response model executor can only serve it if that \
                                 output is unchanged",
                                span.span_id.0, referenced
                            ));
                        }
                    }
                }
            }
            let mut context = ReplayExecutionContext::new(
                request.source_run_id.clone(),
                request.source_run_id.clone(),
                request.fork_span_id.clone(),
            );
            context.planning = true;
            context.workspace = Some(planning_workspace.clone());
            context.env = match span_env(span, env_scope.as_ref(), &patch, &parent_of) {
                Ok(env) => env,
                Err(reason) => {
                    blocked.insert(span.span_id.clone(), reason);
                    continue;
                }
            };
            context.recorded = Some(self.recorded_span(span)?);
            if let Err(reason) = self.executors.check(&effective, &context) {
                blocked.insert(span.span_id.clone(), reason);
            }
        }

        // Containers: dirty ones must be executable; ancestors of dirty spans
        // are re-derived ("rolled up") when the registry can.
        let mut rollup = BTreeSet::new();
        for record in &dirty_spans {
            let mut cursor = parent_of(&record.span_id);
            while let Some(ancestor) = cursor {
                if !dirty.contains_key(&ancestor)
                    && let Some(span) = span_map.get(&ancestor)
                    && is_container(span.kind)
                {
                    let context = ReplayExecutionContext::new(
                        request.source_run_id.clone(),
                        request.source_run_id.clone(),
                        request.fork_span_id.clone(),
                    );
                    if self.executors.check(span, &context).is_ok() {
                        rollup.insert(ancestor.clone());
                    }
                }
                cursor = parent_of(&ancestor);
            }
        }
        let mut containers = spans
            .iter()
            .filter(|span| is_container(span.kind))
            .filter(|span| dirty.contains_key(&span.span_id) || rollup.contains(&span.span_id))
            .collect::<Vec<_>>();
        containers.sort_by_key(|span| std::cmp::Reverse(span.sequence_no));
        for container in containers {
            if dirty.contains_key(&container.span_id) {
                let mut context = ReplayExecutionContext::new(
                    request.source_run_id.clone(),
                    request.source_run_id.clone(),
                    request.fork_span_id.clone(),
                );
                context.planning = true;
                if let Err(reason) = self.executors.check(container, &context) {
                    blocked.insert(container.span_id.clone(), reason);
                    continue;
                }
            }
            if let Some(child) = spans.iter().find(|span| {
                span.parent_span_id.as_ref() == Some(&container.span_id)
                    && blocked.contains_key(&span.span_id)
            }) {
                blocked.insert(
                    container.span_id.clone(),
                    format!("child span {} is blocked", child.span_id.0),
                );
            }
        }

        let dirty_ids = dirty.keys().cloned().collect::<BTreeSet<_>>();
        let reusable_spans = spans
            .iter()
            .filter(|span| !dirty_ids.contains(&span.span_id) && !rollup.contains(&span.span_id))
            .map(|span| span.span_id.clone())
            .collect::<Vec<_>>();
        let rollup_spans = spans
            .iter()
            .filter(|span| rollup.contains(&span.span_id))
            .map(|span| span.span_id.clone())
            .collect::<Vec<_>>();
        let blocked_ordered = spans
            .iter()
            .filter_map(|span| {
                blocked.get(&span.span_id).map(|reason| BlockedSpanRecord {
                    span_id: span.span_id.clone(),
                    reason: reason.clone(),
                })
            })
            .collect::<Vec<_>>();
        warnings.dedup();

        let plan = BranchPlan {
            source_run_id: request.source_run_id.clone(),
            fork_span_id: request.fork_span_id.clone(),
            dirty_spans,
            blocked_spans: blocked_ordered.iter().map(|b| b.span_id.clone()).collect(),
            reusable_spans,
            blocked: blocked_ordered,
            warnings,
            rollup_spans,
        };

        Ok(PlanState {
            plan,
            spans,
            span_map,
            dirty,
            blocked,
            rollup,
            upstream,
            disposition,
            patch,
            env_scope,
            points,
            base_point,
            recorded_root,
            override_archive,
        })
    }

    fn snapshot_override_archive(
        &self,
        request: &BranchRequest,
        points: &[WorkspacePoint],
    ) -> Result<WorkspaceArchive, ReplayError> {
        let reference = inputs::replacement_text(&request.patch_manifest.replacement)
            .map_err(ReplayError::InvalidPatch)?;
        let reference = reference.trim();
        if let Some(point) = points
            .iter()
            .find(|point| point.snapshot.snapshot_id.0 == reference)
        {
            return Ok(load_workspace_archive(&*self.storage, &point.snapshot)?);
        }
        let path = std::path::Path::new(reference);
        if path.is_dir() {
            return Ok(WorkspaceArchive::capture(path)?);
        }
        Err(ReplayError::InvalidPatch(format!(
            "snapshot_override replacement {reference:?} is neither a workspace snapshot id of \
             the source run nor a directory"
        )))
    }

    /// Recorded state of a source span with its output contents.
    fn recorded_span(&self, span: &SpanRecord) -> Result<RecordedSpan, ReplayError> {
        let mut outputs = Vec::new();
        for artifact_id in &span.output_artifact_ids {
            let artifact = self.storage.get_artifact(&span.run_id, artifact_id)?;
            let content = self
                .storage
                .read_artifact_content(&span.run_id, artifact_id)
                .ok();
            outputs.push(RecordedOutput { artifact, content });
        }
        Ok(RecordedSpan {
            span: span.clone(),
            outputs,
        })
    }

    /// Text of a span's primary (first) output in `run_id`.
    fn output_text(&self, run_id: &RunId, span_id: &str) -> Result<String, String> {
        let span = self
            .storage
            .get_span(run_id, &SpanId(span_id.to_owned()))
            .map_err(|_| format!("input binding references unknown span {span_id}"))?;
        let artifact_id = span
            .output_artifact_ids
            .first()
            .ok_or_else(|| format!("upstream span {span_id} has no output to bind"))?;
        match self.storage.read_artifact_content(run_id, artifact_id) {
            Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
            Err(err) => match self
                .storage
                .get_artifact(run_id, artifact_id)
                .ok()
                .and_then(|a| a.summary.get(INLINE_CONTENT_KEY).cloned())
            {
                Some(Value::Text(text)) => Ok(text),
                _ => Err(format!(
                    "output of span {span_id} has no stored content: {err}"
                )),
            },
        }
    }

    /// The span with its input bindings re-rendered from `lookup_run` and
    /// (for the fork span) the patch applied.
    fn effective_span(
        &self,
        span: &SpanRecord,
        lookup_run: &RunId,
        patch: Option<&PatchInputs>,
    ) -> Result<SpanRecord, String> {
        let mut effective = span.clone();
        let bindings = bindings_of(span)?;
        let mut lookup = |id: &str| self.output_text(lookup_run, id);
        for (attribute, template) in &bindings {
            if patch.is_some_and(|p| p.attributes.contains_key(attribute)) {
                continue;
            }
            let rendered = render_template(template, &mut lookup)
                .map_err(|e| format!("cannot render input {attribute}: {e}"))?;
            effective
                .attributes
                .insert(attribute.clone(), Value::Text(rendered));
        }
        if let Some(patch) = patch {
            for (attribute, value) in &patch.attributes {
                let value = match value {
                    Value::Text(text) if text.contains("{{") => Value::Text(
                        render_template(text, &mut lookup)
                            .map_err(|e| format!("cannot render patched {attribute}: {e}"))?,
                    ),
                    other => other.clone(),
                };
                effective.attributes.insert(attribute.clone(), value);
            }
        }
        Ok(effective)
    }

    pub fn execute_fork(&self, request: BranchRequest) -> Result<BranchExecution, ReplayError> {
        let state = self.plan_state(&request)?;
        let plan = state.plan.clone();
        let source_run = self.storage.get_run(&request.source_run_id)?;
        let source_artifacts = self.storage.list_artifacts(&request.source_run_id)?;
        let source_snapshots = self.storage.list_snapshots(&request.source_run_id)?;
        let source_edges = self.storage.list_edges(&request.source_run_id)?;
        let source_events = self.storage.list_events(&request.source_run_id)?;

        let target_run_id = RunId(self.storage.allocate_id(IdKind::Run)?);
        let branch_id = BranchId(self.storage.allocate_id(IdKind::Branch)?);
        let replay_job_id = ReplayJobId(self.storage.allocate_id(IdKind::ReplayJob)?);
        let now = request.patch_manifest.created_at;

        let mut target_run = source_run.clone();
        target_run.run_id = target_run_id.clone();
        target_run.source_run_id = Some(source_run.run_id.clone());
        target_run.branch_id = Some(branch_id.clone());
        target_run.started_at = now;
        target_run.ended_at = None;
        target_run.status = RunStatus::Running;
        if !target_run.labels.iter().any(|label| label == "branch") {
            target_run.labels.push("branch".into());
        }
        self.storage.insert_run(target_run.clone())?;

        let replay_job = ReplayJobRecord {
            replay_job_id: replay_job_id.clone(),
            source_run_id: request.source_run_id.clone(),
            target_run_id: Some(target_run_id.clone()),
            mode: ReplayMode::Forked,
            status: ReplayJobStatus::Running,
            created_at: now,
            started_at: Some(now),
            ended_at: None,
            progress: Document::new(),
            error_summary: None,
        };
        self.storage.insert_replay_job(replay_job.clone())?;

        // Workspace points from the base onwards are re-recorded by the
        // replay; earlier ones are shared history.
        let replay_from_ordinal = state
            .base_point
            .map(|index| state.points[index].ordinal)
            .unwrap_or(i64::MIN);
        let copied_snapshots = source_snapshots
            .iter()
            .filter(|snapshot| {
                snapshot.kind != WORKSPACE_SNAPSHOT_KIND
                    || state
                        .points
                        .iter()
                        .find(|p| p.snapshot.snapshot_id == snapshot.snapshot_id)
                        .is_some_and(|p| {
                            state.base_point.is_some() && p.ordinal < replay_from_ordinal
                        })
            })
            .cloned()
            .collect::<Vec<_>>();
        let copied_snapshot_ids = copied_snapshots
            .iter()
            .map(|snapshot| snapshot.snapshot_id.clone())
            .collect::<BTreeSet<_>>();
        let dropped_artifacts = source_snapshots
            .iter()
            .filter(|snapshot| !copied_snapshot_ids.contains(&snapshot.snapshot_id))
            .map(|snapshot| snapshot.artifact_id.clone())
            .collect::<BTreeSet<_>>();
        let copied_artifacts = source_artifacts
            .into_iter()
            .filter(|artifact| !dropped_artifacts.contains(&artifact.artifact_id))
            .collect::<Vec<_>>();
        copy_run_data(
            &*self.storage,
            &target_run_id,
            &state.spans,
            &copied_artifacts,
            &copied_snapshots,
            &source_edges,
            &source_events,
        )?;

        let patch_artifact = ArtifactRecord {
            artifact_id: ArtifactId(self.storage.allocate_id(IdKind::Artifact)?),
            run_id: target_run_id.clone(),
            span_id: Some(request.fork_span_id.clone()),
            artifact_type: ArtifactType::PatchManifest,
            mime: "application/replaykit-patch".into(),
            sha256: format!(
                "patch:{}:{}:{}",
                request.source_run_id.0, request.fork_span_id.0, request.patch_manifest.created_at
            ),
            byte_len: 1,
            blob_path: format!("memory://patch/{}", request.fork_span_id.0),
            summary: patch_summary(&request),
            redaction: Document::new(),
            created_at: now,
        };
        self.storage.insert_artifact(patch_artifact.clone())?;

        let branch = BranchRecord {
            branch_id: branch_id.clone(),
            source_run_id: request.source_run_id.clone(),
            target_run_id: target_run_id.clone(),
            fork_span_id: request.fork_span_id.clone(),
            patch_manifest_artifact_id: patch_artifact.artifact_id.clone(),
            created_at: now,
            created_by: request.created_by.clone(),
            status: RunStatus::Running,
        };
        self.storage.insert_branch(branch.clone())?;

        // Prepare the sandbox.
        let sandbox_dir = self.options.sandbox_root.join(format!(
            "{}-{}-{}",
            target_run_id.0,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let has_timeline = state.base_point.is_some();
        let mut timeline = WorkspaceTimeline::new(
            &*self.storage,
            target_run_id.clone(),
            if has_timeline {
                state.points.clone()
            } else {
                Vec::new()
            },
            sandbox_dir.clone(),
            now,
        );
        match state.base_point {
            Some(base) => timeline.start(base, state.override_archive.as_ref())?,
            None => timeline.start_detached(state.override_archive.as_ref())?,
        }
        let workspace = ReplayWorkspace {
            sandbox: sandbox_dir.clone(),
            recorded_root: state.recorded_root.clone(),
        };

        let result = self.replay_spans(
            &request,
            &state,
            &target_run_id,
            &patch_artifact,
            &workspace,
            &mut timeline,
            now,
        );
        let cleanup = || {
            if !self.options.keep_sandbox {
                let _ = std::fs::remove_dir_all(&sandbox_dir);
            }
        };
        let counts = match result {
            Ok(counts) => counts,
            Err(err) => {
                cleanup();
                return Err(err);
            }
        };
        cleanup();

        let final_spans = self.storage.list_spans(&target_run_id)?;
        target_run = self.storage.get_run(&target_run_id)?;
        target_run.ended_at = Some(now);
        target_run.status = derive_run_status(&final_spans);
        let final_artifacts = self.storage.list_artifacts(&target_run_id)?;
        target_run.summary =
            RunSummary::from_run_state(target_run.status, &final_spans, &final_artifacts, None);
        self.storage.update_run(target_run.clone())?;

        let mut branch = self.storage.get_branch(&branch_id)?;
        branch.status = target_run.status;
        self.storage.insert_branch(branch.clone())?;

        let mut replay_job = self.storage.get_replay_job(&replay_job_id)?;
        replay_job.status = match target_run.status {
            RunStatus::Blocked => ReplayJobStatus::Blocked,
            RunStatus::Failed => ReplayJobStatus::Failed,
            _ => ReplayJobStatus::Completed,
        };
        replay_job.ended_at = Some(now);
        replay_job
            .progress
            .insert("executed".into(), Value::Int(counts.executed as i64));
        replay_job
            .progress
            .insert("reused_unchanged".into(), Value::Int(counts.reused as i64));
        replay_job
            .progress
            .insert("blocked".into(), Value::Int(counts.blocked as i64));
        replay_job
            .progress
            .insert("hermetic".into(), Value::Bool(has_timeline));
        if self.options.keep_sandbox {
            replay_job.progress.insert(
                "sandbox".into(),
                Value::Text(sandbox_dir.to_string_lossy().into_owned()),
            );
        }
        self.storage.update_replay_job(replay_job.clone())?;

        Ok(BranchExecution {
            branch,
            replay_job,
            target_run,
            plan,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn replay_spans(
        &self,
        request: &BranchRequest,
        state: &PlanState,
        target_run_id: &RunId,
        patch_artifact: &ArtifactRecord,
        workspace: &ReplayWorkspace,
        timeline: &mut WorkspaceTimeline<'_, S>,
        now: u64,
    ) -> Result<ReplayCounts, ReplayError> {
        let parent_of = |id: &SpanId| {
            state
                .span_map
                .get(id)
                .and_then(|s| s.parent_span_id.clone())
        };
        let mut outcomes = BTreeMap::<SpanId, Outcome>::new();
        // Spans whose recorded workspace effects must not be re-applied.
        let mut skip = BTreeSet::<SpanId>::new();
        let mut counts = ReplayCounts::default();

        for source_span in &state.spans {
            if !state.dirty.contains_key(&source_span.span_id) || is_container(source_span.kind) {
                continue;
            }
            if let Some(index) = timeline.index_of(WorkspacePhase::Before, &source_span.span_id) {
                timeline.advance_to(index, &skip)?;
            }

            let mut target_span = self.storage.get_span(target_run_id, &source_span.span_id)?;
            let is_fork = source_span.span_id == request.fork_span_id;

            if is_fork && state.disposition == PatchDisposition::SatisfiesSpan {
                self.satisfy_with_patch(request, &mut target_span, patch_artifact, now)?;
                self.storage.upsert_span(target_span)?;
                outcomes.insert(source_span.span_id.clone(), Outcome::Satisfied);
                continue;
            }

            let runtime_blocked_upstream =
                state.upstream.get(&source_span.span_id).and_then(|ups| {
                    ups.iter()
                        .find(|up| matches!(outcomes.get(*up), Some(Outcome::Blocked)))
                        .cloned()
                });
            let block_reason = state
                .blocked
                .get(&source_span.span_id)
                .cloned()
                .or_else(|| {
                    runtime_blocked_upstream
                        .map(|up| format!("upstream span {} did not complete: it is blocked", up.0))
                });
            if let Some(reason) = block_reason {
                self.mark_blocked(&mut target_span, &reason, now)?;
                outcomes.insert(source_span.span_id.clone(), Outcome::Blocked);
                skip.insert(source_span.span_id.clone());
                counts.blocked += 1;
                continue;
            }

            let in_env_scope = state.env_scope.as_ref().is_some_and(|scope| {
                is_self_or_descendant(&source_span.span_id, scope, &parent_of)
            });
            if !is_fork
                && !in_env_scope
                && state.override_archive.is_none()
                && upstream_unchanged(
                    &state.dirty[&source_span.span_id],
                    &outcomes,
                    &state.span_map,
                    &*self.storage,
                    target_run_id,
                )?
            {
                // Early cutoff: every upstream it was dirty for replayed to
                // the recorded result, so the recorded output stands.
                target_span.attributes.insert(
                    "replay".into(),
                    Value::Text("reused: upstream unchanged".into()),
                );
                self.storage.upsert_span(target_span)?;
                outcomes.insert(source_span.span_id.clone(), Outcome::Reused);
                counts.reused += 1;
                continue;
            }

            let patch = is_fork.then_some(&state.patch);
            let effective = match self.effective_span(source_span, target_run_id, patch) {
                Ok(effective) => effective,
                Err(reason) => {
                    self.mark_blocked(&mut target_span, &reason, now)?;
                    outcomes.insert(source_span.span_id.clone(), Outcome::Blocked);
                    skip.insert(source_span.span_id.clone());
                    counts.blocked += 1;
                    continue;
                }
            };
            let mut context = ReplayExecutionContext::new(
                request.source_run_id.clone(),
                target_run_id.clone(),
                request.fork_span_id.clone(),
            );
            context.workspace = Some(workspace.clone());
            context.env = span_env(
                source_span,
                state.env_scope.as_ref(),
                &state.patch,
                &parent_of,
            )
            .map_err(ReplayError::Blocked)?;
            context.recorded = Some(self.recorded_span(source_span)?);

            reset_span_for_replay(&mut target_span);
            target_span.attributes = effective.attributes.clone();
            if is_fork {
                target_span.attributes.insert(
                    PATCHED_ATTR.into(),
                    Value::Text(format!("{:?}", request.patch_manifest.patch_type)),
                );
                if !state.patch.env.is_empty() {
                    target_span.attributes.insert(
                        attrs::ENV.into(),
                        Value::Object(
                            context
                                .env
                                .iter()
                                .map(|(k, v)| (k.clone(), Value::Text(v.clone())))
                                .collect(),
                        ),
                    );
                }
            }
            if effective.attributes != source_span.attributes || in_env_scope {
                target_span.input_fingerprint =
                    Some(inputs_fingerprint(&effective.attributes, &context.env));
            }
            skip.insert(source_span.span_id.clone());

            let checked = self.executors.check(&effective, &context);
            let executed = match checked {
                Err(reason) => Err(ReplayError::Blocked(reason)),
                Ok(()) => self.executors.execute(&effective, &context),
            };
            match executed {
                Ok(result) => {
                    let output_artifact_ids = self.persist_executor_artifacts(
                        target_run_id,
                        &target_span.span_id,
                        result.output_artifacts,
                    )?;
                    let snapshot_id = self.persist_executor_snapshot(
                        target_run_id,
                        &target_span.span_id,
                        result.snapshot,
                    )?;
                    target_span.status = result.status;
                    target_span.output_artifact_ids = output_artifact_ids;
                    target_span.output_fingerprint = result.output_fingerprint;
                    target_span.snapshot_id = snapshot_id;
                    target_span.error_summary = result.error_summary;
                    target_span.cost = result.cost;
                    target_span.ended_at = Some(now);
                    self.storage.upsert_span(target_span)?;
                    outcomes.insert(source_span.span_id.clone(), Outcome::Executed);
                    counts.executed += 1;
                }
                Err(ReplayError::Blocked(reason)) => {
                    self.mark_blocked(&mut target_span, &reason, now)?;
                    outcomes.insert(source_span.span_id.clone(), Outcome::Blocked);
                    counts.blocked += 1;
                }
                Err(other) => return Err(other),
            }
        }

        timeline.finish(&skip)?;

        // Containers last, innermost first.
        let mut containers = state
            .spans
            .iter()
            .filter(|span| is_container(span.kind))
            .filter(|span| {
                state.dirty.contains_key(&span.span_id) || state.rollup.contains(&span.span_id)
            })
            .collect::<Vec<_>>();
        containers.sort_by_key(|span| std::cmp::Reverse(span.sequence_no));
        for container in containers {
            let mut target_span = self.storage.get_span(target_run_id, &container.span_id)?;
            if let Some(reason) = state
                .blocked
                .get(&container.span_id)
                .filter(|_| state.dirty.contains_key(&container.span_id))
                .filter(|reason| !reason.starts_with("child span"))
            {
                self.mark_blocked(&mut target_span, reason, now)?;
                counts.blocked += 1;
                continue;
            }
            let mut context = ReplayExecutionContext::new(
                request.source_run_id.clone(),
                target_run_id.clone(),
                request.fork_span_id.clone(),
            );
            context.workspace = Some(workspace.clone());
            context.recorded = Some(self.recorded_span(container)?);
            for child in state
                .spans
                .iter()
                .filter(|span| span.parent_span_id.as_ref() == Some(&container.span_id))
            {
                context.children.push(ChildOutcome {
                    recorded: child.clone(),
                    replayed: self.storage.get_span(target_run_id, &child.span_id)?,
                });
            }
            match self.executors.execute(container, &context) {
                Ok(result) => {
                    target_span.status = result.status;
                    target_span.error_summary = result.error_summary;
                    target_span.ended_at = Some(now);
                    if container.span_id == request.fork_span_id {
                        target_span.attributes.insert(
                            PATCHED_ATTR.into(),
                            Value::Text(format!("{:?}", request.patch_manifest.patch_type)),
                        );
                    }
                    self.storage.upsert_span(target_span)?;
                }
                Err(ReplayError::Blocked(reason)) => {
                    if state.dirty.contains_key(&container.span_id) {
                        self.mark_blocked(&mut target_span, &reason, now)?;
                        counts.blocked += 1;
                    }
                }
                Err(other) => return Err(other),
            }
        }
        Ok(counts)
    }

    /// Output-replacing patches: the patched content becomes the span's
    /// output (stored with content so downstream bindings can read it).
    fn satisfy_with_patch(
        &self,
        request: &BranchRequest,
        span: &mut SpanRecord,
        patch_artifact: &ArtifactRecord,
        now: u64,
    ) -> Result<(), ReplayError> {
        let text = inputs::replacement_text(&request.patch_manifest.replacement)
            .map_err(ReplayError::InvalidPatch)?;
        let artifact_type = match span.kind {
            SpanKind::Retrieval => ArtifactType::RetrievalResult,
            SpanKind::FileRead => ArtifactType::FileBlob,
            _ => ArtifactType::ToolOutput,
        };
        let mut summary = Document::new();
        summary.insert(
            "patched_by".into(),
            Value::Text(patch_artifact.artifact_id.0.clone()),
        );
        summary.insert(
            "preview".into(),
            Value::Text(text.chars().take(200).collect()),
        );
        let record = ArtifactRecord {
            artifact_id: ArtifactId(self.storage.allocate_id(IdKind::Artifact)?),
            run_id: span.run_id.clone(),
            span_id: Some(span.span_id.clone()),
            artifact_type,
            mime: "text/plain".into(),
            sha256: String::new(),
            byte_len: text.len(),
            blob_path: String::new(),
            summary,
            redaction: Document::new(),
            created_at: now,
        };
        let stored = match self
            .storage
            .store_artifact_with_content(record.clone(), text.as_bytes())
        {
            Ok(stored) => stored,
            // Stores without a blob store keep the patched text inline.
            Err(StorageError::Internal(_)) => {
                let mut inline = record;
                inline.sha256 = sha256_hex(text.as_bytes());
                inline.blob_path = format!("inline://{}", inline.artifact_id.0);
                inline
                    .summary
                    .insert(INLINE_CONTENT_KEY.into(), Value::Text(text.clone()));
                self.storage.insert_artifact(inline.clone())?;
                inline
            }
            Err(other) => return Err(other.into()),
        };
        if let Some(target_artifact_id) = &request.patch_manifest.target_artifact_id {
            replace_artifact_id(
                &mut span.output_artifact_ids,
                target_artifact_id,
                stored.artifact_id.clone(),
            )?;
            // The patched output becomes the primary output.
            if let Some(pos) = span
                .output_artifact_ids
                .iter()
                .position(|id| *id == stored.artifact_id)
            {
                let id = span.output_artifact_ids.remove(pos);
                span.output_artifact_ids.insert(0, id);
            }
        } else {
            span.output_artifact_ids = vec![stored.artifact_id.clone()];
        }
        span.output_fingerprint = Some(sha256_hex(text.as_bytes()));
        span.status = SpanStatus::Completed;
        span.ended_at = Some(now);
        span.error_code = None;
        span.error_summary = None;
        span.attributes.insert(
            PATCHED_ATTR.into(),
            Value::Text(format!("{:?}", request.patch_manifest.patch_type)),
        );
        Ok(())
    }

    fn mark_blocked(
        &self,
        span: &mut SpanRecord,
        reason: &str,
        now: u64,
    ) -> Result<(), ReplayError> {
        reset_span_for_replay(span);
        span.status = SpanStatus::Blocked;
        span.ended_at = Some(now);
        span.error_summary = Some(format!("replay blocked: {reason}"));
        self.storage.upsert_span(span.clone())?;
        Ok(())
    }

    fn persist_executor_artifacts(
        &self,
        run_id: &RunId,
        span_id: &SpanId,
        artifacts: Vec<ProducedArtifact>,
    ) -> Result<Vec<ArtifactId>, ReplayError> {
        let mut artifact_ids = Vec::with_capacity(artifacts.len());
        for artifact in artifacts {
            let artifact_id = ArtifactId(self.storage.allocate_id(IdKind::Artifact)?);
            let record = ArtifactRecord {
                artifact_id: artifact_id.clone(),
                run_id: run_id.clone(),
                span_id: Some(span_id.clone()),
                artifact_type: artifact.artifact_type,
                mime: artifact.mime,
                sha256: artifact.sha256,
                byte_len: artifact.byte_len,
                blob_path: artifact.blob_path,
                summary: artifact.summary,
                redaction: artifact.redaction,
                created_at: artifact.created_at,
            };
            if let Some(content) = artifact.content {
                self.storage.store_artifact_with_content(record, &content)?;
            } else {
                self.storage.insert_artifact(record)?;
            }
            artifact_ids.push(artifact_id);
        }
        Ok(artifact_ids)
    }

    fn persist_executor_snapshot(
        &self,
        run_id: &RunId,
        span_id: &SpanId,
        snapshot: Option<ProducedSnapshot>,
    ) -> Result<Option<SnapshotId>, ReplayError> {
        let Some(snapshot) = snapshot else {
            return Ok(None);
        };

        let artifact_id = ArtifactId(self.storage.allocate_id(IdKind::Artifact)?);
        let record = ArtifactRecord {
            artifact_id: artifact_id.clone(),
            run_id: run_id.clone(),
            span_id: Some(span_id.clone()),
            artifact_type: snapshot.artifact.artifact_type,
            mime: snapshot.artifact.mime,
            sha256: snapshot.artifact.sha256,
            byte_len: snapshot.artifact.byte_len,
            blob_path: snapshot.artifact.blob_path,
            summary: snapshot.artifact.summary,
            redaction: snapshot.artifact.redaction,
            created_at: snapshot.artifact.created_at,
        };
        if let Some(content) = snapshot.artifact.content {
            self.storage.store_artifact_with_content(record, &content)?;
        } else {
            self.storage.insert_artifact(record)?;
        }

        let snapshot_id = SnapshotId(self.storage.allocate_id(IdKind::Snapshot)?);
        self.storage.insert_snapshot(SnapshotRecord {
            snapshot_id: snapshot_id.clone(),
            run_id: run_id.clone(),
            span_id: Some(span_id.clone()),
            kind: snapshot.kind,
            artifact_id,
            summary: snapshot.summary,
            created_at: snapshot.created_at,
        })?;
        Ok(Some(snapshot_id))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Executed,
    Satisfied,
    Reused,
    Blocked,
}

#[derive(Default)]
struct ReplayCounts {
    executed: usize,
    reused: usize,
    blocked: usize,
}

fn sandbox_root_label(point: &WorkspacePoint) -> Option<String> {
    replaykit_storage::workspace::workspace_root_of(point).filter(|root| !root.is_empty())
}

/// Index of the workspace point the sandbox starts from: the snapshot taken
/// right before `first` started, or the nearest earlier "before" snapshot.
fn base_point_index(
    points: &[WorkspacePoint],
    span_map: &BTreeMap<SpanId, SpanRecord>,
    first: &SpanRecord,
) -> Option<usize> {
    if let Some(index) = points.iter().position(|point| {
        point.phase == WorkspacePhase::Before && point.span_id.as_ref() == Some(&first.span_id)
    }) {
        return Some(index);
    }
    points.iter().rposition(|point| {
        point.phase == WorkspacePhase::Before
            && point
                .span_id
                .as_ref()
                .and_then(|id| span_map.get(id))
                .is_some_and(|span| span.sequence_no <= first.sequence_no)
    })
}

fn is_self_or_descendant(
    span_id: &SpanId,
    ancestor: &SpanId,
    parent_of: &dyn Fn(&SpanId) -> Option<SpanId>,
) -> bool {
    let mut cursor = Some(span_id.clone());
    while let Some(current) = cursor {
        if &current == ancestor {
            return true;
        }
        cursor = parent_of(&current);
    }
    false
}

/// Recorded `env` attribute merged with any env override in scope.
fn span_env(
    span: &SpanRecord,
    scope: Option<&SpanId>,
    patch: &PatchInputs,
    parent_of: &dyn Fn(&SpanId) -> Option<SpanId>,
) -> Result<BTreeMap<String, String>, String> {
    let mut env = match span.attributes.get(attrs::ENV) {
        Some(value) => parse_env(value).map_err(|e| format!("bad recorded env: {e}"))?,
        None => BTreeMap::new(),
    };
    if let Some(scope) = scope
        && is_self_or_descendant(&span.span_id, scope, parent_of)
    {
        env.extend(patch.env.clone());
    }
    Ok(env)
}

fn inputs_fingerprint(attributes: &Document, env: &BTreeMap<String, String>) -> String {
    let mut canonical = String::new();
    for (key, value) in attributes {
        if key == PATCHED_ATTR || key == "replay" {
            continue;
        }
        canonical.push_str(key);
        canonical.push('=');
        canonical.push_str(&format!("{value:?}"));
        canonical.push('\n');
    }
    for (key, value) in env {
        canonical.push_str("env:");
        canonical.push_str(key);
        canonical.push('=');
        canonical.push_str(value);
        canonical.push('\n');
    }
    format!("inputs:{}", sha256_hex(canonical.as_bytes()))
}

/// True if every span that made `record` dirty replayed to its recorded
/// status and output.
fn upstream_unchanged<S: Storage>(
    record: &DirtySpanRecord,
    outcomes: &BTreeMap<SpanId, Outcome>,
    span_map: &BTreeMap<SpanId, SpanRecord>,
    storage: &S,
    target_run_id: &RunId,
) -> Result<bool, ReplayError> {
    if record.triggered_by.is_empty() || record.reasons.contains(&DirtyReason::PatchedInput) {
        return Ok(false);
    }
    for trigger in &record.triggered_by {
        let Some(source) = span_map.get(trigger) else {
            return Ok(false);
        };
        if is_container(source.kind) {
            return Ok(false);
        }
        match outcomes.get(trigger) {
            Some(Outcome::Reused) => {}
            Some(Outcome::Executed) | Some(Outcome::Satisfied) => {
                let replayed = storage.get_span(target_run_id, trigger)?;
                if replayed.status != source.status
                    || replayed.output_fingerprint.is_none()
                    || replayed.output_fingerprint != source.output_fingerprint
                {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }
    }
    Ok(true)
}

/// Run status after a replay: blocked if anything blocked; otherwise the
/// status of the root `Run` span when there is one (its status is re-derived
/// from its children); otherwise failed if any span failed.
fn derive_run_status(spans: &[SpanRecord]) -> RunStatus {
    if spans.iter().any(|span| span.status == SpanStatus::Blocked) {
        return RunStatus::Blocked;
    }
    if let Some(root) = spans
        .iter()
        .find(|span| span.parent_span_id.is_none() && span.kind == SpanKind::Run)
    {
        return match root.status {
            SpanStatus::Completed => RunStatus::Completed,
            SpanStatus::Canceled => RunStatus::Canceled,
            _ => RunStatus::Failed,
        };
    }
    if spans.iter().any(|span| span.status == SpanStatus::Failed) {
        RunStatus::Failed
    } else {
        RunStatus::Completed
    }
}

fn validate_patch_target(span: &SpanRecord, request: &BranchRequest) -> Result<(), ReplayError> {
    let supported = patch_supported(request.patch_manifest.patch_type, span.kind);
    if supported {
        Ok(())
    } else {
        Err(ReplayError::InvalidPatch(format!(
            "patch {:?} is not valid for span kind {:?}",
            request.patch_manifest.patch_type, span.kind
        )))
    }
}

/// Which patch types apply to which span kinds.
pub fn patch_supported(patch_type: PatchType, kind: SpanKind) -> bool {
    matches!(
        (patch_type, kind),
        (PatchType::PromptEdit, SpanKind::LlmCall)
            | (PatchType::ModelConfigEdit, SpanKind::LlmCall)
            | (PatchType::ToolOutputOverride, SpanKind::ToolCall)
            | (PatchType::RetrievalContextOverride, SpanKind::Retrieval)
            | (
                PatchType::CommandOverride,
                SpanKind::ShellCommand | SpanKind::ToolCall
            )
            | (
                PatchType::FileContentOverride,
                SpanKind::FileWrite | SpanKind::FileRead
            )
            | (PatchType::EnvVarOverride, _)
            | (PatchType::SnapshotOverride, _)
    )
}

fn patch_disposition(patch_type: PatchType, kind: SpanKind) -> PatchDisposition {
    match (patch_type, kind) {
        (PatchType::ToolOutputOverride, _)
        | (PatchType::RetrievalContextOverride, _)
        | (PatchType::FileContentOverride, SpanKind::FileRead) => PatchDisposition::SatisfiesSpan,
        _ => PatchDisposition::RequiresExecution,
    }
}

fn reset_span_for_replay(span: &mut SpanRecord) {
    span.status = SpanStatus::Running;
    span.ended_at = None;
    span.output_artifact_ids.clear();
    span.snapshot_id = None;
    span.output_fingerprint = None;
    span.error_code = None;
    span.error_summary = None;
    span.cost = CostMetrics::default();
}

fn replace_artifact_id(
    artifact_ids: &mut [ArtifactId],
    old_id: &ArtifactId,
    new_id: ArtifactId,
) -> Result<(), ReplayError> {
    let slot = artifact_ids
        .iter_mut()
        .find(|artifact_id| **artifact_id == *old_id)
        .ok_or_else(|| {
            ReplayError::InvalidPatch(format!(
                "artifact {:?} was not attached to the patched span",
                old_id.0
            ))
        })?;
    *slot = new_id;
    Ok(())
}

fn patch_summary(request: &BranchRequest) -> Document {
    let mut summary = Document::new();
    summary.insert(
        "patch_type".into(),
        Value::Text(format!("{:?}", request.patch_manifest.patch_type)),
    );
    summary.insert(
        "fork_span_id".into(),
        Value::Text(request.fork_span_id.0.clone()),
    );
    summary.insert(
        "replacement".into(),
        request.patch_manifest.replacement.clone(),
    );
    if let Some(note) = &request.patch_manifest.note {
        summary.insert("note".into(), Value::Text(note.clone()));
    }
    summary
}

fn compute_dirty_map(
    fork_span_id: &SpanId,
    spans: &[SpanRecord],
    edges: &[SpanEdgeRecord],
) -> BTreeMap<SpanId, DirtySpanRecord> {
    let mut dirty = BTreeMap::new();
    let children = spans.iter().fold(BTreeMap::new(), |mut acc, span| {
        if let Some(parent) = &span.parent_span_id {
            acc.entry(parent.clone())
                .or_insert_with(Vec::new)
                .push(span.span_id.clone());
        }
        acc
    });

    // Map from a span to all spans that depend on it.
    // Edge semantics: from_span_id DataDependsOn to_span_id means "from depends on to".
    // When to_span_id is dirty, from_span_id is a downstream dependent that must re-run.
    let data_dependents = edges.iter().fold(BTreeMap::new(), |mut acc, edge| {
        if edge.kind == EdgeKind::DataDependsOn {
            acc.entry(edge.to_span_id.clone())
                .or_insert_with(Vec::new)
                .push(edge.from_span_id.clone());
        }
        acc
    });

    let mut queue = VecDeque::new();
    dirty.insert(
        fork_span_id.clone(),
        DirtySpanRecord {
            span_id: fork_span_id.clone(),
            reasons: vec![DirtyReason::PatchedInput],
            triggered_by: Vec::new(),
        },
    );
    queue.push_back(fork_span_id.clone());

    while let Some(current) = queue.pop_front() {
        let mut data_dependents_for_current = BTreeSet::new();
        if let Some(dependents) = data_dependents.get(&current) {
            for dependent in dependents {
                data_dependents_for_current.insert(dependent.clone());
                push_dirty(
                    &mut dirty,
                    &mut queue,
                    dependent.clone(),
                    DirtyReason::UpstreamOutputChanged,
                    current.clone(),
                );
            }
        }

        if let Some(descendants) = children.get(&current) {
            for child in descendants {
                if data_dependents_for_current.contains(child) {
                    continue;
                }
                push_dirty(
                    &mut dirty,
                    &mut queue,
                    child.clone(),
                    DirtyReason::DependencyUnknown,
                    current.clone(),
                );
            }
        }
    }

    dirty
}

fn push_dirty(
    dirty: &mut BTreeMap<SpanId, DirtySpanRecord>,
    queue: &mut VecDeque<SpanId>,
    span_id: SpanId,
    reason: DirtyReason,
    triggered_by: SpanId,
) {
    if let Some(existing) = dirty.get_mut(&span_id) {
        if !existing.reasons.contains(&reason) {
            existing.reasons.push(reason);
        }
        if !existing.triggered_by.contains(&triggered_by) {
            existing.triggered_by.push(triggered_by);
        }
        return;
    }

    dirty.insert(
        span_id.clone(),
        DirtySpanRecord {
            span_id: span_id.clone(),
            reasons: vec![reason],
            triggered_by: vec![triggered_by],
        },
    );
    queue.push_back(span_id);
}

fn copy_run_data<S: Storage>(
    storage: &S,
    target_run_id: &RunId,
    source_spans: &[SpanRecord],
    source_artifacts: &[ArtifactRecord],
    source_snapshots: &[SnapshotRecord],
    source_edges: &[SpanEdgeRecord],
    source_events: &[replaykit_core_model::EventRecord],
) -> Result<(), ReplayError> {
    for span in source_spans {
        let mut copied = span.clone();
        copied.run_id = target_run_id.clone();
        storage.upsert_span(copied)?;
    }

    for artifact in source_artifacts {
        let mut copied = artifact.clone();
        copied.run_id = target_run_id.clone();
        storage.insert_artifact(copied)?;
    }

    for snapshot in source_snapshots {
        let mut copied = snapshot.clone();
        copied.run_id = target_run_id.clone();
        storage.insert_snapshot(copied)?;
    }

    for edge in source_edges {
        let mut copied = edge.clone();
        copied.run_id = target_run_id.clone();
        storage.insert_edge(copied)?;
    }

    for event in source_events {
        let mut copied = event.clone();
        copied.run_id = target_run_id.clone();
        storage.insert_event(copied)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use replaykit_core_model::{
        ArtifactRecord, ArtifactType, BranchRequest, CostMetrics, Document, EdgeId, EdgeKind,
        PatchManifest, PatchType, ReplayPolicy, RunRecord, RunStatus, SpanEdgeRecord, SpanKind,
        SpanRecord, SpanStatus, TraceId, Value,
    };
    use replaykit_storage::{InMemoryStorage, Storage};

    use super::*;
    use crate::executors::CompositeExecutorRegistry;

    struct FakeExecutorRegistry;

    impl ExecutorRegistry for FakeExecutorRegistry {
        fn supports(&self, span: &SpanRecord) -> bool {
            matches!(span.kind, SpanKind::ToolCall | SpanKind::LlmCall)
        }

        fn execute(
            &self,
            span: &SpanRecord,
            _context: &ReplayExecutionContext,
        ) -> Result<ExecutionResult, ReplayError> {
            let artifact = match span.kind {
                SpanKind::ToolCall => ProducedArtifact {
                    artifact_type: ArtifactType::ToolOutput,
                    mime: "application/json".into(),
                    sha256: format!("tool-output:{}", span.span_id.0),
                    byte_len: 1,
                    blob_path: format!("memory://tool/{}", span.span_id.0),
                    content: None,
                    summary: summary_from_pairs(&[("tool", "patched tool output")]),
                    redaction: Document::new(),
                    created_at: 25,
                },
                SpanKind::LlmCall => ProducedArtifact {
                    artifact_type: ArtifactType::ModelResponse,
                    mime: "application/json".into(),
                    sha256: format!("model-output:{}", span.span_id.0),
                    byte_len: 1,
                    blob_path: format!("memory://llm/{}", span.span_id.0),
                    content: None,
                    summary: summary_from_pairs(&[("answer", "patched final answer")]),
                    redaction: Document::new(),
                    created_at: 26,
                },
                _ => unreachable!("unsupported span kind for fake executor"),
            };

            let snapshot = matches!(span.kind, SpanKind::LlmCall).then(|| ProducedSnapshot {
                kind: "state".into(),
                artifact: ProducedArtifact {
                    artifact_type: ArtifactType::StateSnapshot,
                    mime: "application/json".into(),
                    sha256: format!("snapshot:{}", span.span_id.0),
                    byte_len: 1,
                    blob_path: format!("memory://snapshot/{}", span.span_id.0),
                    content: None,
                    summary: summary_from_pairs(&[("state", "post-replay")]),
                    redaction: Document::new(),
                    created_at: 27,
                },
                summary: summary_from_pairs(&[("state", "post-replay")]),
                created_at: 27,
            });

            Ok(ExecutionResult {
                status: SpanStatus::Completed,
                output_artifacts: vec![artifact],
                output_fingerprint: Some(format!("replayed:{}", span.span_id.0)),
                snapshot,
                error_summary: None,
                cost: CostMetrics {
                    input_tokens: 3,
                    output_tokens: 5,
                    estimated_cost_micros: 11,
                },
            })
        }
    }

    fn insert_run_fixture(storage: &InMemoryStorage) -> RunRecord {
        let run = RunRecord::new(
            RunId("run-src".into()),
            TraceId("trace-src".into()),
            "source",
            "agent.main",
            "adapter",
            "0.1.0",
            1,
        );
        storage.insert_run(run.clone()).unwrap();

        let planner = SpanRecord {
            run_id: run.run_id.clone(),
            span_id: SpanId("planner".into()),
            trace_id: run.trace_id.clone(),
            parent_span_id: None,
            sequence_no: 1,
            kind: SpanKind::PlannerStep,
            name: "planner".into(),
            status: SpanStatus::Completed,
            started_at: 1,
            ended_at: Some(2),
            replay_policy: ReplayPolicy::RecordOnly,
            executor_kind: None,
            executor_version: None,
            input_artifact_ids: Vec::new(),
            output_artifact_ids: Vec::new(),
            snapshot_id: None,
            input_fingerprint: None,
            output_fingerprint: Some("planner-out".into()),
            environment_fingerprint: None,
            attributes: BTreeMap::new(),
            error_code: None,
            error_summary: None,
            cost: CostMetrics::default(),
        };
        let tool = SpanRecord {
            span_id: SpanId("tool".into()),
            sequence_no: 2,
            kind: SpanKind::ToolCall,
            name: "tool".into(),
            replay_policy: ReplayPolicy::RerunnableSupported,
            output_artifact_ids: vec![ArtifactId("tool-output".into())],
            output_fingerprint: Some("tool-out".into()),
            ..planner.clone()
        };
        let final_answer = SpanRecord {
            span_id: SpanId("answer".into()),
            sequence_no: 3,
            kind: SpanKind::LlmCall,
            name: "answer".into(),
            replay_policy: ReplayPolicy::RerunnableSupported,
            parent_span_id: Some(SpanId("planner".into())),
            output_artifact_ids: vec![ArtifactId("answer-output".into())],
            output_fingerprint: Some("answer-out".into()),
            status: SpanStatus::Failed,
            error_summary: Some("failed".into()),
            ..planner.clone()
        };

        storage.upsert_span(planner).unwrap();
        storage.upsert_span(tool).unwrap();
        storage.upsert_span(final_answer).unwrap();
        storage
            .insert_artifact(ArtifactRecord {
                artifact_id: ArtifactId("tool-output".into()),
                run_id: run.run_id.clone(),
                span_id: Some(SpanId("tool".into())),
                artifact_type: ArtifactType::ToolOutput,
                mime: "application/json".into(),
                sha256: "tool-output".into(),
                byte_len: 1,
                blob_path: "memory://tool-output".into(),
                summary: summary_from_pairs(&[("tool", "initial tool output")]),
                redaction: Document::new(),
                created_at: 2,
            })
            .unwrap();
        storage
            .insert_artifact(ArtifactRecord {
                artifact_id: ArtifactId("answer-output".into()),
                run_id: run.run_id.clone(),
                span_id: Some(SpanId("answer".into())),
                artifact_type: ArtifactType::ModelResponse,
                mime: "application/json".into(),
                sha256: "answer-output".into(),
                byte_len: 1,
                blob_path: "memory://answer-output".into(),
                summary: summary_from_pairs(&[("answer", "initial answer")]),
                redaction: Document::new(),
                created_at: 3,
            })
            .unwrap();
        // "answer depends on tool" — answer uses tool's output
        storage
            .insert_edge(SpanEdgeRecord {
                edge_id: EdgeId("edge-1".into()),
                run_id: run.run_id.clone(),
                from_span_id: SpanId("answer".into()),
                to_span_id: SpanId("tool".into()),
                kind: EdgeKind::DataDependsOn,
                attributes: Document::new(),
            })
            .unwrap();
        run
    }

    #[test]
    fn plans_dirty_subgraph_from_data_dependencies() {
        let storage = Arc::new(InMemoryStorage::new());
        let run = insert_run_fixture(&storage);
        let engine = ReplayEngine::new(storage, NoopExecutorRegistry);

        let request = BranchRequest {
            source_run_id: run.run_id,
            fork_span_id: SpanId("tool".into()),
            patch_manifest: PatchManifest {
                patch_type: PatchType::ToolOutputOverride,
                target_artifact_id: Some(ArtifactId("tool-output".into())),
                replacement: Value::Text("patched".into()),
                note: None,
                created_at: 10,
            },
            created_by: None,
        };

        let plan = engine.plan_fork(&request).unwrap();
        let dirty_ids = plan
            .dirty_spans
            .into_iter()
            .map(|record| record.span_id.0)
            .collect::<Vec<_>>();
        assert!(dirty_ids.contains(&"tool".into()));
        assert!(dirty_ids.contains(&"answer".into()));
        assert!(!dirty_ids.contains(&"planner".into()));
    }

    #[test]
    fn plans_dirty_subgraph_for_unknown_children_even_when_data_edges_exist() {
        let storage = Arc::new(InMemoryStorage::new());
        let run = insert_run_fixture(&storage);
        storage
            .upsert_span(SpanRecord {
                run_id: run.run_id.clone(),
                span_id: SpanId("audit".into()),
                trace_id: run.trace_id.clone(),
                parent_span_id: Some(SpanId("tool".into())),
                sequence_no: 4,
                kind: SpanKind::GuardrailCheck,
                name: "audit".into(),
                status: SpanStatus::Completed,
                started_at: 4,
                ended_at: Some(5),
                replay_policy: ReplayPolicy::RecordOnly,
                executor_kind: None,
                executor_version: None,
                input_artifact_ids: Vec::new(),
                output_artifact_ids: Vec::new(),
                snapshot_id: None,
                input_fingerprint: None,
                output_fingerprint: Some("audit-out".into()),
                environment_fingerprint: None,
                attributes: BTreeMap::new(),
                error_code: None,
                error_summary: None,
                cost: CostMetrics::default(),
            })
            .unwrap();
        let engine = ReplayEngine::new(storage, NoopExecutorRegistry);

        let plan = engine
            .plan_fork(&BranchRequest {
                source_run_id: run.run_id,
                fork_span_id: SpanId("tool".into()),
                patch_manifest: PatchManifest {
                    patch_type: PatchType::ToolOutputOverride,
                    target_artifact_id: Some(ArtifactId("tool-output".into())),
                    replacement: Value::Text("patched".into()),
                    note: None,
                    created_at: 10,
                },
                created_by: None,
            })
            .unwrap();

        let dirty_ids = plan
            .dirty_spans
            .into_iter()
            .map(|record| record.span_id.0)
            .collect::<Vec<_>>();
        assert!(dirty_ids.contains(&"tool".into()));
        assert!(dirty_ids.contains(&"answer".into()));
        assert!(dirty_ids.contains(&"audit".into()));
    }

    #[test]
    fn prompt_edit_blocks_without_executor_and_clears_stale_output() {
        let storage = Arc::new(InMemoryStorage::new());
        let run = insert_run_fixture(&storage);
        let engine = ReplayEngine::new(storage.clone(), NoopExecutorRegistry);

        let execution = engine
            .execute_fork(BranchRequest {
                source_run_id: run.run_id,
                fork_span_id: SpanId("answer".into()),
                patch_manifest: PatchManifest {
                    patch_type: PatchType::PromptEdit,
                    target_artifact_id: None,
                    replacement: Value::Text("reword the answer".into()),
                    note: None,
                    created_at: 10,
                },
                created_by: None,
            })
            .unwrap();

        assert_eq!(execution.target_run.status, RunStatus::Blocked);
        assert_eq!(execution.target_run.summary.error_count, 1);
        assert_eq!(
            execution.target_run.summary.failure_class,
            Some(replaykit_core_model::FailureClass::ReplayUnsupported)
        );
        assert_eq!(execution.target_run.summary.token_count, 0);
        assert_eq!(execution.target_run.summary.artifact_count, 3);
        assert_eq!(execution.target_run.summary.final_output_preview, None);
        let answer = storage
            .get_span(&execution.target_run.run_id, &SpanId("answer".into()))
            .unwrap();
        assert_eq!(answer.status, SpanStatus::Blocked);
        assert!(answer.output_artifact_ids.is_empty());
        let err = answer.error_summary.as_deref().expect("should have error");
        assert!(
            err.starts_with("replay blocked:"),
            "unexpected error: {err}"
        );
        assert!(err.contains("LlmCall"), "should mention span kind: {err}");
    }

    #[test]
    fn prompt_edit_reruns_and_persists_executor_outputs() {
        let storage = Arc::new(InMemoryStorage::new());
        let run = insert_run_fixture(&storage);
        let engine = ReplayEngine::new(storage.clone(), FakeExecutorRegistry);

        let execution = engine
            .execute_fork(BranchRequest {
                source_run_id: run.run_id,
                fork_span_id: SpanId("answer".into()),
                patch_manifest: PatchManifest {
                    patch_type: PatchType::PromptEdit,
                    target_artifact_id: None,
                    replacement: Value::Text("produce a fixed answer".into()),
                    note: None,
                    created_at: 10,
                },
                created_by: None,
            })
            .unwrap();

        assert_eq!(execution.target_run.status, RunStatus::Completed);
        assert_eq!(execution.target_run.summary.error_count, 0);
        assert_eq!(execution.target_run.summary.failure_class, None);
        assert_eq!(execution.target_run.summary.token_count, 8);
        assert_eq!(execution.target_run.summary.estimated_cost_micros, 11);
        assert_eq!(execution.target_run.summary.artifact_count, 5);
        assert_eq!(execution.target_run.summary.final_output_preview, None);
        let answer = storage
            .get_span(&execution.target_run.run_id, &SpanId("answer".into()))
            .unwrap();
        assert_eq!(answer.status, SpanStatus::Completed);
        assert_eq!(
            answer.output_fingerprint.as_deref(),
            Some("replayed:answer")
        );
        assert_eq!(answer.output_artifact_ids.len(), 1);
        assert_ne!(answer.output_artifact_ids[0].0, "answer-output");
        assert!(answer.snapshot_id.is_some());

        let output_artifact = storage
            .get_artifact(&execution.target_run.run_id, &answer.output_artifact_ids[0])
            .unwrap();
        assert_eq!(output_artifact.artifact_type, ArtifactType::ModelResponse);
        assert_eq!(output_artifact.span_id, Some(SpanId("answer".into())));

        let snapshot = storage
            .get_snapshot(
                &execution.target_run.run_id,
                &answer.snapshot_id.clone().unwrap(),
            )
            .unwrap();
        assert_eq!(snapshot.span_id, Some(SpanId("answer".into())));
        let snapshot_artifact = storage
            .get_artifact(&execution.target_run.run_id, &snapshot.artifact_id)
            .unwrap();
        assert_eq!(snapshot_artifact.artifact_type, ArtifactType::StateSnapshot);
    }

    #[test]
    fn shell_replay_persists_artifact_content() {
        let storage = Arc::new(InMemoryStorage::new());
        let run = RunRecord::new(
            RunId("run-shell".into()),
            TraceId("trace-shell".into()),
            "shell",
            "agent.main",
            "adapter",
            "0.1.0",
            1,
        );
        storage.insert_run(run.clone()).unwrap();
        storage
            .upsert_span(SpanRecord {
                run_id: run.run_id.clone(),
                span_id: SpanId("shell".into()),
                trace_id: run.trace_id.clone(),
                parent_span_id: None,
                sequence_no: 1,
                kind: SpanKind::ShellCommand,
                name: "printf hello".into(),
                status: SpanStatus::Completed,
                started_at: 1,
                ended_at: Some(2),
                replay_policy: ReplayPolicy::RerunnableSupported,
                executor_kind: Some("shell".into()),
                executor_version: Some("sh".into()),
                input_artifact_ids: Vec::new(),
                output_artifact_ids: Vec::new(),
                snapshot_id: None,
                input_fingerprint: None,
                output_fingerprint: Some("shell-out".into()),
                environment_fingerprint: None,
                attributes: Document::new(),
                error_code: None,
                error_summary: None,
                cost: CostMetrics::default(),
            })
            .unwrap();

        let engine = ReplayEngine::new(storage.clone(), CompositeExecutorRegistry::new());
        let execution = engine
            .execute_fork(BranchRequest {
                source_run_id: run.run_id.clone(),
                fork_span_id: SpanId("shell".into()),
                patch_manifest: PatchManifest {
                    patch_type: PatchType::EnvVarOverride,
                    target_artifact_id: None,
                    replacement: Value::Text("PATH=/usr/bin".into()),
                    note: None,
                    created_at: 10,
                },
                created_by: None,
            })
            .unwrap();

        let shell = storage
            .get_span(&execution.target_run.run_id, &SpanId("shell".into()))
            .unwrap();
        assert_eq!(shell.status, SpanStatus::Completed);
        assert!(!shell.output_artifact_ids.is_empty());

        let stdout_id = shell.output_artifact_ids[0].clone();
        let content = storage
            .read_artifact_content(&execution.target_run.run_id, &stdout_id)
            .unwrap();
        assert_eq!(String::from_utf8(content).unwrap(), "hello");
    }

    #[test]
    fn blocked_upstream_prevents_downstream_execution() {
        let storage = Arc::new(InMemoryStorage::new());
        let run = RunRecord::new(
            RunId("run-upstream".into()),
            TraceId("trace-upstream".into()),
            "upstream",
            "agent.main",
            "adapter",
            "0.1.0",
            1,
        );
        storage.insert_run(run.clone()).unwrap();

        let planner = SpanRecord {
            run_id: run.run_id.clone(),
            span_id: SpanId("planner".into()),
            trace_id: run.trace_id.clone(),
            parent_span_id: None,
            sequence_no: 1,
            kind: SpanKind::PlannerStep,
            name: "planner".into(),
            status: SpanStatus::Completed,
            started_at: 1,
            ended_at: Some(2),
            replay_policy: ReplayPolicy::RecordOnly,
            executor_kind: None,
            executor_version: None,
            input_artifact_ids: Vec::new(),
            output_artifact_ids: Vec::new(),
            snapshot_id: None,
            input_fingerprint: None,
            output_fingerprint: Some("planner".into()),
            environment_fingerprint: None,
            attributes: Document::new(),
            error_code: None,
            error_summary: None,
            cost: CostMetrics::default(),
        };
        let llm = SpanRecord {
            span_id: SpanId("llm".into()),
            sequence_no: 2,
            kind: SpanKind::LlmCall,
            name: "generate fix".into(),
            replay_policy: ReplayPolicy::RerunnableSupported,
            output_fingerprint: Some("llm-out".into()),
            parent_span_id: Some(SpanId("planner".into())),
            ..planner.clone()
        };
        let file_write = SpanRecord {
            span_id: SpanId("file-write".into()),
            sequence_no: 3,
            kind: SpanKind::FileWrite,
            name: "apply patch".into(),
            replay_policy: ReplayPolicy::RerunnableSupported,
            output_fingerprint: Some("write-out".into()),
            parent_span_id: Some(SpanId("planner".into())),
            ..planner.clone()
        };
        let shell = SpanRecord {
            span_id: SpanId("shell".into()),
            sequence_no: 4,
            kind: SpanKind::ShellCommand,
            name: "printf should-not-run".into(),
            replay_policy: ReplayPolicy::RerunnableSupported,
            output_fingerprint: Some("shell-out".into()),
            parent_span_id: Some(SpanId("planner".into())),
            ..planner.clone()
        };

        storage.upsert_span(planner).unwrap();
        storage.upsert_span(llm).unwrap();
        storage.upsert_span(file_write).unwrap();
        storage.upsert_span(shell).unwrap();
        // "file-write depends on llm" (file-write uses llm's output)
        storage
            .insert_edge(SpanEdgeRecord {
                edge_id: EdgeId("edge-llm-file".into()),
                run_id: run.run_id.clone(),
                from_span_id: SpanId("file-write".into()),
                to_span_id: SpanId("llm".into()),
                kind: EdgeKind::DataDependsOn,
                attributes: Document::new(),
            })
            .unwrap();
        // "shell depends on file-write" (shell uses file-write's output)
        storage
            .insert_edge(SpanEdgeRecord {
                edge_id: EdgeId("edge-file-shell".into()),
                run_id: run.run_id.clone(),
                from_span_id: SpanId("shell".into()),
                to_span_id: SpanId("file-write".into()),
                kind: EdgeKind::DataDependsOn,
                attributes: Document::new(),
            })
            .unwrap();

        let engine = ReplayEngine::new(storage.clone(), CompositeExecutorRegistry::new());
        let execution = engine
            .execute_fork(BranchRequest {
                source_run_id: run.run_id.clone(),
                fork_span_id: SpanId("llm".into()),
                patch_manifest: PatchManifest {
                    patch_type: PatchType::PromptEdit,
                    target_artifact_id: None,
                    replacement: Value::Text("fix it".into()),
                    note: None,
                    created_at: 10,
                },
                created_by: None,
            })
            .unwrap();

        assert_eq!(execution.target_run.status, RunStatus::Blocked);

        let llm = storage
            .get_span(&execution.target_run.run_id, &SpanId("llm".into()))
            .unwrap();
        let file_write = storage
            .get_span(&execution.target_run.run_id, &SpanId("file-write".into()))
            .unwrap();
        let shell = storage
            .get_span(&execution.target_run.run_id, &SpanId("shell".into()))
            .unwrap();

        assert_eq!(llm.status, SpanStatus::Blocked);
        let llm_err = llm.error_summary.as_deref().expect("llm should have error");
        assert!(
            llm_err.starts_with("replay blocked:"),
            "unexpected error: {llm_err}"
        );
        assert!(
            llm_err.contains("LlmCall"),
            "should mention LlmCall: {llm_err}"
        );
        assert_eq!(file_write.status, SpanStatus::Blocked);
        assert!(
            file_write
                .error_summary
                .as_deref()
                .unwrap_or_default()
                .contains("upstream span llm did not complete")
        );
        assert!(file_write.output_artifact_ids.is_empty());
        assert_eq!(shell.status, SpanStatus::Blocked);
        assert!(
            shell
                .error_summary
                .as_deref()
                .unwrap_or_default()
                .contains("upstream span file-write did not complete")
        );
        assert!(shell.output_artifact_ids.is_empty());
    }

    // -- Hermetic replay with patched inputs ---------------------------------

    mod hermetic {
        use std::path::{Path, PathBuf};

        use replaykit_collector::{ArtifactSpec, BeginRun, Collector, EdgeSpec, EndSpan, SpanSpec};
        use replaykit_core_model::HostMetadata;

        use super::*;
        use crate::executors::{
            CompositeExecutorRegistry, ModelExecutorMode, ModelFixture, RecordedModelExecutor,
        };

        pub(super) fn workspace(name: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "rk-engine-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        struct Recorder<'a> {
            collector: &'a Collector<InMemoryStorage>,
            run: RunRecord,
            clock: u64,
        }

        impl Recorder<'_> {
            fn span(
                &mut self,
                id: &str,
                parent: Option<&str>,
                kind: SpanKind,
                attributes: &[(&str, Value)],
                act: impl FnOnce() -> (SpanStatus, Option<(ArtifactType, String)>, String),
            ) {
                self.clock += 1;
                self.collector
                    .start_span(
                        &self.run.run_id,
                        &self.run.trace_id,
                        SpanSpec {
                            span_id: Some(SpanId(id.into())),
                            parent_span_id: parent.map(|p| SpanId(p.into())),
                            kind,
                            name: id.into(),
                            started_at: self.clock,
                            replay_policy: ReplayPolicy::RerunnableSupported,
                            executor_kind: None,
                            executor_version: None,
                            input_artifact_ids: Vec::new(),
                            input_fingerprint: None,
                            environment_fingerprint: None,
                            attributes: attributes
                                .iter()
                                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                                .collect(),
                        },
                    )
                    .unwrap();
                let (status, output, fingerprint) = act();
                let mut outputs = Vec::new();
                if let Some((artifact_type, text)) = output {
                    let artifact = self
                        .collector
                        .add_artifact(
                            &self.run.run_id,
                            Some(&SpanId(id.into())),
                            ArtifactSpec {
                                artifact_type,
                                mime: "text/plain".into(),
                                sha256: String::new(),
                                byte_len: 0,
                                blob_path: String::new(),
                                summary: Document::new(),
                                redaction: Document::new(),
                                created_at: self.clock,
                                content: Some(text.into_bytes()),
                            },
                        )
                        .unwrap();
                    outputs.push(artifact.artifact_id);
                }
                self.clock += 1;
                self.collector
                    .end_span(
                        &self.run.run_id,
                        &SpanId(id.into()),
                        EndSpan {
                            ended_at: self.clock,
                            status,
                            output_artifact_ids: outputs,
                            snapshot_id: None,
                            output_fingerprint: Some(fingerprint),
                            error_code: None,
                            error_summary: (status == SpanStatus::Failed)
                                .then(|| "check failed".to_string()),
                            cost: CostMetrics::default(),
                        },
                    )
                    .unwrap();
            }

            fn edge(&self, from: &str, to: &str) {
                self.collector
                    .add_edge(
                        &self.run.run_id,
                        EdgeSpec {
                            from_span_id: SpanId(from.into()),
                            to_span_id: SpanId(to.into()),
                            kind: EdgeKind::DataDependsOn,
                            attributes: Document::new(),
                        },
                    )
                    .unwrap();
            }
        }

        fn sh(dir: &Path, command: &str) -> (i32, String, String) {
            let out = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(command)
                .current_dir(dir)
                .output()
                .unwrap();
            (
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stdout).into_owned(),
                String::from_utf8_lossy(&out.stderr).into_owned(),
            )
        }

        fn text(value: &str) -> Value {
            Value::Text(value.into())
        }

        /// Record a tiny agent run in `dir`: list files, ask a "model" for
        /// new file content, write it, then run a check that wants "v2".
        /// The recorded agent writes "v1-fixed", so the check fails.
        pub(super) fn record_agent(dir: &Path) -> (Arc<InMemoryStorage>, RunRecord) {
            std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
            std::fs::write(
                dir.join("check.sh"),
                "grep -q v2 app.txt && echo pass || { echo \"fail: $(cat app.txt)\"; echo wrong >&2; exit 1; }\n",
            )
            .unwrap();
            let storage = Arc::new(InMemoryStorage::new());
            let collector = Collector::new(storage.clone());
            let run = collector
                .begin_run(BeginRun {
                    title: "tiny agent".into(),
                    entrypoint: "test".into(),
                    adapter_name: "test".into(),
                    adapter_version: "0".into(),
                    started_at: 0,
                    git_sha: None,
                    environment_fingerprint: None,
                    host: HostMetadata::default(),
                    labels: Vec::new(),
                    workspace_root: Some(dir.to_string_lossy().into_owned()),
                })
                .unwrap();
            let mut rec = Recorder {
                collector: &collector,
                run: run.clone(),
                clock: 0,
            };
            rec.clock += 1;
            collector
                .start_span(
                    &run.run_id,
                    &run.trace_id,
                    SpanSpec {
                        span_id: Some(SpanId("root".into())),
                        parent_span_id: None,
                        kind: SpanKind::Run,
                        name: "root".into(),
                        started_at: rec.clock,
                        replay_policy: ReplayPolicy::RecordOnly,
                        executor_kind: None,
                        executor_version: None,
                        input_artifact_ids: Vec::new(),
                        input_fingerprint: None,
                        environment_fingerprint: None,
                        attributes: Document::new(),
                    },
                )
                .unwrap();

            let listing = sh(dir, "ls").1;
            rec.span(
                "list",
                Some("root"),
                SpanKind::ToolCall,
                &[("command", text("ls"))],
                || {
                    let fp = sha256_hex(listing.as_bytes());
                    (
                        SpanStatus::Completed,
                        Some((ArtifactType::ToolOutput, listing)),
                        fp,
                    )
                },
            );
            rec.span(
                "echo_files",
                Some("root"),
                SpanKind::ShellCommand,
                &[
                    ("command", text("echo files: app.txt check.sh")),
                    ("cwd", text(".")),
                    (
                        attrs::INPUT_BINDINGS,
                        Value::Object(
                            [("command".to_string(), text("echo files: {{list|words}}"))]
                                .into_iter()
                                .collect(),
                        ),
                    ),
                ],
                || {
                    let (code, out, err) = sh(dir, "echo files: app.txt check.sh");
                    let fp = crate::executors::tests_support::shell_fp(code, &out, &err);
                    (
                        SpanStatus::Completed,
                        Some((ArtifactType::ShellStdout, out)),
                        fp,
                    )
                },
            );
            rec.edge("echo_files", "list");
            rec.span(
                "gen",
                Some("root"),
                SpanKind::LlmCall,
                &[
                    ("model", text("scripted")),
                    ("provider", text("local")),
                    ("prompt", text("Return the fixed app.txt.")),
                ],
                || {
                    (
                        SpanStatus::Completed,
                        Some((ArtifactType::ModelResponse, "v1-fixed\n".into())),
                        sha256_hex(b"v1-fixed\n"),
                    )
                },
            );
            rec.span(
                "write",
                Some("root"),
                SpanKind::FileWrite,
                &[
                    ("path", text("app.txt")),
                    ("content", text("v1-fixed\n")),
                    (
                        attrs::INPUT_BINDINGS,
                        Value::Object(
                            [("content".to_string(), text("{{gen}}"))]
                                .into_iter()
                                .collect(),
                        ),
                    ),
                ],
                || {
                    std::fs::write(dir.join("app.txt"), "v1-fixed\n").unwrap();
                    (
                        SpanStatus::Completed,
                        Some((ArtifactType::FileDiff, "-v1\n+v1-fixed\n".into())),
                        sha256_hex(b"v1-fixed\n"),
                    )
                },
            );
            rec.edge("write", "gen");
            rec.span(
                "check",
                Some("root"),
                SpanKind::ShellCommand,
                &[("command", text("sh check.sh")), ("cwd", text("."))],
                || {
                    let (code, out, err) = sh(dir, "sh check.sh");
                    let fp = crate::executors::tests_support::shell_fp(code, &out, &err);
                    let status = if code == 0 {
                        SpanStatus::Completed
                    } else {
                        SpanStatus::Failed
                    };
                    (status, Some((ArtifactType::ShellStdout, out)), fp)
                },
            );
            rec.edge("check", "write");
            rec.clock += 1;
            collector
                .end_span(
                    &run.run_id,
                    &SpanId("root".into()),
                    EndSpan {
                        ended_at: rec.clock,
                        status: SpanStatus::Failed,
                        output_artifact_ids: Vec::new(),
                        snapshot_id: None,
                        output_fingerprint: None,
                        error_code: None,
                        error_summary: Some("check failed".into()),
                        cost: CostMetrics::default(),
                    },
                )
                .unwrap();
            collector
                .finish_run(&run.run_id, rec.clock + 1, RunStatus::Failed, None)
                .unwrap();
            (storage, run)
        }

        fn fork(
            storage: &Arc<InMemoryStorage>,
            registry: CompositeExecutorRegistry,
            run: &RunRecord,
            span: &str,
            patch_type: PatchType,
            replacement: &str,
        ) -> BranchExecution {
            ReplayEngine::new(storage.clone(), registry)
                .execute_fork(BranchRequest {
                    source_run_id: run.run_id.clone(),
                    fork_span_id: SpanId(span.into()),
                    patch_manifest: PatchManifest {
                        patch_type,
                        target_artifact_id: None,
                        replacement: Value::Text(replacement.into()),
                        note: None,
                        created_at: 100,
                    },
                    created_by: None,
                })
                .unwrap()
        }

        fn output(storage: &InMemoryStorage, run_id: &RunId, span: &str) -> String {
            let span = storage.get_span(run_id, &SpanId(span.into())).unwrap();
            let id = span
                .output_artifact_ids
                .first()
                .expect("span has an output");
            String::from_utf8(storage.read_artifact_content(run_id, id).unwrap()).unwrap()
        }

        fn status(storage: &InMemoryStorage, run_id: &RunId, span: &str) -> SpanStatus {
            storage
                .get_span(run_id, &SpanId(span.into()))
                .unwrap()
                .status
        }

        #[test]
        fn recording_captures_a_workspace_timeline() {
            let dir = workspace("timeline");
            let (storage, run) = record_agent(&dir);
            let points = list_workspace_points(&*storage, &run.run_id).unwrap();
            // before+after for 6 spans, plus the end of the run.
            assert_eq!(points.len(), 13);
            assert_eq!(points.last().unwrap().phase, WorkspacePhase::End);
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn file_content_patch_runs_in_a_sandbox_and_never_touches_the_live_tree() {
            let dir = workspace("fcontent");
            let (storage, run) = record_agent(&dir);
            // Later edits to the live tree must not leak into replays.
            std::fs::write(dir.join("app.txt"), "LIVE EDIT\n").unwrap();

            let exec = fork(
                &storage,
                CompositeExecutorRegistry::new(),
                &run,
                "write",
                PatchType::FileContentOverride,
                "v2\n",
            );
            let target = &exec.target_run.run_id;
            assert!(
                exec.plan.blocked_spans.is_empty(),
                "{:?}",
                exec.plan.blocked
            );
            assert_eq!(output(&storage, target, "check"), "pass\n");
            assert_eq!(status(&storage, target, "check"), SpanStatus::Completed);
            // The root re-derives its status from the replayed children.
            assert_eq!(status(&storage, target, "root"), SpanStatus::Completed);
            assert_eq!(exec.target_run.status, RunStatus::Completed);
            // The live tree was not written.
            assert_eq!(
                std::fs::read_to_string(dir.join("app.txt")).unwrap(),
                "LIVE EDIT\n"
            );
            // The branch's final workspace holds the patched file.
            let end = list_workspace_points(&*storage, target)
                .unwrap()
                .into_iter()
                .rfind(|p| p.phase == WorkspacePhase::End)
                .unwrap();
            let archive = load_workspace_archive(&*storage, &end.snapshot).unwrap();
            assert_eq!(archive.file_content("app.txt"), Some(&b"v2\n"[..]));
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn command_patch_changes_the_command_and_sees_the_recorded_state() {
            let dir = workspace("command");
            let (storage, run) = record_agent(&dir);
            std::fs::write(dir.join("app.txt"), "LIVE EDIT\n").unwrap();

            let exec = fork(
                &storage,
                CompositeExecutorRegistry::new(),
                &run,
                "check",
                PatchType::CommandOverride,
                "cat app.txt",
            );
            let target = &exec.target_run.run_id;
            let check = storage.get_span(target, &SpanId("check".into())).unwrap();
            assert_eq!(check.attributes.get("command"), Some(&text("cat app.txt")));
            // The sandbox is the workspace as it was right before `check`
            // ran in the recording: after the agent's write, not the live edit.
            assert_eq!(output(&storage, target, "check"), "v1-fixed\n");
            assert_eq!(check.status, SpanStatus::Completed);
            assert_eq!(exec.target_run.status, RunStatus::Completed);
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn env_patch_reaches_the_shell() {
            let dir = workspace("env");
            let (storage, run) = record_agent(&dir);
            let exec = fork(
                &storage,
                CompositeExecutorRegistry::new(),
                &run,
                "check",
                PatchType::CommandOverride,
                r#"{"command": "printf '%s' \"$MODE\"", "env": {"MODE": "strict"}}"#,
            );
            assert_eq!(output(&storage, &exec.target_run.run_id, "check"), "strict");
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn tool_output_patch_flows_into_bound_downstream_commands() {
            let dir = workspace("tool");
            let (storage, run) = record_agent(&dir);
            let exec = fork(
                &storage,
                CompositeExecutorRegistry::new(),
                &run,
                "list",
                PatchType::ToolOutputOverride,
                "only.txt\nother.txt\n",
            );
            let target = &exec.target_run.run_id;
            let echo = storage
                .get_span(target, &SpanId("echo_files".into()))
                .unwrap();
            assert_eq!(
                echo.attributes.get("command"),
                Some(&text("echo files: only.txt other.txt"))
            );
            assert_eq!(
                output(&storage, target, "echo_files"),
                "files: only.txt other.txt\n"
            );
            // check does not depend on list: reused, still failing.
            assert_eq!(status(&storage, target, "check"), SpanStatus::Failed);
            assert_eq!(exec.target_run.status, RunStatus::Failed);
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn prompt_edit_reruns_end_to_end_with_recorded_responses() {
            let dir = workspace("prompt");
            let (storage, run) = record_agent(&dir);
            let registry = || {
                CompositeExecutorRegistry::new().with_model_mode(ModelExecutorMode::Recorded(
                    RecordedModelExecutor::new(vec![ModelFixture {
                        model: Some("scripted".into()),
                        prompt: "Return the fixed app.txt. It must say v2.".into(),
                        response: "v2\n".into(),
                    }]),
                ))
            };
            let exec = fork(
                &storage,
                registry(),
                &run,
                "gen",
                PatchType::PromptEdit,
                "Return the fixed app.txt. It must say v2.",
            );
            let target = &exec.target_run.run_id;
            assert!(exec.plan.blocked.is_empty(), "{:?}", exec.plan.blocked);
            assert_eq!(output(&storage, target, "gen"), "v2\n");
            let write = storage.get_span(target, &SpanId("write".into())).unwrap();
            assert_eq!(write.attributes.get("content"), Some(&text("v2\n")));
            assert_eq!(output(&storage, target, "check"), "pass\n");
            assert_eq!(exec.target_run.status, RunStatus::Completed);

            // A prompt with no recorded response is reported blocked by the
            // plan, and execution blocks exactly those spans.
            let request = BranchRequest {
                source_run_id: run.run_id.clone(),
                fork_span_id: SpanId("gen".into()),
                patch_manifest: PatchManifest {
                    patch_type: PatchType::PromptEdit,
                    target_artifact_id: None,
                    replacement: Value::Text("unknown prompt".into()),
                    note: None,
                    created_at: 101,
                },
                created_by: None,
            };
            let engine = ReplayEngine::new(storage.clone(), registry());
            let plan = engine.plan_fork(&request).unwrap();
            assert_eq!(
                plan.blocked_spans,
                vec![
                    SpanId("root".into()),
                    SpanId("gen".into()),
                    SpanId("write".into()),
                    SpanId("check".into())
                ]
            );
            assert!(plan.blocked[1].reason.contains("no recorded response"));
            let exec = engine.execute_fork(request).unwrap();
            let blocked = storage
                .list_spans(&exec.target_run.run_id)
                .unwrap()
                .into_iter()
                .filter(|s| s.status == SpanStatus::Blocked)
                .map(|s| s.span_id)
                .collect::<BTreeSet<_>>();
            assert_eq!(blocked, plan.blocked_spans.into_iter().collect());
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn unchanged_replays_reuse_downstream_work() {
            let dir = workspace("cutoff");
            let (storage, run) = record_agent(&dir);
            // Re-running `gen` with its recorded prompt replays its recorded
            // response, so `write` and `check` are reused, not re-run.
            let exec = fork(
                &storage,
                CompositeExecutorRegistry::new()
                    .with_model_mode(ModelExecutorMode::Recorded(RecordedModelExecutor::default())),
                &run,
                "gen",
                PatchType::EnvVarOverride,
                "IRRELEVANT=1",
            );
            let target = &exec.target_run.run_id;
            assert_eq!(
                exec.replay_job.progress.get("executed"),
                Some(&Value::Int(1))
            );
            assert_eq!(
                exec.replay_job.progress.get("reused_unchanged"),
                Some(&Value::Int(2))
            );
            assert_eq!(status(&storage, target, "check"), SpanStatus::Failed);
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn blocked_model_plan_matches_execution() {
            let dir = workspace("agree");
            let (storage, run) = record_agent(&dir);
            let request = BranchRequest {
                source_run_id: run.run_id.clone(),
                fork_span_id: SpanId("gen".into()),
                patch_manifest: PatchManifest {
                    patch_type: PatchType::PromptEdit,
                    target_artifact_id: None,
                    replacement: Value::Text("anything".into()),
                    note: None,
                    created_at: 100,
                },
                created_by: None,
            };
            let engine = ReplayEngine::new(storage.clone(), CompositeExecutorRegistry::new());
            let plan = engine.plan_fork(&request).unwrap();
            assert!(plan.blocked[1].reason.contains("model mode is 'blocked'"));
            let exec = engine.execute_fork(request).unwrap();
            let blocked = storage
                .list_spans(&exec.target_run.run_id)
                .unwrap()
                .into_iter()
                .filter(|s| s.status == SpanStatus::Blocked)
                .map(|s| s.span_id)
                .collect::<Vec<_>>();
            let mut planned = plan.blocked_spans.clone();
            planned.sort();
            let mut blocked_sorted = blocked.clone();
            blocked_sorted.sort();
            assert_eq!(blocked_sorted, planned);
            assert_eq!(exec.target_run.status, RunStatus::Blocked);
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn snapshot_override_replays_against_another_tree() {
            let dir = workspace("snapover");
            let (storage, run) = record_agent(&dir);
            let other = workspace("snapover-other");
            std::fs::write(other.join("app.txt"), "v2 from elsewhere\n").unwrap();
            std::fs::write(other.join("check.sh"), "grep -q v2 app.txt && echo pass\n").unwrap();
            let exec = fork(
                &storage,
                CompositeExecutorRegistry::new(),
                &run,
                "check",
                PatchType::SnapshotOverride,
                &other.to_string_lossy(),
            );
            assert_eq!(output(&storage, &exec.target_run.run_id, "check"), "pass\n");
            let _ = std::fs::remove_dir_all(&dir);
            let _ = std::fs::remove_dir_all(&other);
        }
    }

    fn summary_from_pairs(pairs: &[(&str, &str)]) -> Document {
        let mut summary = Document::new();
        for (key, value) in pairs {
            summary.insert((*key).into(), Value::Text((*value).into()));
        }
        summary
    }
}
