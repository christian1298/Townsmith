//! Household occupancy and simple needs.
//!
//! Phase 3.1 treated population as completed housing capacity. This module
//! adds an independent runtime: completed residential buildings start empty,
//! households move in over time, jobs are assigned, and food/water/rest decay
//! slowly. It is deliberately aggregate: one household record per occupied
//! building, not one record per citizen yet.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Current household save format.
pub const POPULATION_FORMAT_VERSION: u16 = 1;

/// Upper bound for a move-in rate, to keep a malformed save harmless.
pub const MAX_MOVE_IN_PER_SECOND: f32 = 100.0;

/// Upper bound for one simulation step.
pub const MAX_TICK_SECONDS: f64 = 3600.0;

/// Maximum households in one save.
pub const MAX_HOUSEHOLDS: usize = 100_000;

/// Food satisfaction lost per simulated second.
const FOOD_DECAY_PER_S: f32 = 0.0004;
/// Water satisfaction lost per simulated second.
const WATER_DECAY_PER_S: f32 = 0.0006;
/// Rest satisfaction lost per simulated second.
const REST_DECAY_PER_S: f32 = 0.0002;

/// One occupied residential building.
///
/// `placement_id` is the map key in [`CityRuntime`]; the fields are aggregate
/// values for the household living there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Household {
    /// Housing capacity in residents.
    pub capacity: u32,
    /// Residents currently living here.
    pub residents: u32,
    /// Residents assigned to a job in the last tick.
    pub workers: u32,
    /// Food satisfaction, 0..=1.
    pub food: f32,
    /// Water satisfaction, 0..=1.
    pub water: f32,
    /// Rest satisfaction, 0..=1.
    pub rest: f32,
}

impl Household {
    fn new(capacity: u32) -> Self {
        Self {
            capacity,
            residents: 0,
            workers: 0,
            food: 1.0,
            water: 1.0,
            rest: 1.0,
        }
    }

    fn clamp_to_capacity(&mut self) {
        self.capacity = self.capacity.min(1_000_000);
        self.residents = self.residents.min(self.capacity);
        self.workers = self.workers.min(self.residents);
    }

    /// Average need satisfaction.
    #[must_use]
    pub fn needs(&self) -> f32 {
        (self.food + self.water + self.rest) / 3.0
    }

    /// Household happiness, 0..=1. Zero when vacant.
    #[must_use]
    pub fn happiness(&self) -> f32 {
        if self.residents == 0 {
            return 0.0;
        }
        let employment = self.workers as f32 / self.residents as f32;
        (0.6 * self.needs() + 0.4 * employment).clamp(0.0, 1.0)
    }
}

/// Deterministic household runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CityRuntime {
    /// Save format version.
    pub format_version: u16,
    /// World seed this runtime belongs to.
    pub seed: u64,
    /// Residents admitted per simulated second across the city.
    pub move_in_per_second: f32,
    /// Fractional move-in left after the last tick.
    #[serde(default)]
    pub move_in_carry: f32,
    /// Households keyed by placement id.
    pub households: BTreeMap<u64, Household>,
}

