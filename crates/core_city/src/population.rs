//! Household occupancy and simple needs.
//!
//! Phase 3.1 treated population as completed housing capacity. This module
//! adds an independent runtime: completed residential buildings start empty,
//! households move in over time, jobs are assigned, and food/water/rest decay
//! slowly. It is deliberately aggregate: one household record per occupied
//! building, not one record per citizen yet.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::traffic::{travel_time_seconds, FlowField, RoadGraph, DEFAULT_TRAVEL_SPEED_MPS};

/// Current household save format.
pub const POPULATION_FORMAT_VERSION: u16 = 1;

/// Upper bound for a move-in rate, to keep a malformed save harmless.
pub const MAX_MOVE_IN_PER_SECOND: f32 = 100.0;

/// Upper bound for one simulation step.
pub const MAX_TICK_SECONDS: f64 = 3600.0;

/// Abstract wage per employed resident per game day.
pub const WAGE_PER_WORKER_PER_DAY: f64 = 20.0;

/// Commute distance at which the happiness commute term reaches zero.
pub const MAX_COMMUTE_M: f32 = 200.0;

/// Maximum households in one save.
pub const MAX_HOUSEHOLDS: usize = 100_000;

/// Food satisfaction lost per simulated second.
const FOOD_DECAY_PER_S: f32 = 0.0004;
/// Water satisfaction lost per simulated second.
const WATER_DECAY_PER_S: f32 = 0.0006;
/// Rest satisfaction lost per simulated second.
const REST_DECAY_PER_S: f32 = 0.0002;

/// Food satisfaction restored per simulated second at full supply.
pub const FOOD_REPLENISH_PER_S: f32 = 0.002;

/// Water satisfaction restored per simulated second at full service.
pub const WATER_REPLENISH_PER_S: f32 = 0.002;

/// Rest satisfaction restored per simulated second at full power service.
pub const REST_REPLENISH_PER_S: f32 = 0.0018;

/// Largest per-workplace staffing value accepted from a save.
const MAX_STAFFING_PER_WORKPLACE: u32 = 1_000_000;

/// Whether a derived float is still at its old-save default.
fn is_zero_f32(value: &f32) -> bool {
    *value == 0.0
}

/// Below this many household/workplace pairs, the plain global scan is
/// cheaper than building a spatial index.
const SMALL_JOB_PAIRS: usize = 4_096;

/// Both sides need at least this many entries before the index pays for
/// itself.
const MIN_GRID_SIDE: usize = 8;

#[cfg(test)]
std::thread_local! {
    static LAST_JOB_PATH_WAS_GRID: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn mark_job_path(grid: bool) {
    LAST_JOB_PATH_WAS_GRID.with(|cell| cell.set(grid));
}

#[cfg(test)]
fn last_job_path_was_grid() -> bool {
    LAST_JOB_PATH_WAS_GRID.with(std::cell::Cell::get)
}

/// One completed residential building available to households.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Home {
    /// Placement id.
    pub id: u64,
    /// Housing capacity in residents.
    pub capacity: u32,
    /// World position in metres.
    pub center: [f32; 2],
}

