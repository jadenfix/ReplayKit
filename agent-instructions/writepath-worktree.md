# ReplayKit Worktree Instruction: Write Path

Status: active implementation brief

Recommended worktree name: `wt/writepath`

Recommended branch name: `feat/writepath-collector-blobstore`

Primary audience: the agent responsible for the durable local write path, blob ownership, ingestion transport, and recovery behavior

Primary goal: turn the current storage and collector layer into a real local persistence system that owns artifact bytes, enforces integrity, and exposes a local ingestion service

Related local documents:

- [architecture.md](./architecture.md)
- [replay-semantics.md](./replay-semantics.md)
- [delivery-plan.md](./delivery-plan.md)
- [product.md](./product.md)

---

## 0. Why this worktree exists

ReplayKit’s product wedge depends on trustworthy stored runs.

If the write path is weak, everything above it becomes suspect:

- replay fidelity is suspect
- diff correctness is suspect
- failure forensics is suspect
- import/export is suspect
- the UI becomes a prettier view over unreliable evidence

This worktree exists to make the write path real.

The current repo already has:

- a canonical model in `crates/core-model`
- two storage backends in `crates/storage`
- an in-process collector in `crates/collector`
- replay and diff engines
- an in-process API facade
- a demo CLI

What it does not yet have in a complete product-grade form is:

- a ReplayKit-owned managed blob store
- a local ingestion transport for write operations
- collector recovery and startup repair behavior aligned with the architecture
- a robust end-to-end write path where artifact bytes and metadata are persisted coherently

This worktree closes that gap.

---

## 1. Product and architecture context

ReplayKit is a local-first semantic replay debugger for agents.

It is not:

- a workflow orchestrator
- a distributed job system
- a remote-first SaaS control plane
- a raw tracing database
- a deterministic process recorder

The write path should reflect that.

The write path is not being built to serve arbitrary telemetry ingestion at scale.

It is being built to support:

- local coding agents
- generic SDK-integrated agents later
- browser-heavy agents later

with a strong guarantee that a captured run is usable for:

- inspection
- recorded replay
- branching
- selective invalidation
- diffing
- export/import

Read these sections of [architecture.md](./architecture.md) before coding:

- section 4.5 `Artifacts out of line`
- section 5.2 `Collector daemon`
- section 5.3 `Storage engine`
- section 13 `Collector architecture`
- section 15 `Artifact subsystem`
- section 31 `Reliability and data integrity`
- section 39 `Storage service internals`
- section 48.2 `Step 2: storage and blob store`
- section 48.3 `Step 3: collector`

Do not reinterpret those sections loosely. Build to them.

---

## 2. Current repo state

The repository currently looks like this at a subsystem level:

- `crates/core-model`
  - canonical types, ids, enums, summaries, replay-related domain records
- `crates/storage`
  - `InMemoryStorage`
  - `SqliteStorage`
  - metadata persistence
  - integrity checks on artifact metadata and local blob references
- `crates/collector`
  - in-process write orchestrator
  - validation of parents, snapshots, artifacts, edges, end-span rules
- `crates/replay-engine`
  - branch planning and execution
- `crates/diff-engine`
  - run diffs
- `crates/api`
  - in-process service facade
- `crates/sdk-rust-tracing`
  - currently more helper-style than full `tracing` layer
- `crates/cli`
  - demo commands
- `apps/web`
  - placeholder UI

Important current write-path facts:

- metadata can persist in SQLite already
- artifact records store `blob_path`, `sha256`, `byte_len`, `summary`, and redaction metadata
- the system validates artifact metadata and local file references
- the system does not yet fully own the artifact bytes lifecycle as a first-class subsystem
- the collector is a library, not yet a real local service

That means the architecture is partially embodied but not yet complete.

---

## 3. Your mission

Implement the first robust, local, durable write path for ReplayKit.

That means:

