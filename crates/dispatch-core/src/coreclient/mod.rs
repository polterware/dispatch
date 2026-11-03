//! Discovering and dialing a running core.
//!
//! Ported from `dispatch/backend/platform/coreclient`.

mod lifecycle;
pub(crate) mod rpc;
mod services;

pub use crate::runstate::{build_id, executable, newer_build};
pub use lifecycle::{connect, ensure, ensure_with_exe, new, start, start_with_timeout, stop};
pub use rpc::{
    CLIENT_HEADER, ConnectError, Envelope, EnvelopeDecoder, decode_stream_frame, encode_envelope,
    http_status_to_connect_code,
};
pub use services::{Clients, DispatchClient, HealthClient, WatchRunStream, WorkspaceClient};