/// One completed workplace with open job slots.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Workplace {
    /// Placement id.
    pub id: u64,
    /// Job slots at this workplace.
    pub slots: u32,
    /// World position in metres.
    pub center: [f32; 2],
}

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
    /// Average commute to assigned workplaces in metres.
    #[serde(default)]
    pub commute_m: f32,
    /// Average commute time in seconds at [`DEFAULT_TRAVEL_SPEED_MPS`].
    ///
    /// Derived from `commute_m` after each job assignment; it is never an
    /// allocation input. Older saves default to zero and are refreshed on the
    /// next tick. It is omitted from JSON while zero so pre-travel-time saves
    /// round-trip byte-for-byte.
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub commute_time_s: f32,
    /// Gross wage income per game day.
    #[serde(default)]
    pub income_per_day: f64,
    /// Home position in metres; refreshed from the placement on tick.
    #[serde(default)]
    pub home: [f32; 2],
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
            commute_m: 0.0,
            commute_time_s: 0.0,
            income_per_day: 0.0,
            home: [0.0, 0.0],
        }
    }

    fn clamp_to_capacity(&mut self) {
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
        let commute = if self.workers == 0 {
            0.0
        } else {
            1.0 - (self.commute_m / MAX_COMMUTE_M).clamp(0.0, 1.0)
        };
        (0.5 * self.needs() + 0.3 * employment + 0.2 * commute).clamp(0.0, 1.0)
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
    /// Workers assigned to each workplace in the last tick, keyed by
    /// placement id. Derived state, saved so industry production resumes
    /// immediately after loading.
    #[serde(default)]
    pub staffing: BTreeMap<u64, u32>,
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
            staffing: BTreeMap::new(),
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
        self.staffing.remove(&placement_id);
    }

    /// Remove every household.
    pub fn clear(&mut self) {
        self.households.clear();
        self.staffing.clear();
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

    /// Synchronize households and record each home's world position.
    pub fn sync_homes(&mut self, homes: &[Home]) {
        let slots: Vec<(u64, u32)> = homes.iter().map(|home| (home.id, home.capacity)).collect();
        self.sync(&slots);
        for home in homes {
            if let Some(household) = self.households.get_mut(&home.id) {
                household.home = home.center;
            }
        }
    }

    /// Advance move-in, jobs, and needs by `dt_s` seconds.
    ///
    /// Compatibility wrapper for callers without workplace positions; all
    /// jobs are treated as one workplace at the origin.
    pub fn tick(&mut self, dt_s: f64, jobs: u64, slots: &[(u64, u32)]) -> PopulationReport {
        let homes: Vec<Home> = slots
            .iter()
            .map(|(id, capacity)| Home {
                id: *id,
                capacity: *capacity,
                center: [0.0, 0.0],
            })
            .collect();
        let workplaces = if jobs == 0 {
            Vec::new()
        } else {
            vec![Workplace {
                id: 0,
                slots: u32::try_from(jobs).unwrap_or(u32::MAX),
                center: [0.0, 0.0],
            }]
        };
        self.tick_with_workplaces(dt_s, &homes, &workplaces)
    }

    /// Advance move-in, workplace assignment, and needs by `dt_s` seconds.
    ///
    /// Compatibility wrapper with no food supply; needs keep decaying.
    pub fn tick_with_workplaces(
        &mut self,
        dt_s: f64,
        homes: &[Home],
        workplaces: &[Workplace],
    ) -> PopulationReport {
        self.tick_with_workplaces_and_supply(dt_s, homes, workplaces, 0.0)
    }

    /// Advance move-in, workplace assignment, and needs by `dt_s` seconds.
    ///
    /// `food_supply` is the share of resident food demand met this step,
    /// 0..=1. Water and rest still decay; use [`Self::tick_with_supplies`] for
    /// utility service.
    pub fn tick_with_workplaces_and_supply(
        &mut self,
        dt_s: f64,
        homes: &[Home],
        workplaces: &[Workplace],
        food_supply: f32,
    ) -> PopulationReport {
        self.tick_with_supplies(dt_s, homes, workplaces, food_supply, 0.0, 0.0)
    }

    /// Advance move-in, workplace assignment, and needs with service shares.
    ///
    /// `food_supply`, `water_supply`, and `power_supply` are 0..=1 service
    /// shares. Power supports rest at home (lighting, heating, and comfort) as
    /// well as industry production, so a power outage reduces both.
    pub fn tick_with_supplies(
        &mut self,
        dt_s: f64,
        homes: &[Home],
        workplaces: &[Workplace],
        food_supply: f32,
        water_supply: f32,
        power_supply: f32,
    ) -> PopulationReport {
        self.tick_with_supplies_and_roads(
            dt_s,
            homes,
            workplaces,
            food_supply,
            water_supply,
            power_supply,
            None,
        )
    }

    /// Advance with service shares and an optional road graph.
    ///
    /// When a graph is supplied, commute distances follow the road network;
    /// otherwise they are straight-line metres. Unreachable pairs fall back to
    /// straight-line distance so a disconnected network cannot strand jobs.
    #[allow(clippy::too_many_arguments)]
    pub fn tick_with_supplies_and_roads(
        &mut self,
        dt_s: f64,
        homes: &[Home],
        workplaces: &[Workplace],
        food_supply: f32,
        water_supply: f32,
        power_supply: f32,
        roads: Option<&RoadGraph>,
    ) -> PopulationReport {
        self.sync_homes(homes);
        let food = normalize_supply(food_supply);
        let water = normalize_supply(water_supply);
        let power = normalize_supply(power_supply);
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

        let flows = roads
            .map(|graph| {
                workplaces
                    .iter()
                    .map(|work| graph.flow_to(work.center))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let assignment = assign_jobs(&self.households, workplaces, roads, &flows);
        self.staffing = assignment.staffing;
        let mut assigned = 0_usize;
        for (&id, household) in &mut self.households {
            let (workers, weighted) = if household.residents > 0 {
                debug_assert_eq!(
                    assignment.home_ids.get(assigned).copied(),
                    Some(id),
                    "job assignment must follow household id order"
                );
                let values = (
                    assignment.workers[assigned],
                    assignment.commute_weighted[assigned],
                );
                assigned += 1;
                values
            } else {
                (0, 0.0)
            };
            household.workers = workers;
            household.commute_m = if workers > 0 {
                weighted / workers as f32
            } else {
                0.0
            };
            household.commute_time_s =
                travel_time_seconds(household.commute_m, DEFAULT_TRAVEL_SPEED_MPS).unwrap_or(0.0);
            household.income_per_day = f64::from(workers) * WAGE_PER_WORKER_PER_DAY;
        }

        for household in self.households.values_mut() {
            household.food = (household.food - FOOD_DECAY_PER_S * dt
                + FOOD_REPLENISH_PER_S * food * dt)
                .clamp(0.0, 1.0);
            household.water = (household.water - WATER_DECAY_PER_S * dt
                + WATER_REPLENISH_PER_S * water * dt)
                .clamp(0.0, 1.0);
            household.rest = (household.rest - REST_DECAY_PER_S * dt
                + REST_REPLENISH_PER_S * power * dt)
                .clamp(0.0, 1.0);
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

    /// Gross wages paid per game day.
    #[must_use]
    pub fn daily_wages(&self) -> f64 {
        self.households
            .values()
            .map(|household| household.income_per_day)
            .sum()
    }

    /// Resident-weighted average commute distance in metres.
    #[must_use]
    pub fn average_commute_m(&self) -> f32 {
        let population = self.population();
        if population == 0 {
            return 0.0;
        }
        self.households
            .values()
            .map(|household| household.commute_m * household.residents as f32)
            .sum::<f32>()
            / population as f32
    }

    /// Resident-weighted average commute time in seconds.
    #[must_use]
    pub fn average_commute_time_s(&self) -> f32 {
        let population = self.population();
        if population == 0 {
            return 0.0;
        }
        self.households
            .values()
            .map(|household| household.commute_time_s * household.residents as f32)
            .sum::<f32>()
            / population as f32
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
            if !household.commute_m.is_finite() || household.commute_m < 0.0 {
                return Err(PopulationError::InvalidRuntime(
                    "household commute is out of range",
                ));
            }
            if !household.commute_time_s.is_finite() || household.commute_time_s < 0.0 {
                return Err(PopulationError::InvalidRuntime(
                    "household commute time is out of range",
                ));
            }
            if !household.income_per_day.is_finite() || household.income_per_day < 0.0 {
                return Err(PopulationError::InvalidRuntime(
                    "household income is out of range",
                ));
            }
            if household.home.iter().any(|value| !value.is_finite()) {
                return Err(PopulationError::InvalidRuntime(
                    "household home is not finite",
                ));
            }
        }
        for (&workplace_id, &staffed) in &self.staffing {
            if workplace_id > i64::MAX as u64 || staffed > MAX_STAFFING_PER_WORKPLACE {
                return Err(PopulationError::InvalidRuntime(
                    "workplace staffing is out of range",
                ));
            }
        }
        Ok(())
    }

    /// FNV-1a checksum over the compact JSON.
    ///
    /// `commute_time_s` is a derived display value; it is zeroed before
    /// serialization so adding travel time does not change the pinned
    /// population checksum.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        let mut snapshot = self.clone();
        for household in snapshot.households.values_mut() {
            household.commute_time_s = 0.0;
        }
        let json = serde_json::to_vec(&snapshot).expect("CityRuntime always serializes");
        let mut hash = OFFSET;
        for byte in json {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(PRIME);
        }
        hash
    }
}

/// One household considered for job assignment.
struct JobHome {
    id: u64,
    center: [f32; 2],
    residents: u32,
}

/// Result of one tick's job assignment.
struct JobAssignment {
    /// Household ids in the same order as `workers` and `commute_weighted`.
    home_ids: Vec<u64>,
    /// Employed residents per household.
    workers: Vec<u32>,
    /// Worker-weighted commute distance per household in metres.
    commute_weighted: Vec<f32>,
    /// Workers per workplace id.
    staffing: BTreeMap<u64, u32>,
}

/// One candidate pair, ordered by the global allocation comparator:
/// distance, then household id, then workplace index.
///
/// `JobHome` entries are collected in ascending id order, so the household
/// index orders exactly like the household id it replaces.
type JobPair = (f32, usize, usize);

/// Allocation state after one greedy pass.
struct JobPass {
    /// Residents of each household still looking for work.
    need: Vec<u32>,
    /// Job slots still open at each workplace.
    remaining: Vec<u32>,
    /// Worker-weighted commute distance per household.
    commute_weighted: Vec<f32>,
}

/// Assign residents to job slots, globally nearest pair first.
///
/// Below [`SMALL_JOB_PAIRS`] this is the original full scan. Above it, each
/// household keeps a sorted candidate stream built from a uniform grid over
/// workplace centres, and a binary heap merges those streams in the same
/// global `(distance, household id, workplace index)` order as the full scan.
/// Only the prefix the greedy actually examines is materialized, so the
/// allocation, tie-break, and float accumulation order match the full scan
/// exactly. Layouts the grid cannot index fall back to the full scan.
fn assign_jobs(
    households: &BTreeMap<u64, Household>,
    workplaces: &[Workplace],
    roads: Option<&RoadGraph>,
    flows: &[Option<FlowField>],
) -> JobAssignment {
    let homes: Vec<JobHome> = households
        .iter()
        .filter(|(_, household)| household.residents > 0)
        .map(|(&id, household)| JobHome {
            id,
            center: household.home,
            residents: household.residents,
        })
        .collect();
    let active_workplaces = workplaces.iter().filter(|work| work.slots > 0).count();
    if homes.is_empty() || active_workplaces == 0 {
        return finish_assignment(&homes, workplaces, run_pass(&[], &homes, workplaces));
    }
    let pairs = homes.len().saturating_mul(active_workplaces);
    if pairs <= SMALL_JOB_PAIRS || homes.len() < MIN_GRID_SIDE || active_workplaces < MIN_GRID_SIDE
    {
        return greedy_full(&homes, workplaces, roads, flows);
    }
    match WorkplaceGrid::build(workplaces, &homes) {
        Some(grid) => assign_with_grid(&grid, &homes, workplaces, roads, flows),
        None => greedy_full(&homes, workplaces, roads, flows),
    }
}

/// One entry in the best-first merge: either a real candidate pair or a
/// frontier marker that grows a household's stream.
///
/// Home indices ascend with household ids, so ordering by `(key, home_index)`
/// reproduces the global `(distance, household id, …)` tie-break. Within one
/// household the stream is sorted by `(distance, workplace index)`, covering
/// the last tie-break.
///
/// A frontier's `key` is the straight-line radius already collected, which is
/// a lower bound on any undiscovered pair's actual distance (`straight-line`
/// never exceeds road distance). A real pair is only queued when every
/// undiscovered pair is provably farther, so the heap minimum is always the
/// globally nearest unconsumed pair.
#[derive(Clone, Copy, Debug)]
struct JobCandidate {
    key: f32,
    home_index: usize,
    /// `None` marks a frontier that must grow before its household can be
    /// compared as a real pair.
    work_index: Option<u32>,
}

impl PartialEq for JobCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for JobCandidate {}

impl Ord for JobCandidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key
            .total_cmp(&other.key)
            .then(self.home_index.cmp(&other.home_index))
            // `None` (frontier) sorts before `Some`, which is the conservative
            // order for equal keys: grow first, then decide.
            .then(self.work_index.cmp(&other.work_index))
    }
}

