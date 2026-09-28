//! `stream-core`: the cross-platform heart of the screen-sharing system.
//!
//! Contains only platform-agnostic logic: the binary media protocol, the
//! transport abstraction, the streaming loop, diagnostics, and the trait
//! definitions for the per-OS backends. No Windows/Linux-specific code lives
//! here.

pub mod diagnostics;
pub mod platform;
pub mod protocol;
pub mod streaming;
pub mod transport;
