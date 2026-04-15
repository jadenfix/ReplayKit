# ReplayKit

ReplayKit is a local-first replay debugger for AI agent runs.
Record an execution, inspect the span tree, fork from any step,
patch one input or output, replay only affected downstream work,
and diff the result against the original.

## Prerequisites

- **Rust stable** with `rustfmt` and `clippy`
- **Node.js 22** recommended for the web app
- **Docker** (optional, for the containerized stack)

## Quick Start: Local Development

Build the workspace:

```bash
cargo build --workspace
```

Start the **collector** (write endpoint, port 4100):

```bash
REPLAYKIT_DATA_ROOT=./data cargo run --bin replaykit-collector
```

Start the **API server** (read endpoint, port 3210) in a second terminal:

```bash
cargo run --bin replaykit -- --storage sqlite --data-root ./data serve
```

Start the **web app** in a third terminal:

```bash
cd apps/web && npm install --cache .npm-cache && npm run dev
```

Open `http://localhost:5173/?api=http://localhost:3210` to connect the UI to the live API.

### Seed Demo Data

```bash
cargo run --bin replaykit -- --storage sqlite --data-root ./data demo
```

Or seed a run with a branch:

```bash
cargo run --bin replaykit -- --storage sqlite --data-root ./data demo-branch
```

## Docker Quick Start

The compose file starts the collector, API, and web app as separate services sharing a persistent volume:

```bash
docker compose up --build
```

- Collector: `http://localhost:4100`
- API: `http://localhost:3210`
- Web: `http://localhost:5173`

Seed one real run into the running stack:

```bash
bash scripts/seed-stack-run.sh
```

## Storage Layout

Both collector and API share a data root directory:

```
{data-root}/
  replaykit.db              # SQLite database (runs, spans, artifacts, branches)
  blobs/
    .tmp/                   # Atomic write staging
    sha256/
      {aa}/{bb}/{hash}.blob # Content-addressed artifact blobs
```

The collector defaults to `REPLAYKIT_DATA_ROOT=./data`.
The CLI defaults to `--data-root data/replaykit` — pass `--data-root ./data` explicitly to share the same store as the collector.

## Testing

```bash
# Rust
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Web
cd apps/web && npm ci --cache .npm-cache && npm run lint && npm test && npm run build

# Browser smoke prerequisites
cd apps/web && npx playwright install chromium

# Smoke test (starts services, seeds data, verifies end-to-end)
bash scripts/smoke-test.sh
```

## Architecture Documentation

- [architecture.md](./agent-instructions/architecture.md) — system design and subsystem reference
- [product.md](./agent-instructions/product.md) — product thesis and positioning
- [replay-semantics.md](./agent-instructions/replay-semantics.md) — replay, branching, and patch semantics
- [delivery-plan.md](./agent-instructions/delivery-plan.md) — milestones and acceptance criteria

## Project Structure

```
crates/
  core-model/          # Domain types: RunRecord, SpanRecord, Value, etc.
  storage/             # Storage trait, SQLite impl, content-addressed blob store
  collector/           # Write path: ingest runs/spans/artifacts via HTTP (port 4100)
  api/                 # Read path: query runs/spans/diffs/branches via HTTP (port 3210)
  cli/                 # CLI binary: serve, demo, runs, replay commands
  replay-engine/       # Fork, branch, dirty-set computation, replay execution
  diff-engine/         # Run-to-run diff computation
  sdk-rust-tracing/    # Rust tracing SDK for recording agent runs
apps/
  web/                 # React/Vite frontend
examples/
  coding-agent/        # Example agent integration
fixtures/
  golden-runs/         # Test fixtures
```

## License

MIT