impl PartialOrd for JobCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Per-household candidate stream.
///
/// `radius` is the straight-line radius already collected. A head candidate is
/// only trusted once its actual distance is at most `radius` (or the range is
/// complete): every uncollected pair has straight-line distance above the
/// radius and therefore actual distance above it as well, because straight-line
/// distance is a lower bound for road distance. Until then this household is
/// grown and re-queued.
struct JobCursor {
    radius: f32,
    complete: bool,
    /// Sorted by `(actual distance, workplace index)`.
    candidates: Vec<(f32, u32)>,
    cursor: usize,
}

impl JobCursor {
    fn new() -> Self {
        Self {
            radius: 0.0,
            complete: false,
            candidates: Vec::new(),
            cursor: 0,
        }
    }

    fn current(&self) -> Option<(f32, u32)> {
        self.candidates.get(self.cursor).copied()
    }

    fn advance(&mut self) {
        self.cursor += 1;
    }
}

fn candidate_ready(state: &JobCursor, distance: f32) -> bool {
    state.complete || distance <= state.radius
}

/// Queue a household's next decision: a ready pair, or a frontier to grow.
fn push_next(
    heap: &mut BinaryHeap<std::cmp::Reverse<JobCandidate>>,
    state: &JobCursor,
    home_index: usize,
) {
    match state.current() {
        Some((distance, work_index)) if candidate_ready(state, distance) => {
            heap.push(std::cmp::Reverse(JobCandidate {
                key: distance,
                home_index,
                work_index: Some(work_index),
            }));
        }
        _ if !state.complete => {
            heap.push(std::cmp::Reverse(JobCandidate {
                key: state.radius,
                home_index,
                work_index: None,
            }));
        }
        _ => {}
    }
}

/// Grow one household's candidate stream to the next radius.
///
/// The annulus is `(previous radius, next radius]` in straight-line distance,
/// so every workplace enters the stream once. Collected pairs are merged into
/// the unconsumed suffix; consumed pairs are provably earlier than anything a
/// later annulus can hold.
fn grow_home(
    state: &mut JobCursor,
    home: &JobHome,
    grid: &WorkplaceGrid,
    workplaces: &[Workplace],
    roads: Option<&RoadGraph>,
    flows: &[Option<FlowField>],
) {
    if state.complete {
        return;
    }
    let previous = state.radius;
    let doubled = if previous > 0.0 {
        previous * 2.0
    } else {
        grid.cell_m
    };
    let complete = !doubled.is_finite() || doubled >= grid.cover_m;
    let upper = if complete { grid.cover_m } else { doubled };
    if previous > 0.0 && upper <= previous {
        state.complete = true;
        return;
    }
    state.radius = upper;
    state.complete = complete;
    let lower = if previous > 0.0 { previous } else { -1.0 };
    let mut gathered = Vec::new();
    grid.collect_annulus(
        home.center,
        lower,
        upper,
        workplaces,
        roads,
        flows,
        &mut gathered,
    );
    if gathered.is_empty() {
        return;
    }
    gathered.sort_by(|left, right| left.0.total_cmp(&right.0).then(left.1.cmp(&right.1)));
    // Drop consumed prefixes so a long-running home does not retain the whole
    // explored candidate list. The cursor and order are preserved.
    if state.cursor > 1_024 && state.cursor * 2 > state.candidates.len() {
        state.candidates.drain(..state.cursor);
        state.cursor = 0;
    }
    let tail = state.candidates.split_off(state.cursor);
    state.candidates.extend(merge_sorted(tail, gathered));
}

fn merge_sorted(mut left: Vec<(f32, u32)>, mut right: Vec<(f32, u32)>) -> Vec<(f32, u32)> {
    let mut merged = Vec::with_capacity(left.len() + right.len());
    let mut left_index = 0;
    let mut right_index = 0;
    while left_index < left.len() && right_index < right.len() {
        let order = left[left_index]
            .0
            .total_cmp(&right[right_index].0)
            .then(left[left_index].1.cmp(&right[right_index].1));
        if order == Ordering::Greater {
            merged.push(right[right_index]);
            right_index += 1;
        } else {
            merged.push(left[left_index]);
            left_index += 1;
        }
    }
    merged.extend(left.drain(left_index..));
    merged.extend(right.drain(right_index..));
    merged
}

