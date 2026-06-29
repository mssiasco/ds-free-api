# ds_core

DeepSeek API client library. Encapsulates the complete workflow for interacting with the DeepSeek backend: login authentication, account pool management, PoW computation, session management, file upload, SSE stream parsing, and oversize fallback strategies.

## Responsibility Boundaries

ds_core is a **standalone library crate** that does not depend on any types from the main crate:

- **No dependency** on HTTP frameworks (axum/warp etc.)
- **No awareness** of OpenAI / Anthropic protocols
- **No involvement** in API key authentication, request statistics, admin panel
- All configuration injected via `DsCoreConfig` / `AccountConfig`

## Module Architecture

```
ds_core/src/
├── lib.rs           ── Public API re-exports + DsCore/CoreError
├── accounts.rs      ── Accounts facade, integrates client/pool/solver
├── accounts/
│   ├── client.rs    ── DsClient: DeepSeek REST client
│   ├── pool.rs      ── AccountPool: account pool, state management, recovery
│   └── pow.rs       ── PowSolver: WASM PoW solver
├── chat.rs          ── Chat facade, dispatches by prompt size
├── chat/
│   ├── request.rs   ── Three request paths (normal/history file/chunked write)
│   └── response.rs  ── ResponseStream: SSE parsing + streamlined event protocol
└── config.rs        ── DsCoreConfig / AccountConfig
```

## Core Flow

```
Main Crate
  │  DsCore::v0_chat(ChatRequest) → Result<ChatResponse, CoreError>
  ▼
DsCore
  │  Forwards to Chat::v0_chat()
  ▼
Chat
  │  Determines if prompt exceeds limit
  ├── Not oversized → v0_chat_once(): get account → create session → upload files → PoW → completion
  ├── Oversized + default → v0_chat_oversized_file(): split history into file uploads + inline latest turn
  └── Oversized + expert → v0_chat_oversized_chunk(): chunked write to session + normal completion for last chunk
  ▼
ResponseStream
  │  StreamEvent stream (Meta, ThinkStart, ThinkDelta, ContentStart, ContentDelta, Done)
  ▼
Main Crate consumes StreamEvent stream
```

## Core Types

### `DsCore` — Unified Entry Point

```rust
pub struct DsCore { /* accounts + chat */ }

impl DsCore {
    // Create instance (initializes account pool, loads WASM, health check)
    pub async fn new(config: &DsCoreConfig, account_creds: Vec<AccountConfig>)
        -> Result<Self, CoreError>;

    // Initiate conversation, returns streamlined protocol event stream
    pub async fn v0_chat(&self, req: ChatRequest, request_id: &str)
        -> Result<ChatResponse, CoreError>;

    // Query account statuses
    pub fn account_statuses(&self) -> Vec<AccountStatus>;

    // Dynamically add/remove accounts
    pub async fn add_account(&self, creds: &AccountConfig) -> Result<String, PoolError>;
    pub async fn remove_account(&self, email_or_mobile: &str) -> Result<String, PoolError>;

    // Re-login
    pub async fn re_login_single(&self, email_or_mobile: &str) -> Result<(), String>;

    // Graceful shutdown
    pub async fn shutdown(&self);

    // Hot-reload configuration
    pub async fn reload_config(&self, config: &DsCoreConfig) -> Result<(), CoreError>;
}
```

### `ChatRequest` — Conversation Request

```rust
pub struct ChatRequest {
    pub prompt: String,           // Conversation prompt (passed through to DeepSeek backend)
    pub thinking_enabled: bool,   // Whether to enable thinking
    pub search_enabled: bool,     // Whether to enable web search
    pub model_type: String,       // Model type: e.g. "default" / "expert"
    pub files: Vec<FilePayload>,  // Files to upload
}
```

`prompt` uses DeepSeek native tag format:
```
<|User|>Hello<|Assistant|>
```

### `ChatResponse` — Conversation Response

```rust
pub struct ChatResponse {
    pub stream: Pin<Box<dyn Stream<Item = Result<StreamEvent, CoreError>> + Send>>,
}
```

### `StreamEvent` — Streamlined Response Protocol

`StreamEvent` abstracts DeepSeek's complex p/o/v patch protocol into 6 structured events.

