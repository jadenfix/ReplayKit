# ReplayKit Worktree Instruction: Rust SDK, Fixtures, And Example Agent

Status: active implementation brief

Recommended worktree name: `wt/sdk`

Recommended branch name: `feat/sdk-tracing-fixtures`

Primary audience: the agent responsible for Rust-side instrumentation, semantic wrappers, deterministic examples, and fixture generation

Primary goal: turn the current helper-style SDK into the first real Rust capture story and provide stable fixtures that the rest of the repo can build on

Related local documents:

- [architecture.md](./architecture.md)
- [replay-semantics.md](./replay-semantics.md)
- [delivery-plan.md](./delivery-plan.md)
- [product.md](./product.md)

---

## 0. Why this worktree exists

ReplayKit’s architecture is only as good as the data emitted into it.

A strong replay debugger needs capture that is:

- semantic
- explicit
- repeatable
- representative of real agent behavior

Right now the repo has an SDK crate, but it is closer to a convenience wrapper than a complete `tracing`-aligned adapter.

That is enough for demos.

It is not enough for:

- realistic instrumentation
- fixture-driven verification
- future multi-language adapter work
- a strong story for local coding agents

This worktree exists to build the first believable capture path and to leave behind deterministic fixtures the rest of the system can rely on.

---

## 1. Product and architecture context

ReplayKit does not want raw logs.

It wants semantic execution histories.

That means the SDK must emit:

- the right span kinds
- the right replay policy
- the right artifact attachments
- the right snapshots
- the right dependency edges
- the right cost and error metadata

The first user the architecture cares about is the local coding-agent builder.

That means this worktree should optimize for a believable coding-agent trace shape rather than a generic demo trace.

Read these sections of [architecture.md](./architecture.md) before coding:

- section 5.1 `Adapters and SDKs`
- section 7.1 `Local coding agents`
- section 14 `SDK and adapter architecture`
- section 17.8 `Dependency emission strategy`
- section 35 `Detailed end-to-end scenario: coding agent`
- section 42 `Testing philosophy`
- section 43.1 `Core model tests`
- section 43.3 `Collector tests`
- section 43.4 `Replay engine tests`
- section 44.1 through 44.4
- section 48.4 `Step 4: fixture ingestion`
- section 48.5 `Step 5: Rust adapter`

Also read [replay-semantics.md](./replay-semantics.md), especially:

- replay policy
- patch types
- dependency semantics

---

## 2. Current repo state

The repo currently contains:

- `crates/sdk-rust-tracing`
  - a `SemanticSession`
  - helper methods to record completed spans and artifacts
- `examples/coding-agent`
  - a minimal example
- no dedicated `fixtures/golden-runs` area yet
- no full `tracing_subscriber::Layer`-style integration yet

Important current limitations:

- instrumentation is still closer to explicit helper calls than natural tracing integration
- the example trace is useful but not yet rich enough to anchor the whole product
- other worktrees do not have deterministic fixture data they can rely on for UI, replay, and diff work

You are fixing that.

---

## 3. Your mission

Build the first strong Rust instrumentation path and fixture story.

That means:

1. provide a real `tracing`-aligned adapter or layer
2. keep semantic emission explicit and ergonomic
3. improve the coding-agent example to look like a real run
4. create stable fixtures that encode realistic runs
5. make the emitted graph useful for replay, diff, and UI work

You are not responsible for storage internals.

You are not responsible for the HTTP API.

You are not responsible for the web UI.

You are responsible for realistic capture and deterministic sample data.

---

## 4. Ownership boundaries

### 4.1 Files and areas you may edit

You may edit:

- `crates/sdk-rust-tracing/**`
- `examples/coding-agent/**`
- add new files under `fixtures/golden-runs/**`
- related `Cargo.toml` files
- `README.md` if SDK or example usage updates are worth documenting

### 4.2 Files and areas you should avoid editing

Do not edit:

- `crates/storage/**`
- `crates/collector/**`
- `crates/api/**`
- `crates/replay-engine/**`
- `crates/diff-engine/**`
- `apps/web/**`

Only touch `crates/core-model/**` if an additive shared field or helper is truly required for correct semantic capture.

### 4.3 Boundary reminders

Per the architecture:

