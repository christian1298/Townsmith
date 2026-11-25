//! Townsmith headless server library.
//!
//! Phase 6 starts with the transport-only protocol in [`protocol`], the first
//! authoritative executor in [`world`], optional single-world persistence in
//! [`save`], and the listen/save loop in [`listen`]. The binary in `main.rs`
//! keeps the existing Phase 0 tick harness.

pub mod chat_smoke;
pub mod listen;
pub mod placement;
pub mod protocol;
pub mod save;
pub mod settlement_smoke;
pub mod vote;
pub mod world;