1. ReplayKit must own artifact bytes in a managed blob store.
2. Artifact persistence must become a coherent metadata-plus-blob operation.
3. The collector must expose a real local ingestion interface.
4. Interrupted writes and interrupted runs must be recoverable and explicit.
5. Integrity failures must be observable and test-covered.

Your job is not to build the query API or the frontend.

Your job is not to redesign replay.

Your job is to make the evidence layer trustworthy.

---

## 4. Ownership boundaries

### 4.1 Files and areas you may edit

You may edit:

- `crates/storage/**`
- `crates/collector/**`
- `Cargo.toml` files needed for those crates
- `README.md` if storage or collector usage changes need documenting
- `Dockerfile` and `docker-compose.yml` only if a small change is needed for the local data path

You may make a minimal additive change in:

- `crates/core-model/src/lib.rs`

only if you truly need a shared type to support the blob store or ingestion protocol.

### 4.2 Files and areas you should not edit

Do not edit:

- `crates/api/**`
- `crates/replay-engine/**`
- `crates/diff-engine/**`
- `crates/sdk-rust-tracing/**`
- `crates/cli/**` unless a tiny compatibility update is strictly required
- `apps/web/**`
- `examples/**`

### 4.3 Architectural ownership reminders

Per [architecture.md](./architecture.md):

- `core-model` owns shared types
- `storage` owns repositories and persistence internals
- `collector` owns ingestion validation and write orchestration

Do not let `collector` become a second storage layer and do not let `storage` absorb collector policy.

---

## 5. Core principles you must preserve

### 5.1 Local-first

Do not introduce:

- remote DB dependencies
- cloud object storage
- account setup
- auth
- remote coordination

This system should still work for a single developer on one machine.

### 5.2 Canonical model first

Do not create private duplicate enums or records for runs, spans, artifacts, or edges.

If a new shared concept is needed, add it to `core-model` once.

### 5.3 Artifacts out of line

Large payloads belong in the blob store.

Metadata belongs in SQLite.

Do not collapse them into one JSON row or one giant blob table.

### 5.4 Safe degradation

If integrity is compromised, fail explicitly.

Do not silently drop bad artifacts or auto-heal in ways that hide data loss.

### 5.5 Recovery over cleverness

Prefer simple and inspectable recovery rules over ambitious background repair logic.

---

## 6. Detailed deliverables

You are expected to produce all of the following unless something in the existing code makes one item impossible without excessive cross-worktree coupling.

### 6.1 Managed blob store

Implement a ReplayKit-owned blob store with these properties:

- rooted under a local data directory
- content-addressed by `sha256`
- stable directory fanout, consistent with the architecture
- supports deduplication
- supports read-back by artifact record
- supports integrity verification

Use a layout compatible with the architecture’s expectation:

```text
data/
  replaykit.db
  blobs/
    sha256/
      aa/
        bb/
          aabbcc...blob
```

It does not have to be named exactly this if the implementation already has a better local root abstraction, but the semantics should match.

### 6.2 Blob write lifecycle

Implement blob writes with a safe pattern:

- receive bytes or a payload source
- hash content
- write to a temp file
- fsync or otherwise flush safely if practical
- atomically rename into final location
- then commit DB metadata

If DB commit fails after blob write:

- leave the blob in place if it is content-addressed and reusable
- avoid orphan semantics that create ambiguity
- document the chosen policy in code comments where appropriate

### 6.3 Artifact persistence orchestration

Today artifact records and blob bytes are not yet a fully unified subsystem.

Change that.

The collector should be able to accept a write that results in:

- stored bytes in the blob store
- a canonical artifact row in SQLite
- a stable `artifact_id`
- valid `sha256`
- valid `byte_len`
- stored `summary`
- stored redaction metadata
- correct `run_id` and optional `span_id`

### 6.4 Local ingestion transport

Implement a real local write transport.

Acceptable choices:

- local HTTP JSON
- local HTTP plus binary payload endpoint
- Unix socket with JSON messages

Choose the boring path that gets to a stable contract fastest.

