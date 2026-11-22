//! Engine-agnostic cohort simulation for Townsmith.
//!
//! The 1M+ population target uses cohorts rather than individual agents:
//! each cohort stores a count and aggregate needs, happiness, employment, and
//! income. Individual citizens are materialized near the camera by the shell
//! and updated by the [`active`] module at 10 Hz.

pub mod active;
pub mod cohort;
pub mod district;
pub mod sim;
pub mod summary;

pub use active::{
    ActiveAgent, ActiveAgentSim, NeighborStats, Pcg32, SpatialHash, ARRIVAL_RADIUS,
    DEFAULT_AGENT_COUNT, DEFAULT_WORLD_SIZE, MAX_NEIGHBORS, MIN_WORLD_SIZE, SEPARATION_RADIUS,
    TICK_DT,
};
pub use cohort::{Cohort, Need, NEED_COUNT};
pub use district::DistrictServices;
pub use sim::{CatchUpReport, CatchUpState, CohortSimulation, SimError, MAX_CATCH_UP_DAYS};
pub use summary::{
    DistrictCohortSummary, NeedBands, NEED_CONTENT_THRESHOLD, NEED_CRITICAL_THRESHOLD,
};
