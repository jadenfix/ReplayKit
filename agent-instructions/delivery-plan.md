# Delivery Plan

## Goal

Deliver ReplayKit as a usable local debugger for agent runs with recorded replay, branching, and diffs.

## Workstreams

### Workstream 1: Canonical model

Build:

- stable core types
- schema definitions
- artifact manifests
- replay policies
- patch manifests

Exit criteria:

- fixture data validates against the model
- storage and collector both use the same canonical contracts

### Workstream 2: Storage and collector

Build:

- SQLite schema and migrations
- content-addressed blob store
- local collector daemon
- integrity checks

Exit criteria:

- real and fixture runs ingest cleanly
- collector recovers safely from interruption

### Workstream 3: SDK and adapters

Build:

- Rust `tracing` adapter
- sample coding-agent integration
- generic transport contract

Exit criteria:

- one real local run produces complete graph, artifacts, and snapshots

### Workstream 4: Replay engine

Build:

- recorded replay loader
- branch creation
- invalidation logic
- executor registry
- partial downstream rerun

Exit criteria:

- user can patch a tool output and replay only affected work

### Workstream 5: Diff and forensics

Build:

- run diff summaries
- span-level artifact diffs
- first divergence computation
- deepest failing dependency analysis

Exit criteria:

- user can explain why a branch succeeded where the source failed

### Workstream 6: UI

Build:

- run list
- tree/timeline view
- span inspector
- artifact tabs
- fork and diff workflows

Exit criteria:

- the entire debugger loop is usable without the CLI

## Milestones

### Milestone 0: Design freeze

Deliver:

- architecture docs
- canonical type list
- fixture scenarios

### Milestone 1: Storage core

Deliver:

- DB migrations
- blob store
- repository layer

### Milestone 2: Collector

Deliver:

- ingest API
- batch writer
- recovery logic

### Milestone 3: First adapter

Deliver:

- Rust `tracing` layer
- sample coding-agent run

### Milestone 4: CLI debugger

Deliver:

- list runs
- show run tree
- inspect artifacts
- replay recorded run

### Milestone 5: Web alpha

Deliver:

- run browser
- tree/timeline
- span inspector

### Milestone 6: Fork and replay

Deliver:

- patch manifests
- invalidation engine
- selective rerun

### Milestone 7: Diff and forensics

Deliver:

- run diff
- span diff
- failure slicing

### Milestone 8: Hardening

Deliver:

- export/import
- redaction hooks
- migration and performance tests

## Verification by phase

### Early verification

- schema round-trip tests
- fixture ingestion tests
- artifact integrity tests

### Mid-phase verification

- collector interruption recovery
- replay correctness on golden fixtures
- invalidation correctness on synthetic graphs

### Late verification

- end-to-end branch demo
- large-run performance checks
- imported bundle replay inspection

## Demo criteria

The first strong demo must show:

1. a failed local coding-agent run
2. a selected failed tool or prompt span
3. a branch created from that span
4. a patch applied
5. downstream selective recompute
6. a successful final result
7. a clear diff against the source

## Decision defaults

- local-first before hosted
- coding-agent adapter first
- branch and diff before broad compatibility
- semantic replay before deterministic replay
- web app before desktop packaging