/// Best-first allocation over per-household candidate streams.
///
/// Popping the heap minimum yields the globally nearest unconsumed pair, so
/// the greedy decisions are identical to the full scan. The loop stops as soon
/// as all demand or all slots are consumed; if the heap drains first, only
/// off-grid workplaces can be left, and the full scan decides.
fn assign_with_grid(
    grid: &WorkplaceGrid,
    homes: &[JobHome],
    workplaces: &[Workplace],
    roads: Option<&RoadGraph>,
    flows: &[Option<FlowField>],
) -> JobAssignment {
    #[cfg(test)]
    mark_job_path(true);
    let mut need: Vec<u32> = homes.iter().map(|home| home.residents).collect();
    let mut remaining: Vec<u32> = workplaces.iter().map(|work| work.slots).collect();
    let mut commute_weighted = vec![0.0_f32; homes.len()];
    let mut need_left: u64 = need.iter().map(|&value| u64::from(value)).sum();
    let mut slots_left: u64 = remaining.iter().map(|&value| u64::from(value)).sum();

    let mut cursors: Vec<JobCursor> = homes.iter().map(|_| JobCursor::new()).collect();
    let mut heap: BinaryHeap<std::cmp::Reverse<JobCandidate>> = BinaryHeap::new();
    for (index, home) in homes.iter().enumerate() {
        if !home.center.iter().all(|value| value.is_finite()) {
            cursors[index].complete = true;
            continue;
        }
        push_next(&mut heap, &cursors[index], index);
    }

    while let Some(std::cmp::Reverse(candidate)) = heap.pop() {
        let index = candidate.home_index;
        let Some(work_index) = candidate.work_index else {
            // Frontier: extend this household's stream, then re-queue.
            grow_home(
                &mut cursors[index],
                &homes[index],
                grid,
                workplaces,
                roads,
                flows,
            );
            push_next(&mut heap, &cursors[index], index);
            continue;
        };
        let work_index = work_index as usize;
        let Some((distance, head_work)) = cursors[index].current() else {
            continue;
        };
        debug_assert!(candidate_ready(&cursors[index], distance));
        debug_assert_eq!(Some(head_work), candidate.work_index);
        if remaining[work_index] > 0 && need[index] > 0 {
            let take = need[index].min(remaining[work_index]);
            need[index] -= take;
            remaining[work_index] -= take;
            need_left -= u64::from(take);
            slots_left -= u64::from(take);
            commute_weighted[index] += distance * take as f32;
        }
        cursors[index].advance();
        if need[index] > 0 {
            push_next(&mut heap, &cursors[index], index);
        }
        if need_left == 0 || slots_left == 0 {
            break;
        }
    }

    if need_left > 0 && slots_left > 0 {
        // Only off-grid workplaces (non-finite centres) can remain unmatched.
        return greedy_full(homes, workplaces, roads, flows);
    }
    finish_assignment(
        homes,
        workplaces,
        JobPass {
            need,
            remaining,
            commute_weighted,
        },
    )
}

/// Original global scan: materialize every household/workplace pair.
fn greedy_full(
    homes: &[JobHome],
    workplaces: &[Workplace],
    roads: Option<&RoadGraph>,
    flows: &[Option<FlowField>],
) -> JobAssignment {
    #[cfg(test)]
    mark_job_path(false);
    let mut pairs: Vec<JobPair> = Vec::new();
    for (home_index, home) in homes.iter().enumerate() {
        for (work_index, work) in workplaces.iter().enumerate() {
            if work.slots == 0 {
                continue;
            }
            let distance = job_distance(roads, flows, home.center, work_index, work.center);
            pairs.push((distance, home_index, work_index));
        }
    }
    sort_job_pairs(&mut pairs);
    finish_assignment(homes, workplaces, run_pass(&pairs, homes, workplaces))
}

fn sort_job_pairs(pairs: &mut [JobPair]) {
    pairs.sort_by(|left, right| {
        left.0
            .total_cmp(&right.0)
            .then(left.1.cmp(&right.1))
            .then(left.2.cmp(&right.2))
    });
}

/// Commute distance for one home/workplace pair.
fn job_distance(
    roads: Option<&RoadGraph>,
    flows: &[Option<FlowField>],
    home: [f32; 2],
    work_index: usize,
    center: [f32; 2],
) -> f32 {
    roads
        .and_then(|graph| {
            flows
                .get(work_index)
                .and_then(Option::as_ref)
                .and_then(|flow| graph.flow_distance_m(home, flow))
        })
        .unwrap_or_else(|| RoadGraph::straight_line_m(home, center))
}

/// One greedy allocation pass over sorted candidate pairs.
fn run_pass(pairs: &[JobPair], homes: &[JobHome], workplaces: &[Workplace]) -> JobPass {
    let mut need: Vec<u32> = homes.iter().map(|home| home.residents).collect();
    let mut remaining: Vec<u32> = workplaces.iter().map(|work| work.slots).collect();
    let mut commute_weighted = vec![0.0_f32; homes.len()];
    let mut need_left: u64 = need.iter().map(|&value| u64::from(value)).sum();
    let mut slots_left: u64 = remaining.iter().map(|&value| u64::from(value)).sum();
    for &(distance, home_index, work_index) in pairs {
        if need_left == 0 || slots_left == 0 {
            break;
        }
        let slots = &mut remaining[work_index];
        if *slots == 0 {
            continue;
        }
        let needed = &mut need[home_index];
        if *needed == 0 {
            continue;
        }
        let take = (*needed).min(*slots);
        *needed -= take;
        *slots -= take;
        need_left -= u64::from(take);
        slots_left -= u64::from(take);
        commute_weighted[home_index] += distance * take as f32;
    }
    JobPass {
        need,
        remaining,
        commute_weighted,
    }
}

fn finish_assignment(homes: &[JobHome], workplaces: &[Workplace], pass: JobPass) -> JobAssignment {
    let workers = homes
        .iter()
        .zip(&pass.need)
        .map(|(home, &left)| home.residents.saturating_sub(left))
        .collect();
    let mut staffing: BTreeMap<u64, u32> = BTreeMap::new();
    for (index, work) in workplaces.iter().enumerate() {
        let staffed = work.slots.saturating_sub(pass.remaining[index]);
        if staffed > 0 {
            let entry = staffing.entry(work.id).or_insert(0);
            *entry = entry.saturating_add(staffed);
        }
    }
    JobAssignment {
        home_ids: homes.iter().map(|home| home.id).collect(),
        workers,
        commute_weighted: pass.commute_weighted,
        staffing,
    }
}

/// Deterministic uniform grid over workplace centres.
///
/// Workplace indices are bucketed with a flat CSR layout: `offsets` holds the
/// start of each cell in `items`. The grid only narrows candidates; callers
/// still sort every pair with the global comparator.
struct WorkplaceGrid {
    origin: [f32; 2],
    cell_m: f32,
    cols: usize,
    rows: usize,
    offsets: Vec<usize>,
    items: Vec<u32>,
    /// Farthest distance from any home to the workplace bounding-box corner.
    /// Once the search radius reaches it, every indexed workplace is a
    /// candidate.
    cover_m: f32,
}