- adapters emit canonical model concepts
- collectors ingest and validate
- replay and diff consume what was emitted

Do not make the SDK responsible for storage policy or replay policy interpretation.

The SDK emits facts.

Other layers decide how to persist and reuse them.

---

## 5. Core principles you must preserve

### 5.1 Semantic first

Emit spans that mean something to the user:

- planner step
- tool call
- shell command
- file read
- file write
- model call
- human input

Do not reduce everything to generic log spans.

### 5.2 Explicit dependencies

The architecture depends on more than a control tree.

Where possible, the SDK should help emit:

- control parent relationships
- data dependency edges

Do not assume the replay engine can infer everything later.

### 5.3 Replay metadata matters

Every relevant span should carry:

- replay policy
- executor kind/version where applicable
- input and output fingerprints when available

Without that, branching and invalidation become weak.

### 5.4 Deterministic fixtures matter

Fixtures are not documentation props.

They are correctness anchors.

Make them:

- stable
- human-readable where practical
- representative of real semantic runs

### 5.5 Sink abstraction over hard-coding

The SDK should not marry itself permanently to one transport shape.

It should be able to work against:

- today’s in-process collector path
- a future local transport if another worktree lands it

---

## 6. Detailed deliverables

### 6.1 Real Rust tracing integration

Implement a stronger `tracing`-based path.

That likely means:

- a `Layer`
- or a clearly equivalent integration point

The result should feel like idiomatic Rust instrumentation, not a manually scripted session API only.

### 6.2 Semantic wrappers

Provide ergonomic helpers for common step types:

- planner step
- tool call
- shell command
- file read
- file write
- model call
- human input

These helpers should make it easier to emit the right:

- span kind
- artifact type
- replay policy
- fingerprints
- dependency edges

without every consumer rebuilding that logic.

### 6.3 Sink abstraction

Introduce or improve a sink abstraction so the SDK can target:

- an in-process collector sink now
- a local transport sink later

Do not hard-code one forever.

### 6.4 Better coding-agent example

Upgrade `examples/coding-agent` to generate a more realistic run.

At minimum it should include:

- a planner span
- at least one tool call
- at least one shell or file step
- at least one LLM step
- at least one dependency edge
- one failure path that is branch-worthy

The example should be simple enough to understand but rich enough to drive replay and diff work.

### 6.5 Fixture directory

Create `fixtures/golden-runs/` and populate it with deterministic examples.

A good initial fixture set would include:

- one failed coding-agent run
- one successful branch or paired success case if practical
- one smaller recorded-only run for basic UI/testing

The exact format is up to you, but it should be easy for other worktrees to consume.

### 6.6 Fixture generation or loading

If you generate fixtures from code:

- make generation deterministic
- avoid wall-clock timestamps where possible
- avoid random ids

If you check fixtures in directly:

- make them stable
- document how they were produced

Either is acceptable. Stability matters more than the exact approach.

---

## 7. Explicit non-goals

Do not add these in this worktree:

- storage schema redesign
- blob-store internals
- HTTP API server
- web UI
- complex browser instrumentation
- Python or TS SDKs

Do not turn the SDK into a general observability package.

This SDK exists to feed ReplayKit’s semantic model.

---

## 8. Detailed implementation guidance

### 8.1 Start from the user story

The key story is a local coding agent that:

1. plans
2. runs one or more tools
3. uses those outputs in an LLM call
4. fails
5. becomes branchable later

Your example and fixtures should make that story obvious.

### 8.2 Balance tracing naturalness and semantic explicitness

A pure “record whatever spans happen” approach is not enough.

The adapter should preserve:

- semantic kind
- artifact bindings
- replay-related metadata

It is fine if some helper APIs are still needed alongside `tracing`.

The important thing is that the common path is natural and correct.

### 8.3 Artifact strategy in the SDK

The SDK should help callers emit the right artifact summaries and associations.

It should not own storage policy.

That means:

- callers or wrappers can provide meaningful artifact payloads
- the sink layer handles actual persistence handoff

### 8.4 Dependency emission

The architecture wants explicit data dependencies.

Think carefully about where the SDK should make this easy.

Examples:

- final answer depends on tool-search output
- shell step depends on file-write or planner output

