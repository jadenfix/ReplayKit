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

## Hermetic Replay

Forks re-execute only the affected spans, in a private sandbox restored from the
workspace the run recorded — never in your working tree.

- Record with a `workspace_root` (collector `POST /v1/runs`): the collector
  snapshots the workspace before and after every span.
- Patches reach executors: `command_override` (shell/tool command, cwd, env),
  `env_var_override` (env for the span and its descendants),
  `file_content_override`, `tool_output_override`, `prompt_edit`,
  `model_config_edit`, `snapshot_override`. Inputs recorded as `input_bindings`
  templates (e.g. `python3 run_tests.py {{select_tests|words}}`) are re-rendered
  from replayed upstream outputs.
- `replay plan` reports blocked spans with reasons from the same executors that
  run the fork. LLM calls are blocked by default; use
  `--model-mode recorded [--model-fixtures FILE]` to replay recorded responses or
  `--model-mode command --model-command CMD` to plug in a provider (JSON request
  on stdin, response on stdout).

Try it with the scripted agent in `examples/scripted-agent`:

```bash
python3 examples/scripted-agent/agent.py make-repo /tmp/rk-ws
REPLAYKIT_DATA_ROOT=./data cargo run --bin replaykit-collector &
RUN=$(python3 examples/scripted-agent/agent.py run --workspace /tmp/rk-ws)
RK="cargo run -q --bin replaykit -- --storage sqlite --data-root ./data"
$RK replay fork $RUN --span test_validate --patch-type env_var_override --replacement AUTH_POOL_TIMEOUT=0.05
$RK --model-mode command --model-command "python3 examples/scripted-agent/fixer.py" \
  replay fork $RUN --span gen_fix --patch-type prompt_edit --patch my-prompt.txt
```

See [replay-semantics.md](./agent-instructions/replay-semantics.md) for details.

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

# Backend smoke (starts real binaries, checks storage and restart persistence)
bash scripts/smoke-test.sh

# Native full stack: collector + SQLite + API + CLI fork/diff + live browser
# Install the web dependencies and Playwright Chromium first:
# cd apps/web && npm ci && npx playwright install chromium && cd ../..
python3 scripts/native-stack-gate.py
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

The native stack gate uses frontend port 5173 because the API deliberately
allows the documented local development origins. It fails if that port is busy.
`PLAYWRIGHT_CHROMIUM_EXECUTABLE=/path/to/chromium` can select an existing browser.
Backend ports are selected dynamically, and `CARGO_TARGET_DIR` is respected.
The CLI demo fork intentionally retains its recorded failed LLM span; the gate
checks the real tool-output change and cached diff without claiming live-model
execution.