The transport should cover write operations only:

- `begin_run`
- `start_span`
- `add_event`
- `add_artifact`
- `add_snapshot`
- `add_edge`
- `end_span`
- `finish_run`
- `abort_run`

If you need a small binary or server entry point, keep it collector-owned.

### 6.5 Recovery scan

On startup, the collector or storage service should be able to:

- identify unfinished runs
- mark interrupted runs appropriately
- surface partially written or invalid artifacts
- preserve inspectable evidence rather than deleting aggressively

Recovery should be simple, explicit, and test-covered.

### 6.6 Integrity tooling

The system should have a clean internal notion of integrity checks.

At minimum support:

- artifact row exists
- blob exists
- blob is a regular file
- size matches
- content hash matches when integrity checking is enabled

If full hash verification is expensive, make the behavior configurable or explicitly scoped, but support it.

### 6.7 Internal diagnostics

Add enough diagnostics to debug the write path:

- collector startup failures
- storage open failures
- blob write failures
- recovery actions
- integrity failures

Do not build a full metrics product. Just make debugging the subsystem possible.

---

## 7. Explicit non-goals

Do not add these in this worktree:

- remote sync
- team sharing
- cloud buckets
- retention/garbage collection policy
- UI query endpoints
- web frontend work
- replay algorithm redesign
- executor registry redesign
- import/export bundle format redesign unless absolutely required for blob ownership

If a tempting improvement does not directly strengthen the current write path, skip it.

---

## 8. Detailed implementation guidance

### 8.1 Storage abstraction strategy

Do not smash everything into the existing `Storage` trait without thought.

You need to preserve two truths:

1. The repo already uses `Storage` as the metadata abstraction.
2. Blob ownership is a sibling concern, not a random helper.

A good direction is:

- keep metadata operations in `Storage`
- add a blob-store abstraction owned by `storage`
- compose them in `SqliteStorage` and any collector-facing write path

Possible shapes:

- `BlobStore` trait plus concrete local implementation
- `Storage` extension methods for artifact payload writes if composition stays clean

The important thing is the boundary, not the exact trait name.

### 8.2 In-memory backend behavior

Do not abandon `InMemoryStorage`.

It is useful for:

- fast unit tests
- isolated collector tests
- replay and diff tests in other crates

You may either:

- keep it metadata-only and clearly scoped
- or teach it to emulate blob ownership in memory

But it must continue to work cleanly for tests.

### 8.3 SQLite behavior

Preserve the existing durability posture and extend it carefully:

- WAL mode
- sane busy timeout
- migration-safe startup
- integrity-preserving inserts

Do not create a design where the SQLite record can claim a blob exists before it actually does.

### 8.4 Artifact summaries

The architecture expects light previews in metadata.

Do not attempt to infer complex previews in this worktree if that drags you into UI work.

It is enough to preserve and validate:

- summary JSON or document
- redaction metadata

### 8.5 Multipart or binary handling

If you choose HTTP:

- you may use JSON for metadata and a second upload route for bytes
- or a simple multipart approach

Choose simplicity and testability over clever protocol design.

### 8.6 Atomicity stance

You are not required to deliver distributed transactions across DB plus filesystem.

You are required to deliver a coherent local policy.

Good enough v1 policy:

- blob write to temp
- final blob rename
- DB insert
- on DB failure, surface error and leave content-addressed blob as reusable orphan-safe data

If you choose a stronger policy, keep it simple.

### 8.7 Recovery stance

If the collector crashes mid-run:

- preserve the run
- mark it interrupted on recovery
- do not silently finish it

If an artifact row references a missing blob:

- surface an integrity error
- do not quietly return a partial artifact

---

## 9. Concrete task breakdown

### Phase 1: inspect and model the current gap

Before coding, confirm:

- how artifacts are inserted now
- where `blob_path` is currently trusted as caller input
- how `SqliteStorage` lays out metadata rows
- how collector write methods currently construct artifact records

