//! ACP control projection of genuine native PTY sessions, separate from the byte stream.
pub mod birth;
pub mod wire;

pub mod terminal;

pub mod transport;

pub mod custody;

pub mod ownership;

pub mod bridge;

pub mod helper;

pub mod manual_fence;

pub mod helper_runtime;

pub mod helper_boot;

/// Native returned-row mapping only; source completeness requires a separate verified cut.
pub mod history_codec;

/// Native15 observation framing; not a content admission or provider capability.
pub mod content_chunks;

/// Core-owned managed API adapter; it makes no all-native-entry admission claim.
pub mod managed;
