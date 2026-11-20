//! Primary industries: lumberjacks, farms, and hunting lodges.
//!
//! Phase 3.4 turns completed workplaces into resource producers. An industry
//! site is a placement id plus a kind; staffing comes from the household
//! runtime, produced goods go into a city stockpile, and residents eat from
//! the food stock. It is deliberately aggregate: one site per placement, no
//! individual workers, vehicles, or storage buildings yet.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Current industry save format.
pub const INDUSTRY_FORMAT_VERSION: u16 = 1;

/// Seconds in one game day.
pub const DAY_SECONDS: f64 = 86_400.0;

/// Maximum industry sites in one save, a guard against malformed input.
pub const MAX_INDUSTRY_SITES: usize = 10_000;

/// Maximum stockpiled units per resource.
pub const MAX_STOCK: f64 = 1_000_000_000_000.0;

/// Upper bound for one industry step.
pub const MAX_INDUSTRY_TICK_SECONDS: f64 = 86_400.0;

/// Wood produced by one lumberjack worker per game day.
pub const WOOD_PER_LUMBERJACK_WORKER_PER_DAY: f64 = 8.0;

/// Food produced by one farm worker per game day.
pub const FOOD_PER_FARM_WORKER_PER_DAY: f64 = 12.0;

/// Food produced by one hunter worker per game day.
pub const FOOD_PER_HUNTER_WORKER_PER_DAY: f64 = 6.0;

/// Hides produced by one hunter worker per game day.
pub const HIDES_PER_HUNTER_WORKER_PER_DAY: f64 = 2.0;

/// Food eaten by one resident per game day.
pub const FOOD_PER_RESIDENT_PER_DAY: f64 = 1.0;

/// One primary industry kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndustryKind {
    /// Fells trees and produces wood.
    Lumberjack,
    /// Grows crops and produces food.
    Farm,
    /// Hunts game for food and hides.
    Hunt,
}

impl IndustryKind {
    /// Every industry kind, in display order.
    pub const ALL: [Self; 3] = [Self::Lumberjack, Self::Farm, Self::Hunt];

    /// Stable lowercase identifier.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lumberjack => "lumberjack",
            Self::Farm => "farm",
            Self::Hunt => "hunt",
        }
    }

    /// Parse a stable identifier.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "lumberjack" | "lumberyard" | "woodcutter" => Some(Self::Lumberjack),
            "farm" | "farmer" => Some(Self::Farm),
            "hunt" | "hunter" | "hunting_lodge" => Some(Self::Hunt),
            _ => None,
        }
    }

    /// `(wood, food, hides)` produced by one worker per game day.
    #[must_use]
    pub fn output_per_worker_day(self) -> (f64, f64, f64) {
        match self {
            Self::Lumberjack => (WOOD_PER_LUMBERJACK_WORKER_PER_DAY, 0.0, 0.0),
            Self::Farm => (0.0, FOOD_PER_FARM_WORKER_PER_DAY, 0.0),
            Self::Hunt => (
                0.0,
                FOOD_PER_HUNTER_WORKER_PER_DAY,
                HIDES_PER_HUNTER_WORKER_PER_DAY,
            ),
        }
    }
}

/// A quantity of each primary resource.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Stockpile {
    /// Timber available for construction and trade.
    #[serde(default)]
    pub wood: f64,
    /// Food available for residents.
    #[serde(default)]
    pub food: f64,
    /// Hides available for future crafting.
    #[serde(default)]
    pub hides: f64,
}

/// One industry site on a completed or planned placement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndustrySite {
    /// Industry kind.
    pub kind: IndustryKind,
    /// Lifetime combined output from this site in mixed resource units
    /// (wood + food + hides). Informational only.
    #[serde(default)]
    pub produced: f64,
}

/// Deterministic primary-industry runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndustryStore {
    /// Save format version.
    pub format_version: u16,
    /// World seed this store belongs to.
    pub seed: u64,
    /// Sites keyed by placement id.
    pub sites: BTreeMap<u64, IndustrySite>,
    /// Current city stockpile.
    #[serde(default)]
    pub stockpile: Stockpile,
    /// Lifetime city production.
    #[serde(default)]
    pub produced_total: Stockpile,
    /// Food satisfaction supplied to households, 0..=1.
    #[serde(default)]
    pub food_supply: f32,
}

