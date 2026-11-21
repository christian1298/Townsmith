//! Water and power service coverage.
//!
//! Phase 3.5 adds a city-wide utility layer. A utility site is a placement id
//! plus a kind; completed homes and workplaces consume water and power when
//! they are within a site's radius. Supply is the served share of total
//! demand, so capacity shortages and uncovered buildings both reduce service.
//!
//! This is deliberately aggregate: no pipes, wires, flow simulation, or
//! individual utility workers yet. The core layer is engine-agnostic.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Current utility save format.
pub const UTILITIES_FORMAT_VERSION: u16 = 1;

/// Maximum utility sites per kind in one save.
pub const MAX_UTILITY_SITES: usize = 10_000;

/// Largest service radius accepted from a save.
pub const MAX_UTILITY_RADIUS_M: f32 = 10_000.0;

/// Largest installed capacity accepted from a save.
pub const MAX_UTILITY_CAPACITY: f64 = 1_000_000_000.0;

/// Default water service radius in metres.
pub const DEFAULT_WATER_RADIUS_M: f32 = 150.0;

/// Default power service radius in metres.
pub const DEFAULT_POWER_RADIUS_M: f32 = 150.0;

/// Residents one water site can serve.
pub const WATER_CAPACITY_PER_SITE: f64 = 300.0;

/// Power demand units one power site can serve.
pub const POWER_CAPACITY_PER_SITE: f64 = 300.0;

/// Water demand per resident.
pub const WATER_PER_RESIDENT: f64 = 1.0;

/// Power demand per employed worker.
pub const POWER_PER_WORKER: f64 = 1.0;

/// Power demand per resident for lighting, heating, and appliances.
pub const POWER_PER_RESIDENT: f64 = 0.25;

/// Construction cost of one water site.
pub const WATER_CONSTRUCTION_COST: f64 = 2_500.0;

/// Construction cost of one power site.
pub const POWER_CONSTRUCTION_COST: f64 = 5_000.0;

/// Operating upkeep of one water site per game day.
pub const WATER_UPKEEP_PER_DAY: f64 = 2.0;

/// Operating upkeep of one power site per game day.
pub const POWER_UPKEEP_PER_DAY: f64 = 3.0;

/// One utility kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UtilityKind {
    /// Wells, pumps, towers, and treatment.
    Water,
    /// Generation and distribution.
    Power,
}

impl UtilityKind {
    /// Every utility kind, in display order.
    pub const ALL: [Self; 2] = [Self::Water, Self::Power];

    /// Stable lowercase identifier.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Water => "water",
            Self::Power => "power",
        }
    }

    /// Parse a stable identifier.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "water" | "well" | "pump" | "water_tower" => Some(Self::Water),
            "power" | "grid" | "plant" | "power_plant" => Some(Self::Power),
            _ => None,
        }
    }

    /// Construction cost of one default site of this kind.
    ///
    /// Water is 2,500; power is 5,000. These are data-only catalog values and
    /// are not charged by the live economy yet.
    #[must_use]
    pub fn construction_cost(self) -> f64 {
        match self {
            Self::Water => WATER_CONSTRUCTION_COST,
            Self::Power => POWER_CONSTRUCTION_COST,
        }
    }

    /// Operating upkeep of one site of this kind per game day.
    ///
    /// Water is 2/day; power is 3/day. These are data-only catalog values and
    /// are not charged by the live economy yet.
    #[must_use]
    pub fn upkeep_per_day(self) -> f64 {
        match self {
            Self::Water => WATER_UPKEEP_PER_DAY,
            Self::Power => POWER_UPKEEP_PER_DAY,
        }
    }
}

/// One installed utility site.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UtilitySite {
    /// Site centre in metres.
    #[serde(default)]
    pub center: [f32; 3],
    /// Service radius in metres.
    pub radius_m: f32,
    /// Installed capacity in demand units.
    pub capacity: f64,
}

