# ReplayKit Worktree Instruction: Web Alpha

Status: active implementation brief

Recommended worktree name: `wt/web`

Recommended branch name: `feat/web-local-debugger-alpha`

Primary audience: the agent responsible for replacing the placeholder frontend with the first real local debugger UI

Primary goal: build a local web alpha that makes ReplayKit’s differentiators visible and usable even before every backend endpoint is finalized

Related local documents:

- [architecture.md](./architecture.md)
- [product.md](./product.md)
- [delivery-plan.md](./delivery-plan.md)
- [replay-semantics.md](./replay-semantics.md)

---

## 0. Why this worktree exists

ReplayKit’s architecture is deep, but the product only becomes legible once a user can:

- see runs
- inspect a semantic graph
- select a step
- understand replay policy
- see failure context
- start a branch flow
- compare source and branch behavior

Right now `apps/web/index.html` is a placeholder.

That is useful only as a marker.

It does not validate:

- the frontend information architecture
- the debugger flow
- the usability of the data model
- the clarity of the branch-and-diff wedge

This worktree exists to make the product visible.

---

## 1. Product and architecture context

ReplayKit is not a generic dashboard.

The frontend should feel like a debugger for agent execution.

The architecture highlights four visible differentiators:

- branching
- diffing
- failure forensics
- local-first semantic replay

The first web alpha should make those differentiators obvious even if some backend endpoints are still maturing.

Read these sections of [architecture.md](./architecture.md) before coding:

- section 3 `Differentiators`
- section 5.5 `API service`
- section 5.6 `UI and CLI`
- section 6.4 through 6.7
- section 23 `Diff engine`
- section 24 `Failure forensics engine`
- section 25 `API layer`
- section 27 `UI architecture`
- section 41 `UI data flow`
- section 59 `UX rules implied by architecture`

Also read [product.md](./product.md), especially:

- branching
- semantic replay
- diff
- failure forensics
- local-first positioning

Do not design a frontend that could equally well belong to any generic tracing tool.

---

## 2. Current repo state

The repo currently contains:

- `apps/web/index.html`
  - a placeholder page describing the project
- Rust crates implementing:
  - core model
  - storage
  - collector
  - replay engine
  - diff engine
  - API facade
  - SDK
  - CLI

Important current frontend limitations:

- no run list
- no tree view
- no inspector
- no diff panel
- no branch draft flow
- no transport abstraction for API vs mock data

You are replacing that with the first real UI.

---

## 3. Your mission

Build a local web alpha that expresses ReplayKit’s core debugger loop.

That means:

1. show runs
2. show a semantic tree or timeline
3. show selected span details
4. show artifacts and metadata
5. make branch-from-span visible
6. make diff summary visible
7. make failure navigation visible

You are not building Rust backend code in this worktree.

You are not responsible for the write path.

You are not responsible for SDK instrumentation.

You are responsible for making the product understandable and navigable.

---

## 4. Ownership boundaries

### 4.1 Files and areas you may edit

You may edit:

- `apps/web/**`
- frontend config files under `apps/web`
- sample JSON or mock data files under `apps/web`

### 4.2 Files and areas you should not edit

Do not edit:

- any Rust crate
- root `Cargo.toml`
- `Dockerfile`
- `docker-compose.yml`
- docs outside `apps/web` unless a tiny frontend usage note is absolutely necessary

### 4.3 Boundary reminders

The frontend should consume:

- mock data now
- local HTTP API later

It should not:

- query SQLite
- depend on Rust internals
- require Cargo changes

---

## 5. Core principles you must preserve

### 5.1 Debugger, not dashboard

The UI should center:

- run inspection
- branchability
- diff
- failure explanation

Do not optimize first for charts, vanity metrics, or generic telemetry cards.

### 5.2 Local-first

The app should work locally with mock data and later swap to a local API.

Do not build auth, user accounts, org switching, or cloud-specific assumptions.

