//! Engine-agnostic cohort simulation for Townsmith.
//!
//! The 1M+ population target uses cohorts rather than individual agents:
//! each cohort stores a count and aggregate needs, happiness, employment, and
//! income. Individual citizens are materialized near the camera by the shell.

pub mod cohort;
pub mod district;
pub mod sim;

pub use cohort::{Cohort, Need, NEED_COUNT};
pub use district::DistrictServices;
pub use sim::{CohortSimulation, SimError};