Even a small helper for dependency emission can materially improve downstream correctness.

### 8.5 Fixed ids and deterministic timestamps

For fixtures and examples, determinism matters.

Good patterns:

- fixed seed ids where appropriate
- explicit timestamps in examples
- stable ordering

Avoid hidden time sources unless they are isolated behind a test-friendly interface.

### 8.6 Example realism

A good example is not huge.

It is just rich enough that:

- replay can invalidate meaningful downstream work
- diff can show a real change
- the UI can render interesting structure

Aim for realism, not maximum complexity.

---

## 9. Concrete task breakdown

### Phase 1: inspect the current SDK shape

Before coding, confirm:

- what `SemanticSession` currently does well
- where it stops short of natural tracing integration
- how examples currently emit spans and dependencies

Then decide the smallest strong architecture for:

- tracing integration
- sink abstraction
- semantic helpers
- fixture generation or storage

### Phase 2: build or strengthen tracing integration

Implement:

- a proper integration point with `tracing`
- or a clear layer-like abstraction
- plus enough tests to trust emitted structure

### Phase 3: add semantic helpers

Implement wrappers for common agent operations with the right metadata defaults.

### Phase 4: improve example agent

Build a richer `examples/coding-agent` path that produces a representative run.

### Phase 5: create fixtures

Add deterministic fixtures and document their intended usage in tests or comments.

### Phase 6: harden with tests

Make sure the emitted structure is stable enough for:

- replay
- diff
- UI mock data

---

## 10. Testing requirements

### 10.1 SDK unit tests

Add tests for:

- span emission through the tracing integration
- artifact association
- snapshot association if applicable
- replay policy propagation
- executor metadata propagation

### 10.2 Dependency tests

Add tests for:

- explicit `DataDependsOn` edge emission
- parent-child structure remains correct

### 10.3 Example tests

Add tests for:

- coding-agent example produces the expected high-level run shape
- failure path exists
- branchable span exists

### 10.4 Fixture tests

Add tests for:

- fixture loading or generation stability
- expected run/tree properties
- ids and ordering are deterministic

### 10.5 Consumer usefulness tests

If practical, add one end-to-end test that proves the captured example is usable by downstream consumers:

- replay planning finds the right dirty spans
- diff can compare paired runs or branches

If that starts dragging you into replay-engine ownership, stop and keep the test focused on emitted structure.

---

## 11. Verification commands

At minimum run:

```bash
cargo fmt
cargo test -p replaykit-sdk-rust-tracing
cargo test -p coding-agent
cargo clippy -p replaykit-sdk-rust-tracing -p coding-agent --all-targets -- -D warnings
```

If you add fixture-generation commands or examples, run them and mention the result in the handoff.

---

## 12. What good deliverables look like

A good delivery from this worktree has these properties:

- Rust users can instrument a local coding agent naturally
- the emitted traces carry semantic meaning, not just structural nesting
- dependency edges and replay metadata are present where they matter
- the repo gains stable fixtures that other worktrees can use without guessing
- the example agent tells a credible debugging story

A weak delivery looks like:

- a nicer wrapper around the same helper without stronger semantics
- fixtures that are unstable or toy-like
- no clear path from `tracing` spans to ReplayKit semantics

Do not stop at surface ergonomics.

---

## 13. Known traps to avoid

- Do not make the SDK own persistence details.
- Do not hard-code everything to one collector mode forever.
- Do not emit generic spans with no semantic kind mapping.
- Do not rely on random timestamps or ids in fixtures.
- Do not build a huge example that is hard to understand or test.

---

## 14. Handoff format

At the end, provide:

1. a short summary of what changed
2. the main SDK entry points added or changed
3. the fixture files added and what each represents
4. verification commands run and whether they passed
5. any assumptions the API or web worktrees should know about

Especially call out:

- how to consume the fixtures
- whether the SDK can target multiple sinks now
- what the example agent emits semantically

---

## 15. Final decision rule

When faced with a design choice in this worktree, choose the option that most improves:

- semantic fidelity of captured runs
- ergonomics for Rust agent builders
- determinism and reusability of fixtures
- compatibility with ReplayKit’s replay and diff model

Do not optimize for generic telemetry breadth.

Optimize for believable, reusable agent-debugging data.
