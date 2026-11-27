//! Townsmith headless server library.
//!
//! Phase 6 starts with the transport-only protocol in [`protocol`], the first
//! authoritative executor in [`world`], optional single-world persistence in
//! [`save`], and the listen/save loop in [`listen`]. The binary in `main.rs`
//! keeps the existing Phase 0 tick harness.

#[cfg(test)]
mod chat_faults;
pub mod chat_smoke;
pub mod listen;
pub mod load_smoke;
pub mod placement;
#[cfg(test)]
mod player_faults;
pub mod protocol;
pub mod save;
#[cfg(test)]
mod save_crash;
#[cfg(test)]
mod save_store_matrix;
#[cfg(test)]
mod settlement_faults;
#[cfg(test)]
mod settlement_paging_faults;
pub mod settlement_smoke;
pub mod vote;
pub mod world;
