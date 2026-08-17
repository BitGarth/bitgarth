//! Trezor hardware wallet communication module.
//!
//! This module provides platform-specific implementations for communicating with Trezor devices:
//! - **Web (WASM)**: Uses trezor-connect JavaScript SDK via wasm-bindgen
//! - **Desktop (native)**: Uses Trezor Bridge HTTP API on localhost:21325
//!
//! Both implementations expose the same public API through this module.

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
mod types;

#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(all(not(target_arch = "wasm32"), feature = "desktop"))]
mod bridge;
#[cfg(all(not(target_arch = "wasm32"), feature = "desktop"))]
mod desktop;
#[cfg(all(not(target_arch = "wasm32"), feature = "desktop"))]
pub(crate) mod proto;

// Re-export types (allow unused for cross-platform compatibility)
#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
pub(crate) use types::{TrezorDevice, TrezorError, TrezorErrorKind};

// Re-export platform-specific implementation
#[cfg(target_arch = "wasm32")]
pub(crate) use web::{get_account_pubkeys, get_master_fingerprint, initialize_trezor};

#[cfg(all(not(target_arch = "wasm32"), feature = "desktop"))]
pub(crate) use desktop::{
    enumerate_devices, get_account_pubkeys, get_master_fingerprint, is_bridge_running,
    set_selected_device,
};