| Event | Meaning |
|-------|---------|
| `Meta { account_id }` | Stream start, carries the account ID used |
| `ThinkStart` | Model begins thinking |
| `ThinkDelta { content }` | Incremental thinking content fragment |
| `ContentStart` | Model begins outputting final content |
| `ContentDelta { content }` | Incremental final content fragment |
| `Done { finish_reason, accumulated_token_usage }` | Stream end. `finish_reason = Some("stop")` normal completion, `None` abnormal termination; `accumulated_token_usage` carries cumulative token usage |

**Event sequence guarantee**:
```
Meta → (ThinkStart → ThinkDelta* → ContentStart → ContentDelta* | ContentStart → ContentDelta*) → Done
```
I.e.: `ThinkStart` followed by 0 or more `ThinkDelta`; `ContentStart` followed by 0 or more `ContentDelta`; thinking and content phases appear at most once each.

### `CoreError` — Error Type

```rust
pub enum CoreError {
    Overloaded,                           // No available accounts
    ProofOfWorkFailed(PowError),          // PoW computation failed
    ProviderError(String),                // Provider error (network, business, etc.)
    Stream(String),                       // Stream processing error
}
```

### `AccountStatus` — Account State

```rust
pub struct AccountStatus {
    pub email: String,
    pub mobile: String,
    pub state: String,         // "idle" / "busy" / "error" / "invalid"
    pub last_released_ms: i64,
    pub error_count: u8,
}
```

## Configuration Types

```rust
pub struct DsCoreConfig {
    pub api_base: String,              // DeepSeek API base URL
    pub wasm_url: String,              // PoW WASM file URL
    pub user_agent: String,            // Browser UA
    pub client_version: String,        // X-Client-Version
    pub client_platform: String,       // X-Client-Platform
    pub client_locale: String,         // X-Client-Locale
    pub proxy_url: Option<String>,     // Proxy URL (non-US IP to bypass WAF)
    pub model_types: Vec<String>,      // Model type list
    pub input_character_limits: Vec<u32>, // Character limit per model type
}

pub struct AccountConfig {
    pub email: String,
    pub mobile: String,
    pub area_code: String,
    pub password: String,
}
```

## Account Pool Model

- 1 account = 1 concurrency. Multiple concurrent requests need multiple accounts.
- `AccountGuard` auto-releases the account on `Drop`, ensuring no leaks.
- Account initialization: login → create_session → health_check → update_title
- Failed accounts are marked as `Invalid` (still kept in pool for frontend display).
- Background recovery task scans `Error` accounts every 60 seconds and attempts re-login.
- 3 consecutive login failures marks account as `Invalid`.

## Request Dispatch Strategy

Chat module routes based on whether prompt character count exceeds 75% of the model limit:

| Condition | Path | Description |
|-----------|------|-------------|
| Prompt not oversized | `v0_chat_once` | Direct completion |
| Oversized + `model_type=expert` | `v0_chat_oversized_chunk` | Chunked write to session, normal completion for last chunk |
| Oversized + other model_type | `v0_chat_oversized_file` | Split history into file uploads |

**History splitting logic**: Find all conversation before the last `<|Assistant|>` block, wrap in `[file content end]` ... `[file content begin]` format and upload as txt file; last assistant block + latest user turn sent inline.

## File Upload

- Main crate passes file data via `FilePayload`.
- Upload process: `upload_file(multipart)` → `fetch_files` poll until SUCCESS (up to 30 times, 2s interval).
- Also requires computing PoW targeting `/api/v0/file/upload_file`.

## PoW Flow

1. `create_pow_challenge(target_path)` obtains challenge / salt / difficulty
2. `PowSolver::solve(&challenge)` WASM computes the answer
3. `PowResult::to_header()` → base64 encoding
4. Placed in `X-Ds-Pow-Response` header

## Error Handling Chain

```
DsClient  HTTP layer errors ─┐
          Business errors ───┤
          JSON errors ───────┤
          WAF interception ──┤
                            ▼
                     ClientError
                        │
                        ▼
                    PoolError        ── Account pool error
                    PowError         ── PoW computation error
                        │
                        ▼
                    CoreError        ── Unified error type
                        │
                        ▼
                Main crate translates by protocol
```