### 5.3 Transport abstraction

Design a clean client seam:

- mock provider
- live provider

The mock provider should unblock progress immediately.

### 5.4 Intentional information architecture

Use the architecture’s layout guidance:

- left pane: runs and filters
- center pane: tree or timeline
- right pane: span inspector
- lower or tabbed area: artifacts and diffs

This matters.

### 5.5 Explainability

Replay policy, blocked replay, dirty reasons, and first divergence should be legible.

The UI should teach the model, not hide it.

---

## 6. Detailed deliverables

### 6.1 Real app structure

Replace the placeholder page with a structured app.

You can choose a lightweight stack.

If introducing a heavy frontend toolchain slows progress or complicates repo usage too much, prefer a simpler setup.

A minimal build setup or even a no-build modular frontend is acceptable if it yields a clear, maintainable result.

### 6.2 Client abstraction

Build a frontend data layer with at least two providers:

- mock provider
- live API provider

The live provider can be skeletal if the API is not fully landed yet.

The mock provider must be good enough to drive the full main flow.

### 6.3 Run list view

Implement a run list that shows useful fields such as:

- title
- status
- started time
- duration
- adapter or source type
- failure summary
- branch indicator

Selection should drive the main detail area.

### 6.4 Run detail / tree view

Implement a tree-centric run view.

The tree should surface:

- nesting
- span kind
- status
- timing
- failure state
- selected node
- quick actions where appropriate

If timeline view is easy, include it.

If not, do not block the work. A strong tree view is enough for the first alpha.

### 6.5 Span inspector

Implement an inspector panel that shows:

- span name and kind
- status
- replay policy
- executor metadata if present
- input artifacts
- output artifacts
- dependency references
- snapshots
- error summary

This is one of the most important views in the app.

### 6.6 Artifact viewer shell

Implement artifact tabs or panels for common artifact types:

- text
- JSON
- shell logs
- file diffs
- metadata preview for images or DOM if sample data includes them

It is fine if initial rendering is basic as long as the shape is there.

### 6.7 Branch draft flow

Make branching visible from a selected span.

At minimum support a draft UI that lets the user:

- see that the span is branchable or not
- choose a patch type
- edit a patch value
- preview impact or at least see intended consequences
- submit a branch action or mock it

Even if the live backend is not ready, the interaction model should be real.

### 6.8 Diff summary flow

Implement a diff summary area that can show:

- source run vs branch run
- changed span count
- first divergent span
- final status difference
- output difference summary

If the mock provider is used, build it around realistic sample data.

### 6.9 Failure workflow affordances

Make it easy to:

- jump to first failure
- jump to deepest failing dependency if data exists
- see blocked replay reason

The UI should reveal why a run is interesting, not force manual hunting.

---

## 7. Explicit non-goals

Do not add these in this worktree:

- backend Rust routes
- SQLite access
- blob-store logic
- auth
- dark-mode rabbit holes
- design-system sprawl
- cloud product features

Do not spend the majority of the time polishing visuals while the main debugger flow is missing.

---

## 8. Detailed implementation guidance

### 8.1 Choose a pragmatic frontend stack

The current repo does not yet have a frontend toolchain committed.

You have freedom, but be pragmatic.

Good outcomes:

- a maintainable app structure
- minimal setup friction
- easy local iteration
- mock data support

Bad outcomes:

- lots of build/config churn with little product progress
- framework complexity that exceeds the current UI needs

### 8.2 Build around mock data first

Do not block on the backend API being done.

Use realistic sample JSON and build the client shape around expected route responses.

This de-risks the frontend and makes API contract gaps obvious.

### 8.3 Use the architecture’s view model shape

The UI should think in terms like:

- run list item
- run tree node
- span detail
- artifact preview
- diff summary
- replay job state

Do not tightly couple component state to raw Rust record types.

### 8.4 Visual emphasis