impl WorkplaceGrid {
    fn build(workplaces: &[Workplace], homes: &[JobHome]) -> Option<Self> {
        // Workplace indices are stored as `u32`.
        if workplaces.len() >= u32::MAX as usize {
            return None;
        }
        let mut min = [f32::INFINITY; 2];
        let mut max = [f32::NEG_INFINITY; 2];
        let mut indexed = 0_usize;
        for work in workplaces {
            if !is_indexable_workplace(work) {
                continue;
            }
            min[0] = min[0].min(work.center[0]);
            min[1] = min[1].min(work.center[1]);
            max[0] = max[0].max(work.center[0]);
            max[1] = max[1].max(work.center[1]);
            indexed += 1;
        }
        if indexed == 0 {
            return None;
        }
        let width = max[0] - min[0];
        let depth = max[1] - min[1];
        if !width.is_finite() || !depth.is_finite() {
            return None;
        }
        // Target roughly one workplace per cell, with a one-metre floor for
        // dense or fully collinear layouts.
        let extent = width.max(depth).max(1.0);
        let area = width * depth;
        let mut cell_m = if area > 0.0 {
            (area / indexed as f32).sqrt()
        } else {
            extent / (indexed as f32).sqrt()
        };
        if !cell_m.is_finite() || cell_m <= 0.0 {
            cell_m = 1.0;
        }
        cell_m = cell_m.max(1.0);

        let max_cells = indexed.saturating_mul(4).saturating_add(64);
        let (mut cols, mut rows) = grid_dims(width, depth, cell_m);
        while cols.saturating_mul(rows) > max_cells {
            cell_m *= 2.0;
            if !cell_m.is_finite() {
                return None;
            }
            (cols, rows) = grid_dims(width, depth, cell_m);
        }
        let cell_count = cols.checked_mul(rows)?;

        let mut cell_of = vec![0_u32; workplaces.len()];
        let mut counts = vec![0_u32; cell_count];
        for (index, work) in workplaces.iter().enumerate() {
            if !is_indexable_workplace(work) {
                continue;
            }
            let col = grid_coord(work.center[0], min[0], cell_m, cols);
            let row = grid_coord(work.center[1], min[1], cell_m, rows);
            let cell = row * cols + col;
            cell_of[index] = u32::try_from(cell).ok()?;
            counts[cell] = counts[cell].saturating_add(1);
        }

        let mut offsets = Vec::with_capacity(cell_count + 1);
        let mut total = 0_usize;
        for &count in &counts {
            offsets.push(total);
            total += count as usize;
        }
        offsets.push(total);

        let mut cursor = offsets.clone();
        let mut items = vec![0_u32; indexed];
        for (index, work) in workplaces.iter().enumerate() {
            if !is_indexable_workplace(work) {
                continue;
            }
            let cell = cell_of[index] as usize;
            items[cursor[cell]] = index as u32;
            cursor[cell] += 1;
        }

        let mut cover_m = 0.0_f32;
        for home in homes {
            if !home.center.iter().all(|value| value.is_finite()) {
                continue;
            }
            let dx = (home.center[0] - min[0])
                .abs()
                .max((home.center[0] - max[0]).abs());
            let dz = (home.center[1] - min[1])
                .abs()
                .max((home.center[1] - max[1]).abs());
            // Overflow makes the cover unreachable; the build then returns
            // `None` and the caller falls back to the full scan.
            cover_m = cover_m.max((dx * dx + dz * dz).sqrt());
        }
        if !cover_m.is_finite() {
            return None;
        }

        Some(Self {
            origin: min,
            cell_m,
            cols,
            rows,
            offsets,
            items,
            cover_m,
        })
    }

    /// Workplace pairs whose straight-line distance from `home` lies in
    /// `(lower, upper]`, with their actual commute distances.
    #[allow(clippy::too_many_arguments)]
    fn collect_annulus(
        &self,
        home: [f32; 2],
        lower: f32,
        upper: f32,
        workplaces: &[Workplace],
        roads: Option<&RoadGraph>,
        flows: &[Option<FlowField>],
        out: &mut Vec<(f32, u32)>,
    ) {
        if !upper.is_finite() || !home.iter().all(|value| value.is_finite()) {
            return;
        }
        let last_col = self.cols as i64 - 1;
        let last_row = self.rows as i64 - 1;
        let low_col = cell_floor(home[0] - upper, self.origin[0], self.cell_m);
        let high_col = cell_floor(home[0] + upper, self.origin[0], self.cell_m);
        let low_row = cell_floor(home[1] - upper, self.origin[1], self.cell_m);
        let high_row = cell_floor(home[1] + upper, self.origin[1], self.cell_m);
        if high_col < 0 || low_col > last_col || high_row < 0 || low_row > last_row {
            return;
        }
        let col_start = low_col.max(0) as usize;
        let col_end = high_col.min(last_col) as usize;
        let row_start = low_row.max(0) as usize;
        let row_end = high_row.min(last_row) as usize;
        for row in row_start..=row_end {
            for col in col_start..=col_end {
                let cell = row * self.cols + col;
                let start = self.offsets[cell];
                let end = self.offsets[cell + 1];
                for &item in &self.items[start..end] {
                    let work_index = item as usize;
                    let center = workplaces[work_index].center;
                    let straight = RoadGraph::straight_line_m(home, center);
                    if straight <= lower || straight > upper {
                        continue;
                    }
                    let distance = if roads.is_some() {
                        job_distance(roads, flows, home, work_index, center)
                    } else {
                        straight
                    };
                    out.push((distance, item));
                }
            }
        }
    }
}

fn is_indexable_workplace(work: &Workplace) -> bool {
    work.slots > 0 && work.center.iter().all(|value| value.is_finite())
}

fn grid_dims(width: f32, depth: f32, cell_m: f32) -> (usize, usize) {
    let cols = ((width / cell_m).floor().max(0.0) as usize).saturating_add(1);
    let rows = ((depth / cell_m).floor().max(0.0) as usize).saturating_add(1);
    (cols, rows)
}

fn grid_coord(value: f32, origin: f32, cell_m: f32, cells: usize) -> usize {
    let index = ((value - origin) / cell_m).floor().max(0.0) as usize;
    index.min(cells - 1)
}

fn cell_floor(value: f32, origin: f32, cell_m: f32) -> i64 {
    ((value - origin) / cell_m).floor() as i64
}