impl IndustryStore {
    /// Create an empty industry store for a seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            format_version: INDUSTRY_FORMAT_VERSION,
            seed,
            sites: BTreeMap::new(),
            stockpile: Stockpile::default(),
            produced_total: Stockpile::default(),
            food_supply: 0.0,
        }
    }

    /// Number of industry sites.
    #[must_use]
    pub fn site_count(&self) -> usize {
        self.sites.len()
    }

    /// One site by placement id.
    #[must_use]
    pub fn site(&self, placement_id: u64) -> Option<&IndustrySite> {
        self.sites.get(&placement_id)
    }

    /// Add or replace an industry site. Replacing keeps the stockpile.
    pub fn add_site(&mut self, placement_id: u64, kind: IndustryKind) -> Result<(), IndustryError> {
        if placement_id > i64::MAX as u64 {
            return Err(IndustryError::Invalid("site id is above i64::MAX"));
        }
        if !self.sites.contains_key(&placement_id) && self.sites.len() >= MAX_INDUSTRY_SITES {
            return Err(IndustryError::TooManySites(self.sites.len()));
        }
        self.sites.insert(
            placement_id,
            IndustrySite {
                kind,
                produced: 0.0,
            },
        );
        Ok(())
    }

    /// Remove one site.
    pub fn remove_site(&mut self, placement_id: u64) -> bool {
        self.sites.remove(&placement_id).is_some()
    }

    /// Remove every site and reset stocks and production.
    pub fn clear(&mut self) {
        self.sites.clear();
        self.stockpile = Stockpile::default();
        self.produced_total = Stockpile::default();
        self.food_supply = 0.0;
    }

    /// Advance production and food consumption by `dt_s` seconds.
    ///
    /// `staffing` is the per-workplace worker count from the household
    /// runtime. Production happens before residents eat, so a harvest in the
    /// same tick can feed the city.
    pub fn tick(
        &mut self,
        dt_s: f64,
        staffing: &BTreeMap<u64, u32>,
        population: u64,
    ) -> IndustryTick {
        let dt = if dt_s.is_finite() {
            dt_s.clamp(0.0, MAX_INDUSTRY_TICK_SECONDS)
        } else {
            0.0
        };
        let days = dt / DAY_SECONDS;
        let mut wood = 0.0;
        let mut food = 0.0;
        let mut hides = 0.0;
        let mut workers = 0_u64;
        for (&id, site) in &mut self.sites {
            let staffed = staffing.get(&id).copied().unwrap_or(0).min(1_000_000);
            if staffed == 0 {
                continue;
            }
            workers += u64::from(staffed);
            let (site_wood, site_food, site_hides) = site.kind.output_per_worker_day();
            let worker_days = f64::from(staffed) * days;
            wood += site_wood * worker_days;
            food += site_food * worker_days;
            hides += site_hides * worker_days;
            site.produced = (site.produced
                + site_wood * worker_days
                + site_food * worker_days
                + site_hides * worker_days)
                .min(MAX_STOCK);
        }
        self.stockpile.wood = (self.stockpile.wood + wood).min(MAX_STOCK);
        self.stockpile.food = (self.stockpile.food + food).min(MAX_STOCK);
        self.stockpile.hides = (self.stockpile.hides + hides).min(MAX_STOCK);
        self.produced_total.wood = (self.produced_total.wood + wood).min(MAX_STOCK);
        self.produced_total.food = (self.produced_total.food + food).min(MAX_STOCK);
        self.produced_total.hides = (self.produced_total.hides + hides).min(MAX_STOCK);

        let demand = population as f64 * FOOD_PER_RESIDENT_PER_DAY * days;
        let consumed = demand.min(self.stockpile.food).max(0.0);
        self.stockpile.food -= consumed;
        self.food_supply = if demand > 0.0 {
            (consumed / demand).clamp(0.0, 1.0) as f32
        } else if self.stockpile.food > 0.0 {
            1.0
        } else {
            0.0
        };

        IndustryTick {
            wood,
            food,
            hides,
            food_consumed: consumed,
            food_supply: self.food_supply,
            workers,
            stockpile: self.stockpile,
        }
    }

    /// Serialize as pretty JSON.
    pub fn to_json_pretty(&self) -> Result<String, IndustryError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse and validate an industry save.
    pub fn from_json(json: &str) -> Result<Self, IndustryError> {
        let store: Self = serde_json::from_str(json)?;
        store.validate()?;
        Ok(store)
    }

    /// Validate all ranges and state invariants.
    pub fn validate(&self) -> Result<(), IndustryError> {
        if self.format_version != INDUSTRY_FORMAT_VERSION {
            return Err(IndustryError::UnsupportedVersion(self.format_version));
        }
        if self.sites.len() > MAX_INDUSTRY_SITES {
            return Err(IndustryError::TooManySites(self.sites.len()));
        }
        if !self.food_supply.is_finite() || !(0.0..=1.0).contains(&self.food_supply) {
            return Err(IndustryError::Invalid("food supply is out of range"));
        }
        for stock in [self.stockpile, self.produced_total] {
            for (name, value) in [
                ("wood", stock.wood),
                ("food", stock.food),
                ("hides", stock.hides),
            ] {
                if !value.is_finite() || !(0.0..=MAX_STOCK).contains(&value) {
                    return Err(IndustryError::Invalid(name));
                }
            }
        }
        for (&id, site) in &self.sites {
            if id > i64::MAX as u64 {
                return Err(IndustryError::Invalid("site id is above i64::MAX"));
            }
            if !site.produced.is_finite() || !(0.0..=MAX_STOCK).contains(&site.produced) {
                return Err(IndustryError::Invalid("site production"));
            }
        }
        Ok(())
    }

    /// FNV-1a checksum over the compact JSON.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let json = serde_json::to_vec(self).expect("industry always serializes");
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in json {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
}