impl UtilitySite {
    /// Default site for a kind.
    #[must_use]
    pub fn default_for(kind: UtilityKind) -> Self {
        match kind {
            UtilityKind::Water => Self {
                center: [0.0, 0.0, 0.0],
                radius_m: DEFAULT_WATER_RADIUS_M,
                capacity: WATER_CAPACITY_PER_SITE,
            },
            UtilityKind::Power => Self {
                center: [0.0, 0.0, 0.0],
                radius_m: DEFAULT_POWER_RADIUS_M,
                capacity: POWER_CAPACITY_PER_SITE,
            },
        }
    }
}

/// One completed building that demands utility service.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UtilityConsumer {
    /// Placement id.
    pub id: u64,
    /// World position in metres.
    pub center: [f32; 2],
    /// Residents currently living here.
    pub residents: u32,
    /// Workers assigned here in the last household tick.
    pub workers: u32,
}

impl UtilityConsumer {
    /// Water demand in residents.
    #[must_use]
    pub fn water_demand(&self) -> f64 {
        f64::from(self.residents) * WATER_PER_RESIDENT
    }

    /// Power demand from workers and residents.
    #[must_use]
    pub fn power_demand(&self) -> f64 {
        f64::from(self.workers) * POWER_PER_WORKER + f64::from(self.residents) * POWER_PER_RESIDENT
    }
}

/// Deterministic utility runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UtilityStore {
    /// Save format version.
    pub format_version: u16,
    /// World seed this store belongs to.
    pub seed: u64,
    /// Water sites keyed by placement id.
    #[serde(default)]
    pub water_sites: BTreeMap<u64, UtilitySite>,
    /// Power sites keyed by placement id.
    #[serde(default)]
    pub power_sites: BTreeMap<u64, UtilitySite>,
    /// Water service share from the last refresh, 0..=1.
    #[serde(default)]
    pub water_supply: f32,
    /// Power service share from the last refresh, 0..=1.
    #[serde(default)]
    pub power_supply: f32,
}