Make these especially legible:

- selected span
- failed path
- blocked replay
- branch action
- first divergence

The user should immediately understand what to click next.

### 8.5 Loading model

Even if the initial app uses mock data, design the client with:

- list loading
- detail loading
- lazy artifact loading

This will make the future live-provider swap cleaner.

### 8.6 Keep the main loop obvious

The UI should make this loop visible:

1. open run
2. inspect failed or interesting span
3. branch from span
4. compare branch to source

If that loop is not clear, the UI is missing the product wedge.

---

## 9. Concrete task breakdown

### Phase 1: choose stack and app structure

Decide:

- whether to keep it very light or introduce a small toolchain
- component organization
- mock data format
- provider abstraction shape

Then scaffold the app structure.

### Phase 2: build mock provider and models

Create:

- sample run list data
- sample run tree data
- sample span detail data
- sample diff data
- sample blocked replay state

Keep the data believable and aligned with the architecture.

### Phase 3: build the main layout

Implement:

- left pane
- center pane
- right pane
- artifact/diff area

### Phase 4: build core interactions

Implement:

- run selection
- tree node selection
- inspector updates
- branch draft interactions
- diff panel rendering

### Phase 5: wire live provider seam

Implement a provider interface and a live provider stub or initial implementation against expected local routes.

### Phase 6: basic tests and polish

Add lightweight tests or deterministic DOM-state checks and make the app feel coherent.

---

## 10. Testing requirements

### 10.1 Rendering tests

Add tests for:

- run list rendering
- tree rendering
- span inspector rendering
- diff summary rendering

### 10.2 Interaction tests

Add tests for:

- selecting a run updates the main panel
- selecting a tree node updates inspector state
- branch draft state behaves correctly
- blocked replay state renders clearly

### 10.3 Provider tests

Add tests for:

- mock provider returns the expected data shapes
- live provider maps or fetches correctly if implemented

### 10.4 Stability checks

If using sample JSON fixtures, make sure they are deterministic and easy to inspect.

If using no-build JS, include at least a small deterministic verification story rather than relying only on manual clicking.

---

## 11. Verification expectations

At minimum, verify:

- the app loads locally
- the mock flow is navigable
- the main debugger loop is visible
- tests for chosen frontend stack pass

Include the exact commands you used in your handoff.

Because the frontend stack choice is open, you should define the verification commands you adopt and then run them.

---

## 12. What good deliverables look like

A good delivery from this worktree has these properties:

- `apps/web` is a real app, not a placeholder
- the layout teaches the ReplayKit mental model
- branch and diff are visible as first-class flows
- failure context is easy to navigate
- the frontend can move from mock data to local API without redesign

A weak delivery looks like:

- a pretty shell with no real debugger flows
- a generic telemetry dashboard aesthetic
- direct dependence on backend internals
- no mock/provider seam

Do not optimize first for visual polish over product legibility.

---

## 13. Known traps to avoid

- Do not wait for the API to be fully finished.
- Do not tie the UI to SQLite or Rust record internals.
- Do not hide replay policy or blocked replay behind obscure menus.
- Do not build a generic dashboard instead of a debugger.
- Do not spend disproportionate time on charts, theming, or animation before the core flows work.

---

## 14. Handoff format

At the end, provide:

1. a short summary of what changed
2. the frontend stack chosen and why
3. the main flows now supported
4. verification commands run and whether they passed
5. any assumptions the API worktree should know about

Especially call out:

- provider interface shape
- sample/mock data files
- expected live route shapes if you encoded them in the client

---

## 15. Final decision rule

When faced with a design choice in this worktree, choose the option that most improves:

- visibility of ReplayKit’s branch-and-diff debugger loop
- clarity of failure and replay semantics
- ability to progress locally without backend blockage
- future swap from mock provider to local API

Do not optimize for generic frontend sophistication.

Optimize for a believable first debugger UI.