impl CityRuntime {
    /// Create an empty runtime for a seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            format_version: POPULATION_FORMAT_VERSION,
            seed,
            move_in_per_second: 1.0,
            move_in_carry: 0.0,
            households: BTreeMap::new(),
        }
    }

    /// Set the move-in rate. Invalid input is clamped to 0..=MAX.
    pub fn set_move_in_per_second(&mut self, rate: f32) {
        self.move_in_per_second = if rate.is_finite() {
            rate.clamp(0.0, MAX_MOVE_IN_PER_SECOND)
        } else {
            0.0
        };
    }

    /// Remove a household by placement id.
    pub fn remove(&mut self, placement_id: u64) {
        self.households.remove(&placement_id);
    }

    /// Remove every household.
    pub fn clear(&mut self) {
        self.households.clear();
        self.move_in_carry = 0.0;
    }

    /// Synchronize households with completed residential placements.
    ///
    /// `slots` is `(placement_id, housing_capacity)` for completed buildings
    /// with capacity greater than zero. Missing households start empty; records
    /// for removed buildings are dropped; capacity changes clamp residents.
    pub fn sync(&mut self, slots: &[(u64, u32)]) {
        let wanted: BTreeMap<u64, u32> = slots
            .iter()
            .copied()
            .filter(|(_, capacity)| *capacity > 0)
            .collect();
        self.households.retain(|id, _| wanted.contains_key(id));
        for (&id, &capacity) in &wanted {
            match self.households.get_mut(&id) {
                Some(household) => {
                    household.capacity = capacity;
                    household.clamp_to_capacity();
                }
                None => {
                    self.households.insert(id, Household::new(capacity));
                }
            }
        }
    }

    /// Advance move-in, jobs, and needs by `dt_s` seconds.
    pub fn tick(&mut self, dt_s: f64, jobs: u64, slots: &[(u64, u32)]) -> PopulationReport {
        self.sync(slots);
        let dt = if dt_s.is_finite() {
            dt_s.clamp(0.0, MAX_TICK_SECONDS) as f32
        } else {
            0.0
        };

        let vacancies: u64 = self
            .households
            .values()
            .map(|household| u64::from(household.capacity - household.residents))
            .sum();
        let desired = self.move_in_per_second * dt + self.move_in_carry;
        let target = desired.floor().max(0.0) as u32;
        let mut to_move = u64::from(target).min(vacancies) as u32;
        let moved_in = to_move;
        for household in self.households.values_mut() {
            if to_move == 0 {
                break;
            }
            let room = household.capacity - household.residents;
            let add = room.min(to_move);
            household.residents += add;
            to_move -= add;
        }
        self.move_in_carry = (desired - target as f32).clamp(0.0, 0.999_999);

        let mut remaining_jobs = jobs;
        for household in self.households.values_mut() {
            let workers = u64::from(household.residents).min(remaining_jobs);
            household.workers = u32::try_from(workers).unwrap_or(u32::MAX);
            remaining_jobs -= workers;
        }

        for household in self.households.values_mut() {
            household.food = (household.food - FOOD_DECAY_PER_S * dt).max(0.0);
            household.water = (household.water - WATER_DECAY_PER_S * dt).max(0.0);
            household.rest = (household.rest - REST_DECAY_PER_S * dt).max(0.0);
            household.clamp_to_capacity();
        }

        PopulationReport {
            moved_in,
            population: self.population(),
            vacancies: self.vacancies(),
            workers: self.workers(),
        }
    }

    /// Total residents.
    #[must_use]
    pub fn population(&self) -> u64 {
        self.households
            .values()
            .map(|household| u64::from(household.residents))
            .sum()
    }

    /// Total housing capacity.
    #[must_use]
    pub fn housing_capacity(&self) -> u64 {
        self.households
            .values()
            .map(|household| u64::from(household.capacity))
            .sum()
    }

    /// Total vacant housing slots.
    #[must_use]
    pub fn vacancies(&self) -> u64 {
        self.housing_capacity().saturating_sub(self.population())
    }

    /// Total employed residents.
    #[must_use]
    pub fn workers(&self) -> u64 {
        self.households
            .values()
            .map(|household| u64::from(household.workers))
            .sum()
    }

    /// Resident-weighted average need satisfaction.
    #[must_use]
    pub fn average_needs(&self) -> (f32, f32, f32) {
        let population = self.population();
        if population == 0 {
            return (0.0, 0.0, 0.0);
        }
        let mut food = 0.0;
        let mut water = 0.0;
        let mut rest = 0.0;
        for household in self.households.values() {
            let weight = household.residents as f32 / population as f32;
            food += household.food * weight;
            water += household.water * weight;
            rest += household.rest * weight;
        }
        (food, water, rest)
    }

    /// Resident-weighted average happiness.
    #[must_use]
    pub fn happiness(&self) -> f32 {
        let population = self.population();
        if population == 0 {
            return 0.0;
        }
        self.households
            .values()
            .map(|household| household.happiness() * household.residents as f32)
            .sum::<f32>()
            / population as f32
    }

    /// Serialize as pretty JSON.
    pub fn to_json_pretty(&self) -> Result<String, PopulationError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse and validate a household save.
    pub fn from_json(json: &str) -> Result<Self, PopulationError> {
        let runtime: Self = serde_json::from_str(json)?;
        runtime.validate()?;
        Ok(runtime)
    }

    /// Validate all ranges and state invariants.
    pub fn validate(&self) -> Result<(), PopulationError> {
        if self.format_version != POPULATION_FORMAT_VERSION {
            return Err(PopulationError::UnsupportedVersion(self.format_version));
        }
        if !self.move_in_per_second.is_finite()
            || self.move_in_per_second < 0.0
            || self.move_in_per_second > MAX_MOVE_IN_PER_SECOND
        {
            return Err(PopulationError::InvalidRuntime(
                "move-in rate is out of range",
            ));
        }
        if !self.move_in_carry.is_finite() || !(0.0..1.0).contains(&self.move_in_carry) {
            return Err(PopulationError::InvalidRuntime(
                "move-in carry is out of range",
            ));
        }
        if self.households.len() > MAX_HOUSEHOLDS {
            return Err(PopulationError::TooManyHouseholds(self.households.len()));
        }
        for (&id, household) in &self.households {
            if id > i64::MAX as u64 {
                return Err(PopulationError::InvalidRuntime(
                    "household id is above i64::MAX",
                ));
            }
            if household.capacity == 0 || household.capacity > 1_000_000 {
                return Err(PopulationError::InvalidRuntime(
                    "household capacity is out of range",
                ));
            }
            if household.residents > household.capacity {
                return Err(PopulationError::InvalidRuntime(
                    "household residents exceed capacity",
                ));
            }
            if household.workers > household.residents {
                return Err(PopulationError::InvalidRuntime(
                    "household workers exceed residents",
                ));
            }
            for value in [household.food, household.water, household.rest] {
                if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                    return Err(PopulationError::InvalidRuntime(
                        "household need is out of range",
                    ));
                }
            }
        }
        Ok(())
    }

    /// FNV-1a checksum over the compact JSON.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        let json = serde_json::to_vec(self).expect("CityRuntime always serializes");
        let mut hash = OFFSET;
        for byte in json {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(PRIME);
        }
        hash
    }
}