impl UtilityStore {
    /// Create an empty utility store for a seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            format_version: UTILITIES_FORMAT_VERSION,
            seed,
            water_sites: BTreeMap::new(),
            power_sites: BTreeMap::new(),
            water_supply: 0.0,
            power_supply: 0.0,
        }
    }

    /// Total sites across both kinds.
    #[must_use]
    pub fn site_count(&self) -> usize {
        self.water_sites.len() + self.power_sites.len()
    }

    /// Sites of one kind.
    #[must_use]
    pub fn sites(&self, kind: UtilityKind) -> &BTreeMap<u64, UtilitySite> {
        match kind {
            UtilityKind::Water => &self.water_sites,
            UtilityKind::Power => &self.power_sites,
        }
    }

    /// Sites of one kind, mutable.
    fn sites_mut(&mut self, kind: UtilityKind) -> &mut BTreeMap<u64, UtilitySite> {
        match kind {
            UtilityKind::Water => &mut self.water_sites,
            UtilityKind::Power => &mut self.power_sites,
        }
    }

    /// Number of sites of one kind.
    #[must_use]
    pub fn count_of(&self, kind: UtilityKind) -> usize {
        self.sites(kind).len()
    }

    /// Construction cost of every site of one kind.
    #[must_use]
    pub fn construction_cost_of(&self, kind: UtilityKind) -> f64 {
        self.count_of(kind) as f64 * kind.construction_cost()
    }

    /// Operating upkeep of every site of one kind per game day.
    #[must_use]
    pub fn upkeep_per_day_of(&self, kind: UtilityKind) -> f64 {
        self.count_of(kind) as f64 * kind.upkeep_per_day()
    }

    /// Construction cost of every water and power site.
    #[must_use]
    pub fn total_construction_cost(&self) -> f64 {
        UtilityKind::ALL
            .iter()
            .map(|&kind| self.construction_cost_of(kind))
            .sum()
    }

    /// Operating upkeep of every water and power site per game day.
    #[must_use]
    pub fn total_upkeep_per_day(&self) -> f64 {
        UtilityKind::ALL
            .iter()
            .map(|&kind| self.upkeep_per_day_of(kind))
            .sum()
    }

    /// Add or replace a utility site. Replacing keeps other sites.
    pub fn add_site(
        &mut self,
        placement_id: u64,
        kind: UtilityKind,
        center: [f32; 3],
    ) -> Result<(), UtilityError> {
        if placement_id > i64::MAX as u64 {
            return Err(UtilityError::Invalid("site id is above i64::MAX"));
        }
        let sites = self.sites_mut(kind);
        if !sites.contains_key(&placement_id) && sites.len() >= MAX_UTILITY_SITES {
            return Err(UtilityError::TooManySites(sites.len()));
        }
        let mut site = UtilitySite::default_for(kind);
        site.center = center;
        sites.insert(placement_id, site);
        Ok(())
    }

    /// Remove one site of a kind.
    pub fn remove_site(&mut self, placement_id: u64, kind: UtilityKind) -> bool {
        self.sites_mut(kind).remove(&placement_id).is_some()
    }

    /// Remove both utility kinds for a placement.
    pub fn remove_placement(&mut self, placement_id: u64) -> bool {
        let water = self.water_sites.remove(&placement_id).is_some();
        let power = self.power_sites.remove(&placement_id).is_some();
        water || power
    }

    /// Remove every site and reset service.
    pub fn clear(&mut self) {
        self.water_sites.clear();
        self.power_sites.clear();
        self.water_supply = 0.0;
        self.power_supply = 0.0;
    }

    /// Recompute service coverage from completed consumers.
    ///
    /// Demand is counted for every consumer. A consumer is covered when at
    /// least one site of that kind is within its radius. Served demand is
    /// `min(total capacity, covered demand)`, so uncovered demand also lowers
    /// the city-wide service share.
    pub fn refresh(&mut self, consumers: &[UtilityConsumer]) -> UtilityReport {
        let water_demand: f64 = consumers.iter().map(UtilityConsumer::water_demand).sum();
        let power_demand: f64 = consumers.iter().map(UtilityConsumer::power_demand).sum();
        let water_capacity: f64 = self.water_sites.values().map(|site| site.capacity).sum();
        let power_capacity: f64 = self.power_sites.values().map(|site| site.capacity).sum();
        let water_covered = covered_demand(consumers, &self.water_sites, UtilityDemand::Water);
        let power_covered = covered_demand(consumers, &self.power_sites, UtilityDemand::Power);
        let water = supply_share(
            water_demand,
            water_covered,
            water_capacity,
            self.water_sites.len(),
        );
        let power = supply_share(
            power_demand,
            power_covered,
            power_capacity,
            self.power_sites.len(),
        );
        self.water_supply = water.supply;
        self.power_supply = power.supply;

        UtilityReport {
            water_supply: water.supply,
            power_supply: power.supply,
            water_demand,
            power_demand,
            water_served: water.served,
            power_served: power.served,
            water_capacity,
            power_capacity,
            water_coverage: water.coverage,
            power_coverage: power.coverage,
            water_covered_buildings: consumers
                .iter()
                .filter(|consumer| {
                    consumer.water_demand() > 0.0
                        && covered(consumer, &self.water_sites, UtilityDemand::Water)
                })
                .count(),
            power_covered_buildings: consumers
                .iter()
                .filter(|consumer| {
                    consumer.power_demand() > 0.0
                        && covered(consumer, &self.power_sites, UtilityDemand::Power)
                })
                .count(),
            water_sites: self.water_sites.len(),
            power_sites: self.power_sites.len(),
            consumers: consumers.len(),
        }
    }

    /// Serialize as pretty JSON.
    pub fn to_json_pretty(&self) -> Result<String, UtilityError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse and validate a utility save.
    pub fn from_json(json: &str) -> Result<Self, UtilityError> {
        let store: Self = serde_json::from_str(json)?;
        store.validate()?;
        Ok(store)
    }

    /// Validate all ranges and state invariants.
    pub fn validate(&self) -> Result<(), UtilityError> {
        if self.format_version != UTILITIES_FORMAT_VERSION {
            return Err(UtilityError::UnsupportedVersion(self.format_version));
        }
        for count in [self.water_sites.len(), self.power_sites.len()] {
            if count > MAX_UTILITY_SITES {
                return Err(UtilityError::TooManySites(count));
            }
        }
        for (name, supply) in [
            ("water supply", self.water_supply),
            ("power supply", self.power_supply),
        ] {
            if !supply.is_finite() || !(0.0..=1.0).contains(&supply) {
                return Err(UtilityError::Invalid(name));
            }
        }
        for sites in [&self.water_sites, &self.power_sites] {
            for (&id, site) in sites {
                if id > i64::MAX as u64 {
                    return Err(UtilityError::Invalid("site id is above i64::MAX"));
                }
                if site.center.iter().any(|value| !value.is_finite()) {
                    return Err(UtilityError::Invalid("site centre"));
                }
                if !site.radius_m.is_finite()
                    || !(0.0..=MAX_UTILITY_RADIUS_M).contains(&site.radius_m)
                {
                    return Err(UtilityError::Invalid("site radius"));
                }
                if !site.capacity.is_finite()
                    || !(0.0..=MAX_UTILITY_CAPACITY).contains(&site.capacity)
                {
                    return Err(UtilityError::Invalid("site capacity"));
                }
            }
        }
        Ok(())
    }

    /// FNV-1a checksum over the compact JSON.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let json = serde_json::to_vec(self).expect("utilities always serialize");
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in json {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
}

