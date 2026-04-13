# ReplayKit Worktree Instruction: API And CLI

Status: active implementation brief

Recommended worktree name: `wt/api`

Recommended branch name: `feat/api-local-http-cli`

Primary audience: the agent responsible for the local query/command API boundary and the CLI as the first real consumer

Primary goal: turn the current in-process service facade into a stable, local API surface and a useful debugger-grade CLI

Related local documents:

- [architecture.md](./architecture.md)
- [delivery-plan.md](./delivery-plan.md)
- [replay-semantics.md](./replay-semantics.md)
- [product.md](./product.md)

---

## 0. Why this worktree exists

ReplayKit is supposed to feel like a debugger, not a pile of library crates.

The architecture is explicit that:

- the UI should not query SQLite directly
- the CLI should exercise the same logical boundary as the UI
- replay, diff, and failure information should be exposed through typed queries and commands

Right now the repo has:

- a useful in-process `ReplayKitService`
- replay and diff engines
- a demo CLI with only `demo` and `demo-branch`

That is enough to prove the internals exist.

It is not enough to support:

- a real frontend
- a stable local product boundary
- a serious manual debugging workflow

This worktree exists to convert internal capability into a consumable service.

---

## 1. Product and architecture context

ReplayKit is not a raw database explorer.

The API is part of the product.

Users need to ask questions like:

- what runs exist
- what failed
- what span should I inspect
- what depends on this span
- what is the branch status
- where was the first divergence
- why did replay block

Those are not raw row-level questions.

They are debugger questions.

Read these sections of [architecture.md](./architecture.md) before coding:

- section 5.4 `Replay and diff engines`
- section 5.5 `API service`
- section 6.4 through 6.7
- section 23 `Diff engine`
- section 24 `Failure forensics engine`
- section 25 `API layer`
- section 26 `CLI architecture`
- section 38 `API contracts in more detail`
- section 41 `UI data flow`
- section 59 `UX rules implied by architecture`

This worktree should express those ideas in code, not reinterpret them.

---

## 2. Current repo state

The repo already contains:

- `crates/api`
  - `ReplayKitService`
  - a direct Rust facade over collector, replay, diff, and storage
- `crates/cli`
  - two basic commands:
    - `demo`
    - `demo-branch`
- `crates/replay-engine`
  - branch planning and execution
- `crates/diff-engine`
  - cached diff creation and lookup

What is missing:

- a versioned local HTTP JSON API
- stable API-facing view models
- route-level error contracts
- a CLI with meaningful commands for inspection and replay
- a backend boundary the frontend can target without knowing Rust internals

That is your scope.

---

## 3. Your mission

Build the first real local API and CLI surface for ReplayKit.

That means:

1. expose query and command flows over a local transport
2. keep payloads typed and stable
3. expand the CLI into a useful debugger harness
4. surface replay and diff semantics clearly
5. keep the frontend from needing to know storage internals

You are not building the write transport in this worktree.

You are not building the frontend in this worktree.

You are not redesigning the canonical model.

You are building the product boundary for read/query/command behavior.

---

## 4. Ownership boundaries

### 4.1 Files and areas you may edit

You may edit:

- `crates/api/**`
- `crates/cli/**`
- `crates/replay-engine/**`
- `crates/diff-engine/**`
- related `Cargo.toml` files
- `README.md` if API or CLI usage changes should be documented

### 4.2 Files and areas you should avoid editing

Avoid editing:

- `crates/storage/**`
- `crates/collector/**`
- `crates/sdk-rust-tracing/**`
- `apps/web/**`
- `examples/**`

Only touch `crates/core-model/**` if a tiny additive type is unavoidable.

### 4.3 Boundary reminders

The architecture wants:

- `storage` to own persistence
- `collector` to own ingestion validation
- `replay-engine` to own branch execution
- `diff-engine` to own comparison logic
- `api` to own query and command composition
- `cli` to consume the API

Do not pull query shaping down into storage and do not leak storage rows directly up to the UI or CLI if a view model is cleaner.

---

## 5. Core principles you must preserve

### 5.1 API over direct DB access

The UI should not know SQL details.

The CLI should not be a special case that bypasses the product boundary.

### 5.2 Typed errors

