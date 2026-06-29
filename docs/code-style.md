# Code Style Guide

## Comment Style

### Module Documentation (//!)
- First line: module responsibility — specific description
- After blank line: key design decisions or constraints

```rust
//! Account pool management — multi-account load balancing
//!
//! 1 account = 1 session = 1 concurrency
```

### Public API Documentation (///)
- Use verb-led phrasing: "returns", "creates", "sends"
- Note side effects: "auto-releases", "cleans up session"
- Document Panic conditions (if any)

```rust
/// Round-robin to get an idle account
///
/// The returned AccountGuard auto-releases the busy flag on Drop
pub fn get_account(&self) -> Option<AccountGuard>
```

### Inline Comments (//)
- Explain "why" rather than "what"
- Mark temporary solutions or external dependencies

```rust
// Order matters: health_check must be before update_title,
// otherwise an empty session causes EMPTY_CHAT_SESSION error
```

## Naming Conventions

| Type | Style | Example |
|------|-------|---------|
| Module/File | snake_case | `ds_core`, `accounts.rs` |
| Type/Struct | PascalCase | `AccountPool`, `CoreError` |
| Function/Method | snake_case | `get_account()`, `compute_pow()` |
| Constant | SCREAMING_SNAKE_CASE | `ENDPOINT_USERS_LOGIN` |
| Enum Variant | PascalCase | `AllAccountsFailed` |

## Error Messages

- **User-facing**: Configuration validation, account management errors use clear descriptive messages
- **Internal**: Library errors (`ds_core`, `client`, `adapter`, `anthropic_compat`) use English for developer debugging
- Include context: "Account {} initialization failed"
- Avoid leaking sensitive info (tokens print only first 8 characters)
- Server layer's `ServerError::Display` preserves adapter original message when presenting errors to API clients

## Enum Variant Naming

- All enum variants use PascalCase (e.g. `AllAccountsFailed`, `BadRequest`)
- Only use non-PascalCase via `#[serde(rename = "...")]` for serde serialization

## Logging Specification

See `docs/logging-spec.md`

## Import Grouping

1. Standard library (`std::`)
2. Third-party crates (`tokio::`, `wreq::`)
3. Internal modules (`crate::`)
4. Local use (super, self)

Groups separated by blank lines.

## Test Code Guidelines

- `println!` is allowed inside test functions for intermediate output, useful for observing parse content on failure
- Library code (`src/` non-`#[cfg(test)]` areas) still prohibits direct use of `println!` / `eprintln!`
