# Development Guide

## Environment Requirements

- Rust **1.95.0+** (see `rust-toolchain.toml`)
- Bun **1.3+** (Web panel build and development)
- `cmake`, `g++`, `libclang-dev` (required to compile `wreq`'s BoringSSL dependency)
- `just` command runner (for `just serve` / `just check` etc. shortcut commands)

## First-time Setup

```bash
# 1. Copy config
cp config.example.toml config.toml

# 2. Build Web frontend (compiled into binary at build time, rebuild required for each frontend change)
cd web && bun install && bun run build && cd ..

# 3. Run development server
just serve
```

After the server starts, visit `http://localhost:22217` which auto-redirects to the admin panel.

> **Frontend hot-reload development**: Run `cd web && bun run dev` (Vite HMR mode)
> alongside `just serve`; the backend prioritizes static files from the `web/dist/` filesystem directory.
> No need to rebuild the binary for every frontend change.

## Release Build

```bash
# 1. Build Web frontend
cd web && bun install && bun run build && cd ..

# 2. Build Release binary
cargo build --release

# 3. Run (can also run the binary directly, no web/dist/ directory needed)
./target/release/ds-free-api
```

Release binary embeds frontend assets at compile time via `rust_embed`. When `web/dist/` directory
doesn't exist, it automatically uses embedded assets. No extra files needed for distribution.

## CI Automated Build

GitHub Actions (`.github/workflows/release.yml`) runs automatically on tag push:

```
build-frontend (bun install --frozen-lockfile + bun run build)
  ├── build-linux-gnu (cargo build)     │
  ├── build-linux-musl (musl-cross)     │── release (tar.gz + zip)
  ├── build-macos (cargo build)  │
  └── build-windows (cargo build)│
  └── docker (ghcr.io image)
```

`build-frontend` produces a `web-dist` artifact. Each build job downloads it before running `cargo build` /
`cross build`, ensuring `rust_embed` embeds actual frontend files.

Docker image auto-pushed to `ghcr.io/niyueee/ds-free-api:latest`.

## Docker Deployment (Production)

Pull from ghcr.io (recommended):

```bash
# Ensure docker/config/ directory exists (auto-created or manual mkdir)
docker compose -f docker/docker-compose.yaml up -d
```

Container auto-creates minimal config on first startup, no need to prepare `config.toml` in advance.
Config and data are persisted to host via bind mount at `docker/config/` and `docker/data/`.

Build local Docker image from source:

```bash
# 1. Build frontend + cross-compile binary
cd web && bun install && bun run build && cd ..
cargo zigbuild --release --target x86_64-unknown-linux-gnu

# 2. Build Docker image
docker build -f docker/Dockerfile -t ds-free-api .

# 3. Export and transfer to server
docker save ds-free-api | gzip > ds-free-api.tar.gz
scp ds-free-api.tar.gz user@server:/tmp/

# 4. Load and start on server
ssh user@server
docker load < /tmp/ds-free-api.tar.gz
docker compose -f docker/docker-compose.yaml up -d
```

> For servers with native x86 environment, you can run the build directly on the server for faster speed.
> Docker image only contains pre-compiled binary + embedded frontend assets, no compilation needed inside the container.

## Command Reference

```bash
# One-pass check (check + clippy + fmt + audit + unused deps)
just check

# Run tests
cargo test --lib

# Run HTTP server
just serve

# Unified protocol debug CLI (built-in chat/compare/concurrent modes)
just adapter-cli

# Start server with e2e config
just e2e-serve
```

## e2e Tests

`py-e2e-tests/` is a JSON scenario-driven end-to-end test framework with no pytest dependency. Three tiers:

| Tier       | Command             | Coverage                                                    |
| ---------- | ------------------- | ----------------------------------------------------------- |
| **Basic**  | `just e2e-basic`    | Basic functionality scenarios (dual-endpoint OpenAI + Anthropic), safe concurrency |
| **Repair** | `just e2e-repair`   | Tool call abnormal format repair tests (OpenAI single endpoint), safe concurrency |
| **Stress** | `just e2e-stress`   | All scenarios × 3 iterations, safe concurrency + 1 concurrency |

Start the server first:

```bash
just e2e-serve
```

Then run e2e tests in another terminal:

```bash
# Basic scenario tests
just e2e-basic

# Tool repair tests
just e2e-repair
```

Scenario files are stored in `scenarios/` organized by type:

```
py-e2e-tests/
├── scenarios/
│   ├── basic/
│   │   ├── openai/         # 7 basic scenarios (chat, reasoning, streaming, tool calls, file upload, image upload, HTTP link)
│   │   └── anthropic/      # 7 basic scenarios (chat, reasoning, streaming, tool calls, document upload, image upload, HTTP link)
│   └── repair/             # 10 tool corruption format scenarios
├── runner.py               # Single run entry point
├── stress_runner.py        # Multi-iteration stress test entry point
└── config.toml             # e2e dedicated server config
```

Each scenario is an independent JSON file containing request parameters and validation rules:

```json
{
  "name": "scenario name",
  "endpoint": "openai|anthropic",
  "category": "basic|repair",
  "models": ["deepseek-default", "deepseek-expert", "deepseek-vision"],
  "messages": [{"role": "user", "content": "..."}],
  "tools": [...],
  "tool_choice": "auto",
  "request": {"stream": false},
  "checks": {
    "has_tool_calls": true,
    "tool_names": ["get_weather"],
    "finish_reason": "tool_calls",
    "no_error": true
  }
}
```

### e2e CLI Parameters

**`just e2e-basic` and `just e2e-repair` (single run):**

| Parameter | Description |
|-----------|-------------|
| `scenario_dir` | Scenario directory, e.g. `scenarios/basic` or `scenarios/repair` |
| `--endpoint` | Endpoint filter: `openai` / `anthropic` |
| `--model` | Model filter: `deepseek-default` / `deepseek-expert` |
| `--filter` | Scenario name keyword filter (space-separated for multiple, e.g. `--filter file image`) |
| `--parallel` | Parallelism, default `account count ÷ 2` |
| `--show-output` | Show model reply summary, tool calls, finish reason |
| `--report` | Output JSON report path |

**`just e2e-stress` (stress test):**

| Parameter | Description |
|-----------|-------------|
| `--iterations` | Iterations per scenario, default 3 |
| `--models` | Model list filter |
| `--filter` | Scenario name keyword filter (space-separated for multiple) |
| `--parallel` | Parallelism, default `account count ÷ 2 + 1` |
| `--show-output` | Show model output |
| `--report` | Output JSON report path |

Usage examples:

```bash
# Quick validation of newly added file upload scenarios
just e2e-basic --filter file image --show-output

# Only view expert model on OpenAI endpoint
just e2e-basic --endpoint openai --model deepseek-expert

# Serial debugging
just e2e-basic --endpoint openai --parallel 1 --show-output

# Stress test: tool call repair scenarios × 5 iterations
just e2e-stress --filter repair --iterations 5

# Output JSON report
just e2e-basic --report result.json
```

## More Documentation

- [Code Style Guide](code-style.md)
- [Logging Specification](logging-spec.md)
- [Prompt Injection Strategy](deepseek-prompt-injection.md)
