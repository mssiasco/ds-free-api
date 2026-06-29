<p align="center">
  <img src="https://raw.githubusercontent.com/NIyueeE/ds-free-api/main/assets/logo.svg" width="81" height="66">
</p>

<h1 align="center">DS-Free-API</h1>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/github/license/NIyueeE/ds-free-api.svg"></a>
  <img src="https://img.shields.io/github/v/release/NIyueeE/ds-free-api.svg">
  <img src="https://img.shields.io/badge/rust-1.95.0+-93450a.svg">
  <img src="https://github.com/NIyueeE/ds-free-api/actions/workflows/ci.yml/badge.svg">
</p>
<p align="center">
  <img src="https://img.shields.io/github/stars/NIyueeE/ds-free-api.svg">
  <img src="https://img.shields.io/github/forks/NIyueeE/ds-free-api.svg">
  <img src="https://img.shields.io/github/last-commit/NIyueeE/ds-free-api.svg">
</p>

[中文](README.zh.md)

Reverse-proxies the free DeepSeek web chat interface and adapts it into standard OpenAI and Anthropic compatible API protocols (currently supporting chat completions and messages, including streaming responses and tool calls).

## Project Highlights

- **Zero-cost API proxy**: Uses DeepSeek's free web interface, no official API Key needed, provides OpenAI / Anthropic compatible endpoints
- **Dual protocol support**: Simultaneously compatible with OpenAI Chat Completions and Anthropic Messages API, plug-and-play with mainstream clients
- **Tool call ready**: Full OpenAI function calling implementation, tool parsing + three-layer self-repair pipeline (text repair → JSON repair → model fallback), covering 10+ abnormal formats
- **File upload ready**: Supports automatic upload of inline data URL files from OpenAI `file` / `image_url` content parts and Anthropic `image` / `document` content blocks to DeepSeek sessions; HTTP URLs automatically trigger search mode, allowing the model to directly access link content
- **Oversized prompt fallback**: When prompts exceed model limits, automatically uses chunked completion + file upload to bypass
- **Web admin panel**: Built-in visual panel with account pool status, API Key management, request logs, i18n support, theme switching, and config hot-reload out of the box
- **Rust implementation**: Single executable + single TOML config, cross-platform native high performance (web panel compiled-in, ready to use)
- **Multi-account pool**: Least-recently-used rotation (DashMap lock-free reads), supports horizontal scaling for concurrency

## Quick Start

### Binary Usage

