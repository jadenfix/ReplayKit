# Replay Semantics

## Purpose

This document isolates the replay model from the rest of the architecture so implementation work can stay aligned on the core contract.

## Replay modes

### Recorded replay

Recorded replay:

- never executes user code, tools, or models
- rebuilds the run from persisted spans, artifacts, and snapshots
- is the baseline for demos, postmortems, and inspection

### Forked replay

Forked replay:

- starts from an existing run
- applies a patch at a selected span
- computes the dirty subgraph
- reuses safe upstream state
- re-executes only supported dirty nodes
- persists a new branch run

## Replay unit

The replay unit is a completed semantic span.
ReplayKit does not replay inside arbitrary code blocks in v1.

That means the minimum re-execution target is:

- one `llm_call`
- one `tool_call`
- one `shell_command`
- one `file_read`
- one `file_write`
- one `retrieval`
- one `browser_action`

## Replay policy

Every span needs a replay policy.

### `record_only`

Use when:

- the system can inspect the span
- the system cannot safely rerun the span
- replay must rely on recorded artifacts

Examples:

- opaque proprietary tool step with no executor
- human input
- unsupported third-party browser interaction

### `rerunnable_supported`

Use when:

- the system has a registered executor
- the step can be rerun from captured inputs and environment assumptions

Examples:

- supported tool wrapper
- model call through a known provider wrapper
- shell command in a controlled adapter

### `cacheable_if_fingerprint_matches`

Use when:

- the step may be reused if all relevant inputs and environment values match

Examples:

- retrieval against a snapshot-indexed corpus
- tool lookup from a versioned cache

### `pure_reusable`

Use when:

- the step is deterministic and side-effect free
- the output can be reused whenever the input fingerprint matches

Examples:

- pure JSON transform
- deterministic planner post-processing step

## Patch types

### Prompt edit

Changes:

- prompt artifact content
- optional prompt template metadata
- optional model config alongside the prompt

Likely impact:

- current `llm_call`
- downstream spans that depend on the model output

### Tool output override

Changes:

- the output artifact for a `tool_call`
- optionally stderr or structured error payload

Likely impact:

- downstream consumers of the tool output
- final answer if the tool materially mattered

### Env var override

Changes:

- selected environment keys visible to the target executor

Likely impact:

- the patched span
- any downstream steps that use derived outputs from the patched span

### Model config edit

Changes:

- provider name
- model name
- temperature
- max tokens
- system prompt variant

Likely impact:

- current model call
- all downstream consumers of that result

### Retrieval context override

Changes:

- retrieved document set
- ranking order
- retrieval metadata

Likely impact:

- current retrieval span
- downstream planner and answer spans

### Command override (`command_override`)

Replaces the command of a `shell_command` or `tool_call` before it re-runs:
plain text replaces `command`; a JSON object may set `command`, `cwd` and `env`.

### File content override (`file_content_override`)

For a `file_write`, replaces the content written (the write then happens in the
replay sandbox). For a `file_read`, replaces what the read returned.

### Snapshot override (`snapshot_override`)

Starts the replay sandbox from another workspace: a workspace snapshot id of the
source run, or a directory to capture.

## How patch values reach executors

Executors never read recorded attributes directly. For every span it re-runs,
the engine builds an *effective span*:

1. attributes named in the span's `input_bindings` are re-rendered from the
   replayed outputs of the referenced upstream spans
   (`{{span_id}}`, `{{span_id|trim}}`, `{{span_id|words}}`);
2. the patch is applied to the fork span (prompt, command, cwd, env, file
   content, model config). A patched value may itself contain placeholders;
3. env overrides apply to the fork span and all of its descendants.

The effective attributes are persisted on the branch span, so the branch shows
exactly what ran.

## Hermetic workspace

A run that declares `workspace_root` gets a workspace snapshot before and after
every span and at the end of the run (deterministic archives in the blob store,
deduplicated by content). A fork:

- restores the snapshot taken right before the first dirty span into a private
  sandbox (never the live working tree);
- walks the recorded timeline: re-executed spans change the sandbox live, and the
  recorded workspace changes of every span it does not re-execute are re-applied;
- maps recorded paths and `cwd` into the sandbox and blocks any path outside the
  recorded workspace;
- records the replayed workspace timeline in the branch run (so branches can be
  forked hermetically too) and deletes the sandbox unless asked to keep it.

`replaykit runs workspace-diff SRC DST` compares the final workspaces of two runs.

## Model executor modes

`llm_call` replays never reach a provider unless configured:

- `blocked` (default): blocks, and the plan says so before anything runs;
- `recorded`: deterministic; serves responses from a fixtures file keyed by
  (model, exact prompt), or the span's own recorded response when its effective
  prompt and model are unchanged;
- `command`: a pluggable provider; runs a command with the JSON request on stdin
  and uses stdout as the response.

## Planning agrees with execution

`plan_fork` asks the same executor registry that will run the fork whether each
dirty span can execute (with its effective, patched inputs) and propagates
blocks downstream, so `blocked` lists every span the replay will block, each with
a reason. Execution may do *less* than the plan: a dirty span whose upstream
replayed to its recorded output is reused (early cutoff), and output
fingerprints are content hashes so recorders can produce matching values.

Container spans (`run`, `planner_step`, `subgraph`) are executed last by the
planner-step executor: their recorded output is reused and their status is
re-derived from their replayed children (`rollup_spans` in the plan).

## Dependency semantics

ReplayKit must distinguish:

- control structure
- data dependence
- retry lineage
- branch lineage

Control structure alone is not sufficient.

A span may be a child of another span without depending on all sibling outputs.

## Dirty set computation

The dirty set is computed from:

- the patched span
- transitive `data_depends_on` edges
- affected control descendants when data dependencies are incomplete
- policy-based invalidation due to executor version or environment drift

The dirty set should be conservative.
It is better to rerun too much than to present a wrong branch result.

## Reuse rules

ReplayKit may reuse a span result only when:

- the replay policy allows reuse
- the input fingerprint matches
- the executor version is compatible
- the environment fingerprint is compatible
- required artifacts are present and verified

## Blocked replay

Replay must stop clearly when:

- a required executor is not registered (or, for model calls, the model mode
  cannot serve the effective prompt)
- a path or working directory lies outside the recorded workspace
- a required artifact is missing
- redaction removed required input material

Blocked replay is a valid product outcome.
The system should explain exactly why the branch cannot continue.

## Branch persistence

A branch should always be persisted as a full run with:

- a new `run_id`
- `source_run_id`
- `fork_span_id`
- patch manifest reference
- replay job metadata
- diff cache once complete

This keeps reading simple and avoids complex overlay logic in the UI.

## Replay success criteria

The replay model is correct if:

- recorded replay matches the stored original
- selective replay updates only affected downstream spans
- unchanged reusable spans are not rerun
- blocked replay is explicit and inspectable
- the resulting branch can be diffed against the source without special cases