A debugger-grade tool needs explicit failure modes.

At minimum distinguish:

- not found
- invalid patch
- replay blocked
- integrity error
- incompatible executor
- storage unavailable

### 5.3 View stability

The API should not force the frontend to understand every internal record.

Shape payloads intentionally.

### 5.4 Local-first transport

Local HTTP JSON is the simplest good answer.

Do not overbuild IPC.

### 5.5 Query clarity over genericity

Prefer explicit endpoints that answer debugger questions over one giant generic query surface.

---

## 6. Detailed deliverables

### 6.1 Local HTTP API

Implement a local API service with versioned routes, ideally under `/api/v1/...`.

You may choose:

- a dedicated binary
- or a library server entry point plus a thin binary if that feels cleaner

The important thing is that there is a real local API surface the future frontend can call.

### 6.2 Query endpoints

At minimum deliver query endpoints for:

- list runs
- get run summary
- get run tree
- get span detail
- get span artifacts
- get span dependencies
- get run diff summary
- get replay job status

If `run timeline` is practical in the current model, include it.

If not, leave the route stubbed or clearly deferred, but do not block the rest of the work.

### 6.3 Command endpoints

At minimum deliver command endpoints for:

- create branch
- start forked replay job if distinct from branch creation in your API design
- recorded replay session initiation if you decide it deserves an explicit route

If canceling replay jobs is cheap to support, include it.

If not, do not fake it.

### 6.4 API-facing view models

Introduce deliberate view structs where useful.

Examples of likely needed models:

- `RunSummaryView`
- `RunTreeView`
- `SpanDetailView`
- `ArtifactPreviewView`
- `DependencyView`
- `ReplayJobView`
- `RunDiffSummaryView`

Good view models:

- hide low-level storage noise
- preserve the semantic fields the UI actually needs
- give stable response shapes

### 6.5 Error contract

Return typed error bodies rather than plain text where practical.

An acceptable error body shape is something like:

```json
{
  "code": "replay_blocked",
  "message": "span final-answer cannot be rerun because no executor is registered",
  "details": {
    "run_id": "...",
    "span_id": "..."
  }
}
```

Keep it simple, but make it explicit.

### 6.6 CLI expansion

Expand the CLI toward the architecture’s command set:

- `replaykit runs list`
- `replaykit runs show <run>`
- `replaykit runs tree <run>`
- `replaykit runs diff <a> <b>`
- `replaykit replay recorded <run>`
- `replaykit replay fork <run> --span <id> --patch <file>`

If full command parity is too much for one pass, prioritize:

1. `runs list`
2. `runs tree`
3. `runs diff`
4. `replay fork`

The CLI should feel like a debugger harness, not a demo stub.

### 6.7 API and CLI alignment

The CLI should consume:

- the actual HTTP API
- or at minimum the same handler/view-model layer that backs the HTTP routes

Do not implement two independent query semantics.

---

## 7. Explicit non-goals

Do not add these in this worktree:

- storage schema redesign
- collector ingestion redesign
- blob-store internals
- frontend rendering
- SDK instrumentation redesign
- auth
- cloud sync
- multi-user collaboration

Also do not spend time turning this into a production deployment platform.

This is a local service for a local debugger.

---

## 8. Detailed implementation guidance

### 8.1 Transport choice

Local HTTP JSON is the default recommendation.

Reasons:

- the frontend can consume it directly
- the CLI can call it or share the same handler layer
- payloads are inspectable
- routing and versioning are straightforward

Do not overcomplicate this with custom protocols unless there is a clear reason.

### 8.2 View shaping

The current `RunRecord`, `SpanRecord`, and related types are good core contracts.

They are not automatically the best API contracts.

Examples where API shaping is useful:

- adding derived fields like failure badges, branch indicators, and child counts
- flattening related records into a span inspector payload
- returning artifact previews instead of full artifact rows where appropriate

Try to keep the API stable even if internals evolve.

### 8.3 Timeline route

If the existing data model can expose timeline information cheaply, include it.

If not, avoid inventing a second ordering model.

A sequence-based first version is acceptable if well documented.

### 8.4 Replay command model

Branch creation and replay initiation may be the same operation in the current internals.

That is fine.