/// Result of one industry tick.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct IndustryTick {
    /// Wood produced this tick.
    pub wood: f64,
    /// Food produced this tick.
    pub food: f64,
    /// Hides produced this tick.
    pub hides: f64,
    /// Food eaten by residents this tick.
    pub food_consumed: f64,
    /// Food satisfaction supplied to households, 0..=1.
    pub food_supply: f32,
    /// Staffed industry workers this tick.
    pub workers: u64,
    /// Stockpile after the tick.
    pub stockpile: Stockpile,
}

/// Errors returned by the industry store.
#[derive(Debug)]
pub enum IndustryError {
    /// JSON parse/serialize error.
    Json(serde_json::Error),
    /// Unsupported save version.
    UnsupportedVersion(u16),
    /// A range or state invariant failed.
    Invalid(&'static str),
    /// Too many industry sites for one save.
    TooManySites(usize),
}

impl fmt::Display for IndustryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "industry JSON error: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported industry version {version}")
            }
            Self::Invalid(message) => write!(formatter, "invalid industry: {message}"),
            Self::TooManySites(count) => write!(
                formatter,
                "industry has {count} sites; limit is {MAX_INDUSTRY_SITES}"
            ),
        }
    }
}

impl std::error::Error for IndustryError {}

impl From<serde_json::Error> for IndustryError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staffing(entries: &[(u64, u32)]) -> BTreeMap<u64, u32> {
        entries.iter().copied().collect()
    }

    #[test]
    fn new_store_is_empty() {
        let store = IndustryStore::new(2026);
        assert_eq!(store.site_count(), 0);
        assert_eq!(store.stockpile, Stockpile::default());
        assert_eq!(store.food_supply, 0.0);
    }

    #[test]
    fn each_kind_produces_its_expected_resource() {
        let mut store = IndustryStore::new(2026);
        store.add_site(1, IndustryKind::Lumberjack).expect("site");
        store.add_site(2, IndustryKind::Farm).expect("site");
        store.add_site(3, IndustryKind::Hunt).expect("site");
        let tick = store.tick(DAY_SECONDS, &staffing(&[(1, 2), (2, 3), (3, 4)]), 0);
        assert!((tick.wood - 16.0).abs() < 0.001);
        assert!((tick.food - 60.0).abs() < 0.001);
        assert!((tick.hides - 8.0).abs() < 0.001);
        assert_eq!(tick.workers, 9);
        assert!((store.stockpile.wood - 16.0).abs() < 0.001);
        assert!((store.stockpile.food - 60.0).abs() < 0.001);
        assert!((store.stockpile.hides - 8.0).abs() < 0.001);
    }

    #[test]
    fn production_scales_with_time_and_staffing() {
        let mut store = IndustryStore::new(2026);
        store.add_site(1, IndustryKind::Farm).expect("site");
        let half_day = store.tick(DAY_SECONDS / 2.0, &staffing(&[(1, 10)]), 0);
        assert!((half_day.food - 60.0).abs() < 0.001);
        let quarter_day = store.tick(DAY_SECONDS / 4.0, &staffing(&[(1, 20)]), 0);
        assert!((quarter_day.food - 60.0).abs() < 0.001);
    }

    #[test]
    fn residents_eat_and_full_supply_is_reported() {
        let mut store = IndustryStore::new(2026);
        store.add_site(1, IndustryKind::Farm).expect("site");
        let tick = store.tick(DAY_SECONDS, &staffing(&[(1, 50)]), 100);
        assert!((tick.food - 600.0).abs() < 0.001);
        assert!((tick.food_consumed - 100.0).abs() < 0.001);
        assert!((tick.food_supply - 1.0).abs() < 0.001);
        assert!((store.stockpile.food - 500.0).abs() < 0.001);
    }

    #[test]
    fn a_shortage_reports_a_partial_supply_ratio() {
        let mut store = IndustryStore::new(2026);
        store.stockpile.food = 25.0;
        let tick = store.tick(DAY_SECONDS, &BTreeMap::new(), 100);
        assert!((tick.food_consumed - 25.0).abs() < 0.001);
        assert!((tick.food_supply - 0.25).abs() < 0.001);
        assert_eq!(store.stockpile.food, 0.0);
    }

    #[test]
    fn an_empty_stock_with_no_population_reports_no_supply() {
        let mut store = IndustryStore::new(2026);
        let tick = store.tick(DAY_SECONDS, &BTreeMap::new(), 0);
        assert_eq!(tick.food_supply, 0.0);
    }

    #[test]
    fn no_population_needs_no_food() {
        let mut store = IndustryStore::new(2026);
        store.stockpile.food = 10.0;
        let tick = store.tick(DAY_SECONDS, &BTreeMap::new(), 0);
        assert_eq!(tick.food_consumed, 0.0);
        assert_eq!(tick.food_supply, 1.0);
        assert_eq!(store.stockpile.food, 10.0);
    }

    #[test]
    fn remove_and_clear_are_deterministic() {
        let mut store = IndustryStore::new(2026);
        store.add_site(1, IndustryKind::Farm).expect("site");
        store.add_site(2, IndustryKind::Hunt).expect("site");
        assert!(store.remove_site(1));
        assert!(!store.remove_site(1));
        assert_eq!(store.site_count(), 1);
        store.clear();
        assert_eq!(store.site_count(), 0);
        assert_eq!(store.stockpile, Stockpile::default());
        assert_eq!(store.food_supply, 0.0);
    }

    #[test]
    fn json_round_trip_and_checksum() {
        let mut store = IndustryStore::new(2026);
        store.add_site(1, IndustryKind::Farm).expect("site");
        store.tick(DAY_SECONDS, &staffing(&[(1, 10)]), 20);
        let json = store.to_json_pretty().expect("json");
        let decoded = IndustryStore::from_json(&json).expect("decode");
        assert_eq!(decoded, store);
        assert_eq!(decoded.checksum(), store.checksum());
    }

    #[test]
    fn invalid_stock_is_rejected() {
        let mut store = IndustryStore::new(2026);
        store.stockpile.wood = f64::NAN;
        assert!(store.validate().is_err());
        let mut store = IndustryStore::new(2026);
        store.food_supply = 2.0;
        assert!(store.validate().is_err());
    }

    #[test]
    fn kind_identifiers_parse_and_round_trip() {
        for kind in IndustryKind::ALL {
            assert_eq!(IndustryKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(IndustryKind::parse(" Farmer "), Some(IndustryKind::Farm));
        assert_eq!(IndustryKind::parse("mine"), None);
    }
}
