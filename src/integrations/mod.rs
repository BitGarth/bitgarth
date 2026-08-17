//! Isolated HTTP integration modules.
//!
//! Each integration lives in its own submodule and follows these rules:
//!
//! # Module structure
//!
//! ```text
//! src/integrations/<name>/
//!   mod.rs    — re-exports, integration-specific constants
//!   client.rs — HTTP client accepting TracedBlockingClient
//!   types.rs  — request/response types (serde Deserialize)
//!   error.rs  — error enum with Display, Error, is_rate_limited()
//! ```
//!
//! # What integrations must expose
//!
//! - A client struct with methods that return `Result<T, <Name>Error>`.
//! - An error type implementing `Display`, `Error`, and `is_rate_limited()`.
//! - Response types needed by the app layer.
//!
//! # What integrations must NOT depend on
//!
//! - App-layer types (`UserId`, `Network`, `EthAddress`, etc.)
//! - Logging (`dioxus::logger::tracing`)
//! - Global state or database access
//!
//! Integrations accept traced HTTP clients (`TracedBlockingClient` for
//! blocking, `TracedAsyncClient` for async) plus primitive parameters
//! (`&str`, `u64`, `&Url`). The app
//! layer is responsible for:
//!
//! - Constructing the HTTP client with appropriate timeouts
//! - Mapping app types to integration parameters (e.g., `Network` →
//!   `EtherscanNetwork`, `EthAddress` → `&str`)
//! - Owning sync-specific pagination, raw-ingestion coordination, and
//!   normalization in `src/tasks/jobs/sync/integrations/*`
//! - Converting integration errors to app-layer error types
//! - Logging failures at the appropriate level

#[cfg(feature = "server")]
pub(crate) mod coingecko;
#[cfg(feature = "server")]
pub(crate) mod etherscan;
#[cfg(feature = "server")]
pub(crate) mod mempool;