What matters is that the API clearly expresses:

- what the request is
- whether it succeeded
- what branch/run/job was created
- whether replay completed or blocked

### 8.5 Failure forensics exposure

If the current engines do not yet compute all forensics views, do not invent fake analytics.

But the API should already leave room for:

- first divergent span
- blocked replay reason
- failure class
- dirty reasons

Expose what is real and stable now.

### 8.6 CLI output quality

Make the CLI outputs useful for humans:

- readable tree rendering
- status badges or readable labels
- explicit blocked replay explanation
- compact diff summary

Avoid giant raw debug dumps unless behind an explicit verbose flag.

### 8.7 API versioning

Start versioning early even if only `v1` exists.

That means:

- route namespace
- possibly top-level response envelope decisions
- stable structs

Do not leave versioning implicit.

---

## 9. Concrete task breakdown

### Phase 1: map current service methods to product endpoints

Inventory what already exists in `ReplayKitService`:

- list runs
- run tree
- create branch
- diff lookup

Then identify missing pieces:

- route layer
- view models
- route-specific error mapping
- CLI command parser and execution

### Phase 2: build API-facing models and error types

Define:

- endpoint inputs
- endpoint outputs
- typed error payloads

Keep these coherent and minimal.

### Phase 3: build the local HTTP layer

Implement:

- route registration
- handler functions
- serialization
- error mapping

Keep this thin. Business logic belongs in service-level functions, not routing glue.

### Phase 4: expand the CLI

Implement the new commands and route them through the API or shared handler layer.

Use the CLI as the first serious consumer.

### Phase 5: harden edge cases

Add tests for:

- blocked replay
- missing runs
- missing spans
- diff not found
- invalid patch input

---

## 10. Testing requirements

### 10.1 API route tests

Add tests for:

- list runs success
- run tree success
- span detail success if implemented
- create branch success
- cached diff lookup success
- typed error response for missing run
- typed error response for blocked replay

### 10.2 Service integration tests

Add tests for:

- branch creation through the API layer
- diff creation and retrieval through the API layer
- stable view-model shape for main routes

### 10.3 CLI tests

Add tests for:

- command parsing
- tree rendering
- diff rendering
- replay fork output
- error surfacing

### 10.4 Stability tests

If practical, add golden JSON assertions for a few key responses:

- run list item
- run tree
- run diff summary

That helps stabilize the frontend contract.

---

## 11. Verification commands

At minimum run:

```bash
cargo fmt
cargo test -p replaykit-api
cargo test -p replaykit-cli
cargo clippy -p replaykit-api -p replaykit-cli --all-targets -- -D warnings
```

If you add a runnable API server binary, run it locally and exercise at least one endpoint manually or with a small automated smoke test.

---

## 12. What good deliverables look like

A good delivery from this worktree has these properties:

- the future frontend has a stable local API to call
- the CLI can meaningfully inspect runs and branches
- replay and diff semantics are visible without reading Rust internals
- errors are typed and understandable
- API handlers remain thin and readable

A weak delivery looks like:

- a server that just exposes raw storage rows
- a CLI still limited to demo commands
- route handlers filled with business logic
- untyped stringly errors

Do not stop at a transport shell.

---

## 13. Known traps to avoid

- Do not couple the API to SQLite schema details.
- Do not make the frontend consume `RunRecord` blindly if a view model is clearer.
- Do not build query semantics directly in CLI commands with no shared layer.
- Do not introduce overly generic “query anything” endpoints that are hard to evolve.
- Do not block on the write transport if it is not ready. The query/command side can proceed against current in-process services.

---

## 14. Handoff format

At the end, provide:

1. a short summary of what changed
2. the new API routes or command surfaces added
3. verification commands run and whether they passed
4. any assumptions the frontend worktree should know about

Especially call out:

- route paths
- payload shapes
- error code shapes
- any intentionally deferred endpoints

---

## 15. Final decision rule

When faced with a design choice in this worktree, choose the option that most improves:

- backend/frontend contract clarity
- debugger usability through CLI and local API
- explicit replay and diff semantics
- compatibility with the existing architecture

Do not optimize for backend cleverness.

Do not optimize for future distributed deployment.

Optimize for a clean local product boundary.
