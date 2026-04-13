# ReplayKit Product Thesis

## One-line description

ReplayKit is a local-first semantic replay debugger for agents.

## Core user promise

Given any agent run, ReplayKit should let the user answer:

- what happened
- why it happened
- what changed
- what would happen if one step were different

## Product wedge

ReplayKit should not compete primarily on generic tracing.
The sharp wedge is:

- local-first workflow
- semantic replay
- branch from any step
- diff branches against source
- failure forensics for agent runs

## The closest analogies

- Chrome DevTools for agent execution
- Git branches for runs
- LangSmith-like observability with a debugger-grade replay loop

## What it is not

- not a workflow orchestrator
- not a queueing system
- not a deterministic kernel-level recorder
- not a generic cloud observability backend

## Primary users

### Agent framework builders

These users need:

- a reusable recording contract
- replay-safe semantics
- branch and diff support for debugging runtime behavior
- a way to compare framework changes across runs

### Product teams building agents

These users need:

- a local tool they can use during development
- visibility into prompts, tools, files, and outputs
- a way to validate fixes without rerunning everything

### Browser and workflow automation builders

These users need:

- the same replay model
- more artifact-heavy runs
- semantic browser actions rather than raw video capture

## Differentiators

### 1. Branching is first-class

A branch is not just a retry.
A branch is a new run with explicit lineage, a patch manifest, recomputation metadata, and a diff against its source.

### 2. Replay is semantic

ReplayKit records enough structure to replay steps at operation boundaries.
It does not try to emulate the entire process.
That keeps the system portable and product-focused.

### 3. Diff is part of the debugger

The user should be able to identify:

- first divergent span
- changed artifacts
- changed output
- changed failure mode
- changed timing and cost

### 4. Failure forensics is built into the graph

The system should help users answer:

- which upstream step actually mattered
- which retries were noise
- which dependency caused the failure
- whether a new branch fixed the root cause or only masked it

### 5. Local-first matters

The initial product experience should work without:

- remote collector infrastructure
- shared managed storage
- cloud credentials
- organization-level setup

## Product boundaries

### In scope for the initial system

- recording semantic traces
- storing artifacts and snapshots locally
- recorded replay
- forked replay for supported step kinds
- branch diffing
- failure-path analysis
- import and export

### Explicitly out of scope for the initial system

- kernel-level record and replay
- whole-process determinism
- cross-machine distributed consistency
- production workflow orchestration
- hosted team collaboration as the primary experience

## Why this should win

Most tools stop at observability.
ReplayKit should turn observability into an interactive debugger.

Most workflow engines optimize for reliability of execution.
ReplayKit should optimize for understanding and editing execution.

Most replay systems are too low-level for agent developers.
ReplayKit should operate at the semantic level agent developers care about:

- prompts
- model calls
- tool calls
- retrieval
- file effects
- browser actions
- human input

## High-level release logic

### V1

- local coding agents
- generic adapter contract
- recorded replay
- branch from prompt or tool output
- run diff

### V2

- deterministic tool mocks
- stronger browser support
- better search across failures
- richer state snapshots

### V3

- partial live re-execution
- breakpoints
- optional team sharing and remote ingest

## Product litmus tests

ReplayKit is on track if a user can say:

- "I know which step caused this failure."
- "I changed one tool output and replayed only what depended on it."
- "I can show exactly why the new branch succeeded."
- "I did all of that locally."