Then decide the minimal internal architecture for:

- blob root resolution
- content-addressed write path
- artifact metadata persistence
- transport layer

### Phase 2: implement blob store

Build:

- root config
- content hash path derivation
- temp file strategy
- write path
- read path
- integrity verification helpers

### Phase 3: integrate collector artifact ingestion

Build:

- an input type that can carry artifact content
- or a transport contract that can feed content into collector-side blob writes
- persistence orchestration that yields canonical artifact rows

### Phase 4: add local ingestion transport

Build:

- a small server
- route handlers
- request and response types
- typed errors

The write transport should be narrow and boring.

### Phase 5: add startup recovery and integrity scan

Build:

- interrupted run detection
- missing blob detection
- inconsistent artifact handling

### Phase 6: docs and operational clarity

Update:

- `README.md` with local write-path usage if needed
- comments only where the write semantics are non-obvious

---

## 10. Testing requirements

You are expected to add serious tests, not just happy-path smoke tests.

### 10.1 Blob store tests

Add tests for:

- blob write/read round-trip
- deduplication for identical content
- fanout path derivation
- size mismatch detection
- hash mismatch detection
- temp-file failure behavior if practical

### 10.2 Storage integration tests

Add tests for:

- SQLite artifact row plus blob write coherence
- artifact lookup after persisted write
- integrity error when blob is deleted
- integrity error when blob contents are mutated
- in-memory backend behavior remains valid for existing tests

### 10.3 Collector tests

Add tests for:

- ingestion of runs, spans, and artifacts through the write transport
- artifact attached to span with persisted bytes
- interrupted run recovery
- duplicate content behavior across two artifacts
- malformed write requests produce explicit errors

### 10.4 Recovery tests

Add tests for:

- startup marks active runs interrupted
- missing blob row is surfaced as integrity failure
- partial artifact state does not masquerade as healthy

### 10.5 Concurrency tests

If practical, add tests for:

- two collector instances writing to the same storage root
- simultaneous artifact writes of identical content

If this is too deep for the current abstraction, document the limit in the handoff.

---

## 11. Verification commands

At minimum run:

```bash
cargo fmt
cargo test -p replaykit-storage
cargo test -p replaykit-collector
cargo clippy -p replaykit-storage -p replaykit-collector --all-targets -- -D warnings
```

If you add a runnable local collector binary or example, run it and include the verification result in your handoff.

---

## 12. What good deliverables look like

A good delivery from this worktree has these properties:

- another engineer can point the system at a local data root and trust that artifact bytes are owned by ReplayKit
- artifact metadata and bytes stay coherent
- ingestion can happen through a real local service rather than only a library call
- interrupted state is preserved and surfaced
- integrity failures are explicit and diagnosable
- tests make it hard to regress the write path silently

A weak delivery looks like:

- only adding more metadata checks
- pushing blob responsibility back onto callers
- adding a transport without real recovery semantics
- widening traits without improving actual behavior

Do not stop at scaffolding.

---

## 13. Known traps to avoid

- Do not make `blob_path` a loosely trusted caller field forever.
- Do not implement blob persistence in the API crate.
- Do not let the collector depend on UI-facing types.
- Do not create a complicated async service if a small sync local server is enough.
- Do not make test coverage so integration-heavy that fast unit tests disappear.
- Do not silently repair bad rows in ways that erase debugging evidence.

---

## 14. Handoff format

At the end, provide:

1. a short summary of what changed
2. the main files touched
3. verification commands run and whether they passed
4. any constraints or follow-on work the next worktree should know about

Especially call out:

- any new local data-root assumptions
- any new collector binary or server entry point
- any additive shared types introduced

---

## 15. Final decision rule

When faced with a design choice in this worktree, choose the option that most improves:

- local durability
- explicit integrity
- recovery clarity
- compatibility with the existing canonical model

Do not optimize for future distributed scale.

Do not optimize for elegance at the expense of trustworthy local evidence.
