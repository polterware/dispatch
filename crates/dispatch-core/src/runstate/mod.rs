//! The core's run directory: core.json, the lock, and the build handshake.
//!
//! Ported from `dispatch/backend/platform/runstate`.

mod build;
mod state;

pub use build::*;
pub use state::*;
