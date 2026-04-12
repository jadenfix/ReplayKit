# ReplayKit

ReplayKit is a local-first semantic replay debugger for agents.

The project is intentionally documented from the top down before implementation so the system can be built with a stable mental model and a clear product wedge.

## Documents

- [architecture.md](./architecture.md) is the main system design document. It is the deepest technical reference and explains how every subsystem fits together.
- [product.md](./product.md) defines the product thesis, positioning, user value, and differentiators.
- [replay-semantics.md](./replay-semantics.md) focuses on replay behavior, branching, invalidation, and patch semantics.
- [delivery-plan.md](./delivery-plan.md) breaks the work into milestones, streams, acceptance criteria, and verification stages.

## Product Summary

ReplayKit should let a user:

1. record a local agent run
2. inspect the execution graph
3. replay the run from recorded artifacts
4. fork the run from any supported step
5. patch one input, output, or environment value
6. re-execute only affected downstream work
7. diff the new result against the original

The product is not trying to replace workflow orchestration systems or deterministic process replay systems.
It is trying to become the best local debugger for agent behavior.

## Current Runtime

The repo now has two storage backends:

- `InMemoryStorage` for fast tests and demos
- `SqliteStorage` for durable local metadata persistence

The CLI chooses the backend through environment variables:

- `REPLAYKIT_STORAGE=memory` uses the in-memory backend
- `REPLAYKIT_STORAGE=sqlite` uses SQLite
- `REPLAYKIT_DB_PATH=/path/to/replaykit.db` sets the SQLite file location

Example:

```bash
REPLAYKIT_STORAGE=sqlite \
REPLAYKIT_DB_PATH=./data/replaykit.db \
cargo run -p replaykit-cli -- demo-branch
```

## Docker

The first Dockerized persistence setup uses SQLite on a Docker volume rather than a networked database.
That keeps the backend aligned with the local-first architecture while giving the project a real durable store.

Run the demo against the Docker volume:

```bash
docker compose run --rm replaykit
```