1. Download the corresponding platform archive from [releases](https://github.com/NIyueeE/ds-free-api/releases) and extract
2. Copy `config.example.toml` to `config.toml` and fill in accounts (optional, can also be configured in the admin panel after startup)
3. Run `./ds-free-api`
4. Visit `http://127.0.0.1:22217/admin` to set the admin password, then create API Keys and manage accounts in the panel

```bash
./ds-free-api
./ds-free-api -c /path/to/config.toml
RUST_LOG=debug ./ds-free-api
```

> **Concurrency**: The free API has session-level rate limits. This project has built-in rate limit detection + exponential backoff retry for stability.
> Recommended parallelism = number of accounts / 2. Supports starting without config.toml and adding accounts via the admin panel.

### Docker Usage

```bash
docker compose -f docker/docker-compose.yaml up -d
```

Compose configuration at [docker/docker-compose.yaml](./docker/docker-compose.yaml).

Admin panel at `http://localhost:22217/admin`, set the admin password on first visit.
`config/` and `data/` directories are bind-mounted into the container, config changes auto-persist to the host.

### Free Test Accounts

Please register on your own, see [issue #62](https://github.com/NIyueeE/ds-free-api/issues/62) for reference methods.

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET  | `/`   | Redirect to admin panel |
| GET  | `/health` | Health check |
| POST | `/v1/chat/completions` | Chat completion (supports streaming and tool calls) |
| GET  | `/v1/models` | Model list |
| GET  | `/v1/models/{id}` | Model details |
| POST | `/anthropic/v1/messages` | Anthropic Messages (supports streaming and tool calls) |
| GET  | `/anthropic/v1/models` | Model list (Anthropic format) |
| GET  | `/anthropic/v1/models/{id}` | Model details (Anthropic format) |

Admin panel at `/admin`, guides admin password setup on first visit.

## Model Mapping

`model_types` in `config.toml` (default `["default", "expert", "vision"]`) auto-maps:

| OpenAI Model ID     | DeepSeek Type |
| ------------------ | ------------- |
| `deepseek-default` | Fast mode     |
| `deepseek-expert`  | Expert mode   |
| `deepseek-vision`  | Vision mode   |

Optional aliases via `model_aliases` aligned by index with `model_types`, no aliases by default. Empty strings are skipped:

```toml
# model_aliases = ["", "deepseek-v4-pro"]  → deepseek-v4-pro maps to expert (index 1)
model_aliases = []
```
Anthropic compatibility layer uses the same model IDs, invoked via `/anthropic/v1/messages`.

### Capability Toggles

- **Deep thinking**: Enabled by default. To explicitly disable, add `"reasoning_effort": "none"` to the request body.
- **Web search**: Enabled by default (DeepSeek backend injects a stronger system prompt in search mode, improving tool call compliance). To explicitly disable, add `"web_search_options": {"search_context_size": "none"}` to the request body.
- **File upload**: Supports inline files (data URL) auto-uploaded to DeepSeek sessions, and HTTP URLs auto-trigger search mode:

  **OpenAI side:**
  ```json
  {"type": "file", "file": {"file_data": "data:text/plain;base64,...", "filename": "doc.txt"}}
  {"type": "image_url", "image_url": {"url": "data:image/png;base64,..."}}
  {"type": "image_url", "image_url": {"url": "https://example.com/img.jpg"}}
  ```

  **Anthropic side:**
  ```json
  {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "..."}}
  {"type": "document", "source": {"type": "base64", "media_type": "text/plain", "data": "..."}}
  {"type": "image", "source": {"type": "url", "url": "https://example.com/img.jpg"}}
  ```

### Tool Call Tag Hallucination

Built-in fuzzy matching (fullwidth `|`<=>`|`, `▁`<=>`_`), automatically covers most variants. If the model outputs fallback tags with different formats, add them in the control panel, or append to `[ds_core]` in `config.toml`:

```toml
tool_call.extra_starts = ["<|tool_call_begin|>", "<tool_calls>", "<tool_call>"]
tool_call.extra_ends = ["<|tool_call_end|>", "</tool_calls>", "</tool_call>"]
```

## Web Admin Panel

After starting the service, visit `http://127.0.0.1:22217/admin` to access the admin panel:

- **Dashboard**: Request statistics, account pool status overview
- **Account Pool**: View/add/remove accounts, manually re-login Error status accounts
- **API Keys**: Create/delete API Keys, masked display
- **Models**: Available model list and details
- **Config**: Current runtime config (masked)
- **Logs**: Recent request logs and runtime logs

<p align="center">
  <img src="https://raw.githubusercontent.com/NIyueeE/ds-free-api/main/assets/web_p1.png" alt="Admin Panel Dashboard" width="700">
  <br>
  <em>Admin Panel Dashboard</em>
</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/NIyueeE/ds-free-api/main/assets/web_p2.png" alt="Config Page" width="700">
  <br>
  <em>Config Page</em>
</p>

On first visit, guides admin password setup (bcrypt hash storage). After login, issues JWT (24h validity), supports revoking old tokens on password reset.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | `info` | Log level (`trace` / `debug` / `info` / `warn` / `error`) |
| `DS_DATA_DIR` | `.` (current directory) | Data directory, stores `logs/runtime.log` and `stats.json` |
| `DS_CONFIG_PATH` | `./config.toml` | Config file path, lower priority than `-c` argument |

## Security

- **Admin panel**: JWT authentication + bcrypt password hashing + login failure rate limiting (5 failures locks for 5 minutes)
- **API access**: API Key authentication created via admin panel (HashSet O(1) lookup)
- **CORS**: Configurable allowed Origin list, default only `http://localhost:22217`
- **Sensitive info**: Account IDs masked in response headers, request bodies not logged, persistent file permissions 0600

## Development

### Design Philosophy

**One `config.toml` reflects all runtime state**. Admin panel changes to config are immediately persisted to `config.toml` and hot-reloaded into the running service.

**No unnecessary runtime system dependencies introduced**. The project always prioritizes pure Rust or statically linked dependencies (e.g. `rustls` → `wreq` + BoringSSL), ensuring the compiled artifact is a single binary with no external `.so`/`.dll` dependencies, ready to use after download.


### Brief Architecture Diagram:

```mermaid
flowchart TB
    %% ===== Theme definitions =====
    classDef client fill:#eff6ff,stroke:#3b82f6,stroke-width:3px,color:#1d4ed8,rx:14,ry:14
    classDef gateway fill:#fffbeb,stroke:#f59e0b,stroke-width:3px,color:#92400e,rx:12,ry:12
    classDef openai_adapter fill:#f8fafc,stroke:#0a9e7b,stroke-width:2px,color:#334155,rx:10,ry:10
    classDef anthropic_compat fill:#f8fafc,stroke:#d07354,stroke-width:2px,color:#334155,rx:10,ry:10
    classDef ds_core fill:#f8fafc,stroke:#3964fe,stroke-width:2px,color:#1e40af,rx:10,ry:10
    classDef external fill:#fef2f2,stroke:#ef4444,stroke-width:3px,color:#991b1b,rx:6,ry:6

    %% ===== Nodes =====
    Client(["🖥️ Client"]):::client

    subgraph GW ["🌐 HTTP Gateway Layer"]
        Handler(["Routing / Auth / Serialization"]):::gateway
    end

    subgraph PL ["⚙️ Protocol Layer"]
        direction TB

        subgraph AC ["Anthropic Compatibility Layer"]
            A2O["Request Transform<br/>Anthropic → OpenAI"]:::anthropic_compat
            O2A["Response Transform<br/>OpenAI → Anthropic"]:::anthropic_compat
        end

        subgraph OA ["OpenAI Adapter Layer"]
            ReqPipe["Request Pipeline<br/>Validation / Tool Extraction / Prompt Building"]:::openai_adapter
            RespPipe["Response Pipeline<br/>SSE Parsing / Format Conversion / Tool Repair"]:::openai_adapter
        end
    end

    subgraph CL ["🔧 Core Layer (ds_core)"]
        Pool["Account Pool Rotation"]:::ds_core
        PoW["PoW Solving"]:::ds_core
        Session["Session Orchestration<br/>Create/Destroy / History Upload"]:::ds_core
    end

    DeepSeek[("🔴 DeepSeek API")]:::external

    %% ===== Connections =====
    Client -->|"HTTP Request"| Handler

    Handler -->|"OpenAI Request Struct"| ReqPipe
    Handler -->|"Anthropic Request Struct"| A2O
    A2O -->|"OpenAI Request Struct"| ReqPipe

    ReqPipe --> Pool
    Pool --> PoW
    PoW --> Session
    Session -->|"completion endpoint"| DeepSeek

    Session -.->|"DeepSeek SSE Data Stream"| RespPipe
    RespPipe -.->|"OpenAI Response Struct"| Handler
    RespPipe -.->|"OpenAI Response Struct"| O2A
    O2A -.->|"Anthropic Response Struct"| Handler

    %% ===== Subgraph Styles =====
    style GW fill:#fffbeb,stroke:#f59e0b,stroke-width:2px,stroke-dasharray: 5 5
    style PL fill:#fafafa,stroke:#94a3b8,stroke-width:2px
    style AC fill:#fdf0ec,stroke:#d07354,stroke-width:2px
    style OA fill:#e6f7f3,stroke:#0a9e7b,stroke-width:2px
    style CL fill:#eef2ff,stroke:#3964fe,stroke-width:2px,stroke-dasharray: 5 5
```

### Data Pipelines:

#### OpenAI (chat_completions) Processing Pipeline:

```mermaid
flowchart TB
    %% ===== Theme definitions =====
    classDef ds_core fill:#eef2ff,stroke:#3964fe,stroke-width:2.5px,color:#1e40af,rx:10,ry:10
    classDef openai_adapter fill:#e6f7f3,stroke:#0a9e7b,stroke-width:2.5px,color:#065f46,rx:10,ry:10
    classDef step fill:#fffbeb,stroke:#f59e0b,stroke-width:1.5px,color:#334155,rx:6,ry:6

    subgraph RQ ["Request Processing"]
        direction TB
        Q1["ChatCompletionsRequest"]:::openai_adapter
        Q2["Parameter Validation + Defaults"]:::step
        Q3["Tool/File Extraction + Prompt Injection"]:::step
        Q4["DeepSeek Native Tag Prompt Building"]:::step
        Q5["Model Mapping + Capability Toggles"]:::step
        Q6["Rate Limit Retry<br/>Exponential Backoff 1s→2s→4s→8s→16s"]:::step
        Q7["ChatRequest"]:::ds_core
    end

    subgraph RS1 ["Non-Streaming Response"]
        direction TB
        OR1["ds_core SSE Stream"]:::ds_core
        OR2["SSE Frame Parsing<br/>ContentDelta / Usage"]:::step
        OR3["State Machine Reassembly<br/>Merge Consecutive Text / Accumulate Usage"]:::step
        OR4["Chunk Aggregation<br/>Concatenate content / reasoning / tool_calls"]:::step
        OR5["ChatCompletionsResponse"]:::openai_adapter
    end

    subgraph RS2 ["Streaming Response"]
        direction TB
        OS1["ds_core SSE Stream"]:::ds_core
        OS2["SSE Frame Parsing + State Machine"]:::step
        OS3["Chunk Conversion<br/>DsFrame → ChatCompletionsResponseChunk"]:::step
        OS4["Tool Call XML Parsing"]:::step
        OS5["Abnormal Tool Call Self-Repair"]:::step
        OS6["Stop Sequence Detection + Obfuscation"]:::step
        OS7["ChatCompletionsResponseChunk"]:::openai_adapter
    end

    Q1 --> Q2 --> Q3 --> Q4 --> Q5 --> Q6 --> Q7
    OR1 --> OR2 --> OR3 --> OR4 --> OR5
    OS1 --> OS2 --> OS3 --> OS4 --> OS5 --> OS6 --> OS7

    style RQ fill:#f8fafc,stroke:#0a9e7b,stroke-width:2px
    style RS1 fill:#f8fafc,stroke:#0a9e7b,stroke-width:2px
    style RS2 fill:#f8fafc,stroke:#0a9e7b,stroke-width:2px
```

#### Anthropic (messages) Processing Pipeline:

```mermaid
flowchart TB
    %% ===== Theme definitions =====
    classDef oai fill:#e6f7f3,stroke:#0a9e7b,stroke-width:2.5px,color:#065f46,rx:10,ry:10
    classDef anth fill:#fdf0ec,stroke:#d07354,stroke-width:2.5px,color:#7c3a2a,rx:10,ry:10
    classDef step fill:#fffbeb,stroke:#f59e0b,stroke-width:1.5px,color:#334155,rx:6,ry:6

    subgraph RQ ["Request Processing"]
        direction TB
        Q1["MessagesRequest"]:::anth
        Q2["Message Expansion<br/>System Prepend / Text Merge / Image/Document Mapping"]:::step
        Q3["Tool Mapping<br/>ToolUnion → OpenAI Tool"]:::step
        Q4["Capability Toggle Mapping<br/>thinking → reasoning_effort"]:::step
        Q5["ChatCompletionsRequest"]:::oai
    end

    subgraph RS3 ["Non-Streaming Response"]
        direction TB
        AR1["ChatCompletionsResponse"]:::oai
        AR2["Content Split<br/>reasoning → Thinking<br/>content → Text<br/>tool_calls → ToolUse"]:::step
        AR3["ID Mapping<br/>chatcmpl → msg<br/>call → toolu"]:::step
        AR4["MessagesResponse"]:::anth
    end

    subgraph RS4 ["Streaming Response"]
        direction TB
        AS1["ChatCompletionsResponseChunk Stream"]:::oai
        AS2["Chunk State Machine<br/>Block Type Switch / Index Progression"]:::step
        AS3["Event Mapping<br/>content → text_delta<br/>reasoning → thinking_delta<br/>tool_calls → input_json_delta"]:::step
        AS4["MessagesResponseChunk"]:::anth
    end

    Q1 --> Q2 --> Q3 --> Q4 --> Q5
    AR1 --> AR2 --> AR3 --> AR4
    AS1 --> AS2 --> AS3 --> AS4

    style RQ fill:#f8fafc,stroke:#d07354,stroke-width:2px
    style RS3 fill:#f8fafc,stroke:#d07354,stroke-width:2px
    style RS4 fill:#f8fafc,stroke:#d07354,stroke-width:2px
```

Detailed development guide (building, testing, Docker deployment, e2e tests, etc.) at [docs/development.md](./docs/development.md).
## License

[GNU General Public License v3.0](LICENSE)

[DeepSeek Official API](https://platform.deepseek.com/top_up) is very affordable, please support the official service.

The original intention of this project is to experience the latest models being A/B tested on the official web interface.

**Commercial use is strictly prohibited**. Avoid putting pressure on official servers, otherwise you bear the risk.