/// Result of one utility refresh.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UtilityReport {
    /// Water service share, 0..=1.
    pub water_supply: f32,
    /// Power service share, 0..=1.
    pub power_supply: f32,
    /// Total water demand.
    pub water_demand: f64,
    /// Total power demand.
    pub power_demand: f64,
    /// Water demand served by capacity and coverage.
    pub water_served: f64,
    /// Power demand served by capacity and coverage.
    pub power_served: f64,
    /// Installed water capacity.
    pub water_capacity: f64,
    /// Installed power capacity.
    pub power_capacity: f64,
    /// Share of water demand inside a water radius.
    pub water_coverage: f32,
    /// Share of power demand inside a power radius.
    pub power_coverage: f32,
    /// Consumers with water demand inside a radius.
    pub water_covered_buildings: usize,
    /// Consumers with power demand inside a radius.
    pub power_covered_buildings: usize,
    /// Water site count.
    pub water_sites: usize,
    /// Power site count.
    pub power_sites: usize,
    /// Consumers evaluated.
    pub consumers: usize,
}

#[derive(Debug, Clone, Copy)]
enum UtilityDemand {
    Water,
    Power,
}

fn consumer_demand(consumer: &UtilityConsumer, demand: UtilityDemand) -> f64 {
    match demand {
        UtilityDemand::Water => consumer.water_demand(),
        UtilityDemand::Power => consumer.power_demand(),
    }
}

fn site_covers(site: &UtilitySite, consumer: &UtilityConsumer) -> bool {
    let dx = f64::from(site.center[0]) - f64::from(consumer.center[0]);
    let dz = f64::from(site.center[2]) - f64::from(consumer.center[1]);
    dx * dx + dz * dz <= f64::from(site.radius_m) * f64::from(site.radius_m)
}

fn covered(
    consumer: &UtilityConsumer,
    sites: &BTreeMap<u64, UtilitySite>,
    demand: UtilityDemand,
) -> bool {
    consumer_demand(consumer, demand) > 0.0
        && sites.values().any(|site| site_covers(site, consumer))
}

fn covered_demand(
    consumers: &[UtilityConsumer],
    sites: &BTreeMap<u64, UtilitySite>,
    demand: UtilityDemand,
) -> f64 {
    consumers
        .iter()
        .filter(|consumer| covered(consumer, sites, demand))
        .map(|consumer| consumer_demand(consumer, demand))
        .sum()
}

fn supply_share(demand: f64, covered_demand: f64, capacity: f64, sites: usize) -> SupplyShare {
    if demand <= 0.0 {
        return SupplyShare {
            served: 0.0,
            supply: if sites > 0 { 1.0 } else { 0.0 },
            coverage: if sites > 0 { 1.0 } else { 0.0 },
        };
    }
    let served = capacity.min(covered_demand).max(0.0);
    SupplyShare {
        served,
        supply: (served / demand).clamp(0.0, 1.0) as f32,
        coverage: (covered_demand / demand).clamp(0.0, 1.0) as f32,
    }
}