fn normalize_supply(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
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
    fn commute_and_wages_use_the_nearest_workplace() {
        let mut runtime = CityRuntime::new(2026);
        runtime.set_move_in_per_second(2.0);
        let homes = [
            Home {
                id: 1,
                capacity: 1,
                center: [0.0, 0.0],
            },
            Home {
                id: 2,
                capacity: 1,
                center: [100.0, 0.0],
            },
        ];
        let workplaces = [
            Workplace {
                id: 10,
                slots: 1,
                center: [10.0, 0.0],
            },
            Workplace {
                id: 11,
                slots: 1,
                center: [90.0, 0.0],
            },
        ];
        runtime.tick_with_workplaces(1.0, &homes, &workplaces);
        assert_eq!(runtime.workers(), 2);
        assert_eq!(runtime.households[&1].workers, 1);
        assert_eq!(runtime.households[&2].workers, 1);
        assert!((runtime.households[&1].commute_m - 10.0).abs() < 0.001);
        assert!((runtime.households[&2].commute_m - 10.0).abs() < 0.001);
        assert!((runtime.daily_wages() - 40.0).abs() < 0.001);
        assert!((runtime.average_commute_m() - 10.0).abs() < 0.001);
        assert!((runtime.households[&1].commute_time_s - 1.25).abs() < 0.001);
        assert!((runtime.average_commute_time_s() - 1.25).abs() < 0.001);
    }

    #[test]
    fn road_commute_follows_the_network_and_falls_back_when_disconnected() {
        fn road(id: u64, start: [i32; 2], end: [i32; 2]) -> crate::road::RoadSegment {
            crate::road::RoadSegment {
                id,
                start,
                end,
                width_m: 8,
            }
        }
        let graph = RoadGraph::build(&[
            road(1, [0, 0], [0, 20]),
            road(2, [0, 20], [20, 20]),
            road(3, [20, 20], [20, 0]),
        ])
        .expect("graph");
        let homes = [Home {
            id: 1,
            capacity: 1,
            center: [0.0, 0.0],
        }];
        let workplaces = [Workplace {
            id: 10,
            slots: 1,
            center: [20.0, 0.0],
        }];
        let mut runtime = CityRuntime::new(2026);
        runtime.set_move_in_per_second(2.0);
        runtime.tick_with_supplies_and_roads(1.0, &homes, &workplaces, 1.0, 1.0, 1.0, Some(&graph));
        assert!((runtime.households[&1].commute_m - 60.0).abs() < 0.001);

        let disconnected =
            RoadGraph::build(&[road(1, [0, 0], [0, 10]), road(2, [20, 0], [20, 10])])
                .expect("graph");
        let mut runtime = CityRuntime::new(2026);
        runtime.set_move_in_per_second(2.0);
        runtime.tick_with_supplies_and_roads(
            1.0,
            &homes,
            &workplaces,
            1.0,
            1.0,
            1.0,
            Some(&disconnected),
        );
        assert!((runtime.households[&1].commute_m - 20.0).abs() < 0.001);
    }

    #[test]
    fn a_workplace_shortage_assigns_the_closest_household() {
        let mut runtime = CityRuntime::new(2026);
        runtime.set_move_in_per_second(2.0);
        let homes = [
            Home {
                id: 1,
                capacity: 1,
                center: [0.0, 0.0],
            },
            Home {
                id: 2,
                capacity: 1,
                center: [100.0, 0.0],
            },
        ];
        let workplaces = [Workplace {
            id: 10,
            slots: 1,
            center: [10.0, 0.0],
        }];
        runtime.tick_with_workplaces(1.0, &homes, &workplaces);
        assert_eq!(runtime.households[&1].workers, 1);
        assert_eq!(runtime.households[&2].workers, 0);
        assert_eq!(runtime.households[&2].income_per_day, 0.0);
    }

    #[test]
    fn long_commutes_lower_happiness() {
        let homes = [Home {
            id: 1,
            capacity: 1,
            center: [0.0, 0.0],
        }];
        let near = [Workplace {
            id: 10,
            slots: 1,
            center: [0.0, 0.0],
        }];
        let far = [Workplace {
            id: 10,
            slots: 1,
            center: [150.0, 0.0],
        }];
        let mut close = CityRuntime::new(2026);
        close.set_move_in_per_second(2.0);
        close.tick_with_workplaces(1.0, &homes, &near);
        let mut distant = CityRuntime::new(2026);
        distant.set_move_in_per_second(2.0);
        distant.tick_with_workplaces(1.0, &homes, &far);
        assert!(distant.households[&1].commute_m > 100.0);
        assert!(distant.happiness() < close.happiness());
    }

    #[test]
    fn old_household_saves_default_commute_and_income() {
        let mut runtime = CityRuntime::new(2026);
        runtime.set_move_in_per_second(2.0);
        runtime.tick(1.0, 1, &[(1, 1)]);
        let mut value: serde_json::Value =
            serde_json::from_str(&runtime.to_json_pretty().expect("json")).expect("value");
        let household = value["households"]["1"].as_object_mut().expect("household");
        household.remove("commute_m");
        household.remove("income_per_day");
        household.remove("home");
        let decoded = CityRuntime::from_json(&value.to_string()).expect("decode");
        assert_eq!(decoded.households[&1].commute_m, 0.0);
        assert_eq!(decoded.households[&1].income_per_day, 0.0);
    }

    #[test]
    fn commute_time_is_derived_and_old_saves_refresh() {
        let homes = [Home {
            id: 1,
            capacity: 1,
            center: [0.0, 0.0],
        }];
        let workplaces = [Workplace {
            id: 10,
            slots: 1,
            center: [80.0, 0.0],
        }];
        let mut runtime = CityRuntime::new(2026);
        runtime.set_move_in_per_second(2.0);
        runtime.tick_with_workplaces(1.0, &homes, &workplaces);
        assert!((runtime.households[&1].commute_time_s - 10.0).abs() < 0.001);
        let mut value: serde_json::Value =
            serde_json::from_str(&runtime.to_json_pretty().expect("json")).expect("value");
        value["households"]["1"]
            .as_object_mut()
            .expect("household")
            .remove("commute_time_s");
        let mut decoded = CityRuntime::from_json(&value.to_string()).expect("decode");
        assert_eq!(decoded.households[&1].commute_time_s, 0.0);
        decoded.tick_with_workplaces(1.0, &homes, &workplaces);
        assert!((decoded.households[&1].commute_time_s - 10.0).abs() < 0.001);
    }

    #[test]
    fn the_nearest_household_wins_regardless_of_id_order() {
        let mut runtime = CityRuntime::new(2026);
        runtime.set_move_in_per_second(2.0);
        let homes = [
            Home {
                id: 1,
                capacity: 1,
                center: [100.0, 0.0],
            },
            Home {
                id: 2,
                capacity: 1,
                center: [0.0, 0.0],
            },
        ];
        let workplaces = [Workplace {
            id: 10,
            slots: 1,
            center: [0.0, 0.0],
        }];
        runtime.tick_with_workplaces(1.0, &homes, &workplaces);
        assert_eq!(runtime.households[&1].workers, 0);
        assert_eq!(runtime.households[&2].workers, 1);
    }

    #[test]
    fn residents_split_across_workplaces_by_distance() {
        let mut runtime = CityRuntime::new(2026);
        runtime.set_move_in_per_second(2.0);
        let homes = [Home {
            id: 1,
            capacity: 2,
            center: [0.0, 0.0],
        }];
        let workplaces = [
            Workplace {
                id: 10,
                slots: 1,
                center: [10.0, 0.0],
            },
            Workplace {
                id: 11,
                slots: 1,
                center: [100.0, 0.0],
            },
        ];
        runtime.tick_with_workplaces(1.0, &homes, &workplaces);
        assert_eq!(runtime.households[&1].workers, 2);
        assert!((runtime.households[&1].commute_m - 55.0).abs() < 0.001);
        assert!((runtime.households[&1].income_per_day - 40.0).abs() < 0.001);
    }

    #[test]
    fn commute_and_income_round_trip_through_json() {
        let mut runtime = CityRuntime::new(2026);
        runtime.set_move_in_per_second(2.0);
        let homes = [Home {
            id: 1,
            capacity: 1,
            center: [0.0, 0.0],
        }];
        let workplaces = [Workplace {
            id: 10,
            slots: 1,
            center: [25.0, 0.0],
        }];
        runtime.tick_with_workplaces(1.0, &homes, &workplaces);
        let json = runtime.to_json_pretty().expect("json");
        let decoded = CityRuntime::from_json(&json).expect("decode");
        assert_eq!(decoded, runtime);
        assert!((decoded.households[&1].commute_m - 25.0).abs() < 0.001);
        assert!((decoded.households[&1].income_per_day - 20.0).abs() < 0.001);
    }

    #[test]
    fn staffing_records_workers_per_workplace() {
        let mut runtime = CityRuntime::new(2026);
        runtime.set_move_in_per_second(2.0);
        let homes = [
            Home {
                id: 1,
                capacity: 1,
                center: [0.0, 0.0],
            },
            Home {
                id: 2,
                capacity: 1,
                center: [10.0, 0.0],
            },
        ];
        let workplaces = [
            Workplace {
                id: 10,
                slots: 1,
                center: [0.0, 0.0],
            },
            Workplace {
                id: 11,
                slots: 1,
                center: [10.0, 0.0],
            },
        ];
        runtime.tick_with_workplaces(1.0, &homes, &workplaces);
        assert_eq!(runtime.staffing.get(&10), Some(&1));
        assert_eq!(runtime.staffing.get(&11), Some(&1));
        assert_eq!(runtime.staffing.values().sum::<u32>(), 2);
        assert_eq!(
            runtime.staffing.values().sum::<u32>(),
            runtime.workers() as u32
        );
    }

    #[test]
    fn food_supply_replenishes_the_food_need() {
        let homes = [Home {
            id: 1,
            capacity: 1,
            center: [0.0, 0.0],
        }];
        let workplaces = [Workplace {
            id: 10,
            slots: 1,
            center: [0.0, 0.0],
        }];
        let mut fed = CityRuntime::new(2026);
        fed.set_move_in_per_second(2.0);
        fed.tick_with_workplaces_and_supply(1.0, &homes, &workplaces, 1.0);
        let mut hungry = CityRuntime::new(2026);
        hungry.set_move_in_per_second(2.0);
        hungry.tick_with_workplaces_and_supply(1.0, &homes, &workplaces, 0.0);
        fed.households.get_mut(&1).expect("home").food = 0.5;
        hungry.households.get_mut(&1).expect("home").food = 0.5;
        fed.tick_with_workplaces_and_supply(100.0, &homes, &workplaces, 1.0);
        hungry.tick_with_workplaces_and_supply(100.0, &homes, &workplaces, 0.0);
        assert!(fed.households[&1].food > 0.5);
        assert!(hungry.households[&1].food < 0.5);
    }

    #[test]
    fn old_saves_without_staffing_load_empty() {
        let mut runtime = CityRuntime::new(2026);
        runtime.set_move_in_per_second(2.0);
        runtime.tick(1.0, 1, &[(1, 1)]);
        let mut value: serde_json::Value =
            serde_json::from_str(&runtime.to_json_pretty().expect("json")).expect("value");
        value.as_object_mut().expect("object").remove("staffing");
        let decoded = CityRuntime::from_json(&value.to_string()).expect("decode");
        assert!(decoded.staffing.is_empty());
    }

    #[test]
    fn water_and_rest_replenish_with_service() {
        let mut runtime = CityRuntime::new(2026);
        runtime.set_move_in_per_second(2.0);
        runtime.tick_with_supplies(
            1_000.0,
            &[Home {
                id: 1,
                capacity: 2,
                center: [0.0, 0.0],
            }],
            &[],
            0.0,
            0.0,
            0.0,
        );
        let before = runtime.households[&1].clone();
        assert!(before.water < 0.8);
        assert!(before.rest < 0.9);
        runtime.tick_with_supplies(
            300.0,
            &[Home {
                id: 1,
                capacity: 2,
                center: [0.0, 0.0],
            }],
            &[],
            1.0,
            1.0,
            1.0,
        );
        let after = &runtime.households[&1];
        assert!(after.water > before.water);
        assert!(after.rest > before.rest);
        assert!(after.water <= 1.0);
        assert!(after.rest <= 1.0);
    }

    #[test]
    fn service_shares_are_clamped() {
        let mut runtime = CityRuntime::new(2026);
        runtime.set_move_in_per_second(2.0);
        let homes = [Home {
            id: 1,
            capacity: 1,
            center: [0.0, 0.0],
        }];
        runtime.tick_with_supplies(10.0, &homes, &[], f32::NAN, -1.0, 5.0);
        let household = &runtime.households[&1];
        assert!(household.food < 1.0);
        assert!(household.water < 1.0);
        assert!(household.rest <= 1.0);
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
                commute_m: 0.0,
                commute_time_s: 0.0,
                income_per_day: 0.0,
                home: [0.0, 0.0],
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

    // --- Job-assignment index -------------------------------------------

    fn lcg_next(seed: &mut u64) -> u32 {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        u32::try_from(*seed >> 33).unwrap_or(0)
    }

    fn jitter(seed: &mut u64, span: f32) -> f32 {
        let unit = f32::from(u16::try_from(lcg_next(seed) % 65_536).unwrap_or(0)) / 65_535.0;
        unit * span
    }

    fn cluster_fixture(
        homes: usize,
        workplaces: usize,
        capacity: u32,
        slots: u32,
        span: f32,
    ) -> (Vec<Home>, Vec<Workplace>) {
        let mut seed = 0x5eed_2026_u64;
        let homes = (0..homes)
            .map(|index| Home {
                id: u64::try_from(index).expect("id") + 1,
                capacity,
                center: [jitter(&mut seed, span), jitter(&mut seed, span)],
            })
            .collect();
        let workplaces = (0..workplaces)
            .map(|index| Workplace {
                id: 1_000_000 + u64::try_from(index).expect("id"),
                slots,
                center: [jitter(&mut seed, span), jitter(&mut seed, span)],
            })
            .collect();
        (homes, workplaces)
    }

    /// The original global scan, kept as an independent reference.
    fn brute_force_reference(
        runtime: &CityRuntime,
        workplaces: &[Workplace],
        roads: Option<&RoadGraph>,
    ) -> (BTreeMap<u64, u32>, BTreeMap<u64, f32>, BTreeMap<u64, u32>) {
        let flows = roads
            .map(|graph| {
                workplaces
                    .iter()
                    .map(|work| graph.flow_to(work.center))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut pairs: Vec<(f32, u64, usize)> = Vec::new();
        for (&id, household) in &runtime.households {
            if household.residents == 0 {
                continue;
            }
            for (index, work) in workplaces.iter().enumerate() {
                if work.slots == 0 {
                    continue;
                }
                let distance = roads
                    .and_then(|graph| {
                        flows
                            .get(index)
                            .and_then(Option::as_ref)
                            .and_then(|flow| graph.flow_distance_m(household.home, flow))
                    })
                    .unwrap_or_else(|| RoadGraph::straight_line_m(household.home, work.center));
                pairs.push((distance, id, index));
            }
        }
        pairs.sort_by(|left, right| {
            left.0
                .total_cmp(&right.0)
                .then(left.1.cmp(&right.1))
                .then(left.2.cmp(&right.2))
        });
        let mut remaining: Vec<u32> = workplaces.iter().map(|work| work.slots).collect();
        let mut need: BTreeMap<u64, u32> = runtime
            .households
            .iter()
            .filter(|(_, household)| household.residents > 0)
            .map(|(&id, household)| (id, household.residents))
            .collect();
        let mut workers: BTreeMap<u64, u32> = BTreeMap::new();
        let mut weighted: BTreeMap<u64, f32> = BTreeMap::new();
        let mut staffing: BTreeMap<u64, u32> = BTreeMap::new();
        for (distance, id, index) in pairs {
            if remaining[index] == 0 {
                continue;
            }
            let Some(left) = need.get_mut(&id) else {
                continue;
            };
            if *left == 0 {
                continue;
            }
            let take = (*left).min(remaining[index]);
            remaining[index] -= take;
            *left -= take;
            *workers.entry(id).or_insert(0) += take;
            *weighted.entry(id).or_insert(0.0) += distance * take as f32;
            let staffed = staffing.entry(workplaces[index].id).or_insert(0);
            *staffed = staffed.saturating_add(take);
        }
        (workers, weighted, staffing)
    }

    fn compare_with_reference(homes: &[Home], workplaces: &[Workplace], roads: Option<&RoadGraph>) {
        let mut runtime = CityRuntime::new(2026);
        runtime.set_move_in_per_second(MAX_MOVE_IN_PER_SECOND);
        runtime.tick_with_supplies_and_roads(
            MAX_TICK_SECONDS,
            homes,
            workplaces,
            1.0,
            1.0,
            1.0,
            roads,
        );
        let (workers, weighted, staffing) = brute_force_reference(&runtime, workplaces, roads);
        assert_eq!(runtime.staffing, staffing, "staffing map");
        for (&id, household) in &runtime.households {
            let expected_workers = workers.get(&id).copied().unwrap_or(0);
            assert_eq!(household.workers, expected_workers, "workers for {id}");
            let expected_commute = if expected_workers > 0 {
                weighted[&id] / expected_workers as f32
            } else {
                0.0
            };
            assert_eq!(
                household.commute_m.to_bits(),
                expected_commute.to_bits(),
                "commute for {id}"
            );
            assert_eq!(
                household.income_per_day,
                f64::from(expected_workers) * WAGE_PER_WORKER_PER_DAY,
                "income for {id}"
            );
        }
    }

    #[test]
    fn grid_assignment_matches_the_global_scan() {
        let (homes, workplaces) = cluster_fixture(96, 112, 4, 3, 1_200.0);
        assert!(
            homes.len() * workplaces.len() > SMALL_JOB_PAIRS,
            "fixture must exercise the indexed path"
        );
        compare_with_reference(&homes, &workplaces, None);
        assert!(last_job_path_was_grid());
    }

    #[test]
    fn grid_assignment_expands_to_far_workplaces() {
        let mut seed = 7_u64;
        let homes: Vec<Home> = (0..100)
            .map(|index| Home {
                id: u64::try_from(index).expect("id") + 1,
                capacity: 2,
                center: [jitter(&mut seed, 200.0), jitter(&mut seed, 200.0)],
            })
            .collect();
        let workplaces: Vec<Workplace> = (0..100)
            .map(|index| Workplace {
                id: 1_000_000 + u64::try_from(index).expect("id"),
                slots: 1,
                center: [
                    8_000.0 + jitter(&mut seed, 400.0),
                    jitter(&mut seed, 5_000.0),
                ],
            })
            .collect();
        compare_with_reference(&homes, &workplaces, None);
        assert!(last_job_path_was_grid());
    }

    #[test]
    fn grid_assignment_matches_the_global_scan_on_roads() {
        fn road(id: u64, start: [i32; 2], end: [i32; 2]) -> crate::road::RoadSegment {
            crate::road::RoadSegment {
                id,
                start,
                end,
                width_m: 8,
            }
        }
        let mut roads = Vec::new();
        let mut id = 1_u64;
        for offset in (0..=600).step_by(100) {
            roads.push(road(id, [offset, 0], [offset, 600]));
            id += 1;
            roads.push(road(id, [0, offset], [600, offset]));
            id += 1;
        }
        let graph = RoadGraph::build(&roads).expect("road grid");
        let (homes, workplaces) = cluster_fixture(80, 80, 4, 3, 600.0);
        assert!(
            homes.len() * workplaces.len() > SMALL_JOB_PAIRS,
            "fixture must exercise the indexed path"
        );
        compare_with_reference(&homes, &workplaces, Some(&graph));
        assert!(last_job_path_was_grid());
    }

    #[test]
    fn grid_assignment_falls_back_for_non_finite_workplaces() {
        let (homes, mut workplaces) = cluster_fixture(80, 80, 1, 1, 1_000.0);
        for (index, work) in workplaces.iter_mut().enumerate() {
            if index % 2 == 0 {
                work.center = [f32::NAN, work.center[1]];
            }
        }
        assert!(
            homes.len() * workplaces.len() > SMALL_JOB_PAIRS,
            "fixture must exercise the indexed path"
        );
        compare_with_reference(&homes, &workplaces, None);
        assert!(!last_job_path_was_grid());
    }

    #[test]
    fn a_thousand_homes_and_workplaces_assign_in_one_tick() {
        let (homes, workplaces) = cluster_fixture(1_000, 1_000, 1, 1, 3_000.0);
        let mut runtime = CityRuntime::new(2026);
        runtime.set_move_in_per_second(MAX_MOVE_IN_PER_SECOND);
        runtime.tick_with_workplaces(MAX_TICK_SECONDS, &homes, &workplaces);
        assert_eq!(runtime.population(), 1_000);
        assert_eq!(runtime.workers(), 1_000);
        assert_eq!(runtime.staffing.values().sum::<u32>(), 1_000);
        assert_eq!(runtime.staffing.len(), 1_000);
        for household in runtime.households.values() {
            assert_eq!(household.workers, 1);
            assert!(household.commute_m >= 0.0);
        }
    }
}