/// Result of one population tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PopulationReport {
    /// Residents admitted this tick.
    pub moved_in: u32,
    /// Total residents after the tick.
    pub population: u64,
    /// Vacant housing slots after the tick.
    pub vacancies: u64,
    /// Employed residents after the tick.
    pub workers: u64,
}

/// Errors returned by the household runtime.
#[derive(Debug)]
pub enum PopulationError {
    /// JSON parse/serialize error.
    Json(serde_json::Error),
    /// Unsupported save version.
    UnsupportedVersion(u16),
    /// A range or state invariant failed.
    InvalidRuntime(&'static str),
    /// Too many households for one save.
    TooManyHouseholds(usize),
}

impl fmt::Display for PopulationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "population JSON error: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported population version {version}")
            }
            Self::InvalidRuntime(message) => write!(formatter, "invalid population: {message}"),
            Self::TooManyHouseholds(count) => {
                write!(
                    formatter,
                    "population has {count} households; limit is {MAX_HOUSEHOLDS}"
                )
            }
        }
    }
}

impl std::error::Error for PopulationError {}

impl From<serde_json::Error> for PopulationError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_runtime_is_empty() {
        let runtime = CityRuntime::new(2026);
        assert_eq!(runtime.population(), 0);
        assert_eq!(runtime.housing_capacity(), 0);
        assert_eq!(runtime.happiness(), 0.0);
    }

    #[test]
    fn move_in_fills_vacancies_over_time() {
        let mut runtime = CityRuntime::new(2026);
        runtime.set_move_in_per_second(1.0);
        let slots = [(1, 4)];
        runtime.tick(1.0, 4, &slots);
        assert_eq!(runtime.population(), 1);
        runtime.tick(1.0, 4, &slots);
        assert_eq!(runtime.population(), 2);
        runtime.tick(10.0, 4, &slots);
        assert_eq!(runtime.population(), 4);
        assert_eq!(runtime.vacancies(), 0);
    }

    #[test]
    fn workers_are_capped_by_jobs() {
        let mut runtime = CityRuntime::new(2026);
        runtime.sync(&[(1, 4)]);
        runtime.tick(10.0, 2, &[(1, 4)]);
        assert_eq!(runtime.population(), 4);
        assert_eq!(runtime.workers(), 2);
    }

    #[test]
    fn needs_decay_and_reduce_happiness() {
        let mut runtime = CityRuntime::new(2026);
        runtime.sync(&[(1, 2)]);
        runtime.tick(10.0, 2, &[(1, 2)]);
        let (food, water, rest) = runtime.average_needs();
        assert!(food < 1.0 && water < 1.0 && rest < 1.0);
        assert!(runtime.happiness() > 0.5);
        runtime.tick(1000.0, 2, &[(1, 2)]);
        assert!(runtime.happiness() < 0.9);
    }

    #[test]
    fn sync_removes_orphans_and_clamps_capacity() {
        let mut runtime = CityRuntime::new(2026);
        runtime.sync(&[(1, 4), (2, 2)]);
        runtime.tick(10.0, 6, &[(1, 4), (2, 2)]);
        runtime.sync(&[(1, 2)]);
        assert_eq!(runtime.households.len(), 1);
        assert_eq!(runtime.housing_capacity(), 2);
        assert_eq!(runtime.population(), 2);
    }

    #[test]
    fn json_round_trip_and_checksum() {
        let mut runtime = CityRuntime::new(2026);
        runtime.tick(2.0, 2, &[(1, 2)]);
        let json = runtime.to_json_pretty().expect("json");
        let decoded = CityRuntime::from_json(&json).expect("decode");
        assert_eq!(decoded, runtime);
        assert_eq!(decoded.checksum(), runtime.checksum());
    }

    #[test]
    fn invalid_runtime_is_rejected() {
        let mut runtime = CityRuntime::new(2026);
        runtime.move_in_per_second = f32::NAN;
        assert!(runtime.validate().is_err());
        let mut runtime = CityRuntime::new(2026);
        runtime.households.insert(
            1,
            Household {
                capacity: 2,
                residents: 3,
                workers: 0,
                food: 1.0,
                water: 1.0,
                rest: 1.0,
            },
        );
        assert!(runtime.validate().is_err());
    }

    #[test]
    fn remove_and_clear_are_deterministic() {
        let mut runtime = CityRuntime::new(2026);
        runtime.sync(&[(1, 2), (2, 2)]);
        runtime.remove(1);
        assert_eq!(runtime.households.len(), 1);
        runtime.clear();
        assert!(runtime.households.is_empty());
    }
}
