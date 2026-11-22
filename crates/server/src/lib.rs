//! Townsmith headless server library.
//!
//! Phase 6 starts with the transport-only protocol in [`protocol`], the first
//! authoritative executor in [`world`], optional single-world persistence in
//! [`save`], and the listen/save loop in [`listen`]. The binary in `main.rs`
//! keeps the existing Phase 0 tick harness.

pub mod listen;
pub mod protocol;
pub mod save;
pub mod world;
