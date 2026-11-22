//! Townsmith headless server library.
//!
//! Phase 6 starts with the transport-only protocol in [`protocol`] and the
//! first authoritative executor in [`world`]; the binary in `main.rs` keeps the
//! existing Phase 0 tick harness.

pub mod protocol;
pub mod world;
