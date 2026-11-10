//! Shared contracts of the voice engine: the wire protocol, the discovery
//! file, and the on-disk layout. Every other crate and every client depends on
//! these types; change them only together with `docs/protocol.md` and
//! `packages/voice-client/src/protocol.ts`.

pub mod catalog;
pub mod discovery;
pub mod paths;
pub mod protocol;

pub use protocol::PROTOCOL_VERSION;