#[derive(Debug, Clone, Copy)]
struct SupplyShare {
    served: f64,
    supply: f32,
    coverage: f32,
}

/// Errors returned by the utility store.
#[derive(Debug)]
pub enum UtilityError {
    /// JSON parse/serialize error.
    Json(serde_json::Error),
    /// Unsupported save version.
    UnsupportedVersion(u16),
    /// A range or state invariant failed.
    Invalid(&'static str),
    /// Too many utility sites for one save.
    TooManySites(usize),
}

impl fmt::Display for UtilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "utilities JSON error: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported utilities version {version}")
            }
            Self::Invalid(message) => write!(formatter, "invalid utilities: {message}"),
            Self::TooManySites(count) => {
                write!(
                    formatter,
                    "utilities have {count} sites; limit is {MAX_UTILITY_SITES}"
                )
            }
        }
    }
}

impl std::error::Error for UtilityError {}

impl From<serde_json::Error> for UtilityError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn consumer(id: u64, residents: u32, workers: u32, x: f32, z: f32) -> UtilityConsumer {
        UtilityConsumer {
            id,
            center: [x, z],
            residents,
            workers,
        }
    }

    #[test]
    fn an_empty_store_has_no_service() {
        let mut store = UtilityStore::new(2026);
        let report = store.refresh(&[consumer(1, 4, 2, 0.0, 0.0)]);
        assert_eq!(report.water_supply, 0.0);
        assert_eq!(report.power_supply, 0.0);
        assert_eq!(report.water_demand, 4.0);
        assert_eq!(report.power_demand, 3.0);
        assert_eq!(report.water_sites, 0);
        assert_eq!(report.power_sites, 0);
    }

    #[test]
    fn a_nearby_site_covers_a_home() {
        let mut store = UtilityStore::new(2026);
        store
            .add_site(1, UtilityKind::Water, [0.0, 0.0, 0.0])
            .expect("site");
        let report = store.refresh(&[consumer(2, 4, 0, 10.0, 0.0)]);
        assert_eq!(report.water_supply, 1.0);
        assert_eq!(report.water_served, 4.0);
        assert_eq!(report.water_covered_buildings, 1);
    }

    #[test]
    fn capacity_shortage_reduces_service() {
        let mut store = UtilityStore::new(2026);
        store
            .add_site(1, UtilityKind::Water, [0.0, 0.0, 0.0])
            .expect("site");
        store.water_sites.get_mut(&1).expect("site").capacity = 2.0;
        let report = store.refresh(&[consumer(2, 4, 0, 10.0, 0.0)]);
        assert_eq!(report.water_supply, 0.5);
        assert_eq!(report.water_served, 2.0);
        assert_eq!(report.water_coverage, 1.0);
    }

    #[test]
    fn an_out_of_range_building_is_not_covered() {
        let mut store = UtilityStore::new(2026);
        store
            .add_site(1, UtilityKind::Water, [0.0, 0.0, 0.0])
            .expect("site");
        let report = store.refresh(&[consumer(2, 4, 0, 200.0, 0.0)]);
        assert_eq!(report.water_supply, 0.0);
        assert_eq!(report.water_coverage, 0.0);
        assert_eq!(report.water_covered_buildings, 0);
    }

    #[test]
    fn overlapping_sites_do_not_double_count_demand() {
        let mut store = UtilityStore::new(2026);
        store
            .add_site(1, UtilityKind::Water, [0.0, 0.0, 0.0])
            .expect("site");
        store
            .add_site(2, UtilityKind::Water, [10.0, 0.0, 0.0])
            .expect("site");
        store.water_sites.get_mut(&1).expect("site").capacity = 2.0;
        store.water_sites.get_mut(&2).expect("site").capacity = 2.0;
        let report = store.refresh(&[consumer(3, 4, 0, 10.0, 0.0)]);
        assert_eq!(report.water_supply, 1.0);
        assert_eq!(report.water_served, 4.0);
    }

    #[test]
    fn power_demand_uses_workers_and_residents() {
        let mut store = UtilityStore::new(2026);
        store
            .add_site(1, UtilityKind::Power, [0.0, 0.0, 0.0])
            .expect("site");
        let report = store.refresh(&[consumer(2, 4, 2, 10.0, 0.0)]);
        assert_eq!(report.power_demand, 3.0);
        assert_eq!(report.power_supply, 1.0);
        assert_eq!(report.water_supply, 0.0);
    }

    #[test]
    fn a_consumer_can_have_water_and_power_on_one_placement() {
        let mut store = UtilityStore::new(2026);
        store
            .add_site(1, UtilityKind::Water, [0.0, 0.0, 0.0])
            .expect("water");
        store
            .add_site(1, UtilityKind::Power, [0.0, 0.0, 0.0])
            .expect("power");
        assert_eq!(store.site_count(), 2);
        assert_eq!(store.count_of(UtilityKind::Water), 1);
        assert_eq!(store.count_of(UtilityKind::Power), 1);
        assert!(store.remove_site(1, UtilityKind::Water));
        assert!(!store.remove_site(1, UtilityKind::Water));
        assert!(store.remove_placement(1));
        assert_eq!(store.site_count(), 0);
    }

    #[test]
    fn zero_demand_with_sites_reports_full_service() {
        let mut store = UtilityStore::new(2026);
        store
            .add_site(1, UtilityKind::Water, [0.0, 0.0, 0.0])
            .expect("site");
        let report = store.refresh(&[]);
        assert_eq!(report.water_supply, 1.0);
        assert_eq!(report.power_supply, 0.0);
    }

    #[test]
    fn utility_costs_are_stable() {
        assert_eq!(UtilityKind::Water.construction_cost(), 2_500.0);
        assert_eq!(UtilityKind::Power.construction_cost(), 5_000.0);
        assert_eq!(UtilityKind::Water.upkeep_per_day(), 2.0);
        assert_eq!(UtilityKind::Power.upkeep_per_day(), 3.0);
        let mut store = UtilityStore::new(2026);
        store
            .add_site(1, UtilityKind::Water, [1.0, 2.0, 3.0])
            .expect("water");
        store
            .add_site(2, UtilityKind::Power, [4.0, 5.0, 6.0])
            .expect("power");
        assert_eq!(store.total_construction_cost(), 7_500.0);
        assert_eq!(store.total_upkeep_per_day(), 5.0);
        assert_eq!(store.construction_cost_of(UtilityKind::Water), 2_500.0);
        assert_eq!(store.upkeep_per_day_of(UtilityKind::Power), 3.0);
    }

    #[test]
    fn json_round_trip_and_checksum() {
        let mut store = UtilityStore::new(2026);
        store
            .add_site(1, UtilityKind::Water, [0.0, 0.0, 0.0])
            .expect("site");
        store
            .add_site(1, UtilityKind::Power, [0.0, 0.0, 0.0])
            .expect("site");
        store.refresh(&[consumer(1, 2, 2, 0.0, 0.0)]);
        let json = store.to_json_pretty().expect("json");
        let decoded = UtilityStore::from_json(&json).expect("decode");
        assert_eq!(decoded, store);
        assert_eq!(decoded.checksum(), store.checksum());
    }

    #[test]
    fn invalid_store_is_rejected() {
        let mut store = UtilityStore::new(2026);
        store.water_supply = f32::NAN;
        assert!(store.validate().is_err());
        let mut store = UtilityStore::new(2026);
        store
            .add_site(1, UtilityKind::Water, [0.0, 0.0, 0.0])
            .expect("site");
        store.water_sites.get_mut(&1).expect("site").radius_m = -1.0;
        assert!(store.validate().is_err());
    }

    #[test]
    fn kind_parse_accepts_aliases() {
        assert_eq!(UtilityKind::parse("water"), Some(UtilityKind::Water));
        assert_eq!(UtilityKind::parse("Well"), Some(UtilityKind::Water));
        assert_eq!(UtilityKind::parse("power_plant"), Some(UtilityKind::Power));
        assert_eq!(UtilityKind::parse("grid"), Some(UtilityKind::Power));
        assert_eq!(UtilityKind::parse("sewage"), None);
        assert_eq!(UtilityKind::Water.as_str(), "water");
        assert_eq!(UtilityKind::Power.as_str(), "power");
    }
}
