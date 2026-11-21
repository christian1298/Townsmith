//! Civic services: schools, clinics, fire, and police coverage.
//!
//! Phase 4.4 adds a city-wide service layer on top of utilities and industry.
//! A service site is a placement id plus a kind and tier; completed buildings
//! consume service when they are within a site's radius. Supply is the served
//! share of demand, so capacity shortages and uncovered buildings both reduce
//! service. Schools also produce research points.
//!
//! This is deliberately aggregate: no individual students, patients, staff,
//! response times, or service vehicles yet. The core layer is engine-agnostic.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::progression::Age;

/// Current civic service save format.
pub const SERVICES_FORMAT_VERSION: u16 = 1;

/// Maximum civic service sites in one save.
pub const MAX_SERVICE_SITES: usize = 10_000;

/// Largest service radius accepted from a save.
pub const MAX_SERVICE_RADIUS_M: f32 = 10_000.0;

/// Largest installed capacity accepted from a save.
pub const MAX_SERVICE_CAPACITY: f64 = 1_000_000_000.0;

/// Largest school research output accepted from a save.
pub const MAX_SERVICE_RESEARCH_PER_DAY: f64 = 1_000_000.0;

/// Share of residents counted as school demand.
pub const SCHOOL_DEMAND_PER_RESIDENT: f64 = 0.25;

/// Workers count as half a resident for clinic, fire, and police demand.
pub const WORKER_SERVICE_DEMAND: f64 = 0.5;

/// One civic service kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceKind {
    /// Schools and, later, universities.
    School,
    /// Clinics and, later, hospitals.
    Clinic,
    /// Fire stations and response coverage.
    Fire,
    /// Police stations and response coverage.
    Police,
}

impl ServiceKind {
    /// Every service kind, in display order.
    pub const ALL: [Self; 4] = [Self::School, Self::Clinic, Self::Fire, Self::Police];

    /// Stable lowercase identifier.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::School => "school",
            Self::Clinic => "clinic",
            Self::Fire => "fire",
            Self::Police => "police",
        }
    }

    /// Human-readable name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::School => "school",
            Self::Clinic => "clinic",
            Self::Fire => "fire station",
            Self::Police => "police station",
        }
    }

    /// Parse a stable identifier.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "school" | "education" | "academy" => Some(Self::School),
            "clinic" | "health" | "hospital" => Some(Self::Clinic),
            "fire" | "fire_station" | "firehouse" => Some(Self::Fire),
            "police" | "police_station" | "constabulary" => Some(Self::Police),
            _ => None,
        }
    }

    /// Whether this kind produces research points.
    #[must_use]
    pub fn produces_research(self) -> bool {
        self == Self::School
    }
}

/// One civic service tier, unlocked by city age.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceTier {
    /// Wood-age civic buildings.
    Basic,
    /// Masonry and industrial civic buildings.
    Improved,
    /// Modern and information-age civic buildings.
    Advanced,
}

impl ServiceTier {
    /// Every tier, in display order.
    pub const ALL: [Self; 3] = [Self::Basic, Self::Improved, Self::Advanced];

    /// Stable lowercase identifier.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Basic => "basic",
            Self::Improved => "improved",
            Self::Advanced => "advanced",
        }
    }

    /// Parse a stable identifier.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "basic" | "wood" => Some(Self::Basic),
            "improved" | "masonry" | "industrial" => Some(Self::Improved),
            "advanced" | "modern" | "information" => Some(Self::Advanced),
            _ => None,
        }
    }

    /// Tier available at an age.
    #[must_use]
    pub fn from_age(age: Age) -> Self {
        match age {
            Age::Wood => Self::Basic,
            Age::Masonry | Age::Industrial => Self::Improved,
            Age::Modern | Age::Information => Self::Advanced,
        }
    }

    /// Research multiplier for a school of this tier.
    #[must_use]
    pub fn research_per_day(self) -> f64 {
        match self {
            Self::Basic => 8.0,
            Self::Improved => 20.0,
            Self::Advanced => 50.0,
        }
    }
}

/// One installed civic service site.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ServiceSite {
    /// Service kind.
    pub kind: ServiceKind,
    /// Service tier.
    pub tier: ServiceTier,
    /// Site centre in metres.
    #[serde(default)]
    pub center: [f32; 3],
    /// Service radius in metres.
    pub radius_m: f32,
    /// Installed capacity in demand units.
    pub capacity: f64,
    /// Research produced per game day when the site covers residents.
    pub research_per_day: f64,
}

impl ServiceSite {
    /// Default site for a kind and tier.
    #[must_use]
    pub fn default_for(kind: ServiceKind, tier: ServiceTier) -> Self {
        let (radius_m, capacity) = match (kind, tier) {
            (ServiceKind::School, ServiceTier::Basic) => (180.0, 200.0),
            (ServiceKind::School, ServiceTier::Improved) => (240.0, 400.0),
            (ServiceKind::School, ServiceTier::Advanced) => (320.0, 800.0),
            (ServiceKind::Clinic, ServiceTier::Basic) => (180.0, 300.0),
            (ServiceKind::Clinic, ServiceTier::Improved) => (240.0, 600.0),
            (ServiceKind::Clinic, ServiceTier::Advanced) => (320.0, 1_200.0),
            (ServiceKind::Fire, ServiceTier::Basic) => (240.0, 400.0),
            (ServiceKind::Fire, ServiceTier::Improved) => (300.0, 800.0),
            (ServiceKind::Fire, ServiceTier::Advanced) => (400.0, 1_600.0),
            (ServiceKind::Police, ServiceTier::Basic) => (240.0, 400.0),
            (ServiceKind::Police, ServiceTier::Improved) => (300.0, 800.0),
            (ServiceKind::Police, ServiceTier::Advanced) => (400.0, 1_600.0),
        };
        let research_per_day = if kind.produces_research() {
            tier.research_per_day()
        } else {
            0.0
        };
        Self {
            kind,
            tier,
            center: [0.0, 0.0, 0.0],
            radius_m,
            capacity,
            research_per_day,
        }
    }
}

/// One completed building that demands civic service.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServiceConsumer {
    /// Placement id.
    pub id: u64,
    /// World position in metres.
    pub center: [f32; 2],
    /// Residents currently living here.
    pub residents: u32,
    /// Workers assigned here in the last household tick.
    pub workers: u32,
}

impl ServiceConsumer {
    /// School demand in residents.
    #[must_use]
    pub fn school_demand(&self) -> f64 {
        f64::from(self.residents) * SCHOOL_DEMAND_PER_RESIDENT
    }

    /// Clinic, fire, and police demand from residents and workers.
    #[must_use]
    pub fn civic_demand(&self) -> f64 {
        f64::from(self.residents) + f64::from(self.workers) * WORKER_SERVICE_DEMAND
    }

    /// Demand for one service kind.
    #[must_use]
    pub fn demand(&self, kind: ServiceKind) -> f64 {
        if kind == ServiceKind::School {
            self.school_demand()
        } else {
            self.civic_demand()
        }
    }
}

/// Deterministic civic service runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceStore {
    /// Save format version.
    pub format_version: u16,
    /// World seed this store belongs to.
    pub seed: u64,
    /// Service sites keyed by placement id.
    #[serde(default)]
    pub sites: BTreeMap<u64, ServiceSite>,
    /// School service share from the last refresh, 0..=1.
    #[serde(default)]
    pub school_supply: f32,
    /// Clinic service share from the last refresh, 0..=1.
    #[serde(default)]
    pub clinic_supply: f32,
    /// Fire service share from the last refresh, 0..=1.
    #[serde(default)]
    pub fire_supply: f32,
    /// Police service share from the last refresh, 0..=1.
    #[serde(default)]
    pub police_supply: f32,
    /// Research produced per game day by covered schools.
    #[serde(default)]
    pub research_per_day: f64,
}

impl ServiceStore {
    /// Create an empty service store for a seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            format_version: SERVICES_FORMAT_VERSION,
            seed,
            sites: BTreeMap::new(),
            school_supply: 0.0,
            clinic_supply: 0.0,
            fire_supply: 0.0,
            police_supply: 0.0,
            research_per_day: 0.0,
        }
    }

    /// Total sites.
    #[must_use]
    pub fn site_count(&self) -> usize {
        self.sites.len()
    }

    /// Number of sites of one kind.
    #[must_use]
    pub fn count_of(&self, kind: ServiceKind) -> usize {
        self.sites.values().filter(|site| site.kind == kind).count()
    }

    /// Sites of one kind.
    pub fn sites_of(&self, kind: ServiceKind) -> impl Iterator<Item = (&u64, &ServiceSite)> {
        self.sites.iter().filter(move |(_, site)| site.kind == kind)
    }

    /// Add or replace one service site.
    pub fn add_site(
        &mut self,
        placement_id: u64,
        kind: ServiceKind,
        tier: ServiceTier,
        center: [f32; 3],
    ) -> Result<(), ServiceError> {
        if placement_id > i64::MAX as u64 {
            return Err(ServiceError::Invalid("site id is above i64::MAX"));
        }
        if !self.sites.contains_key(&placement_id) && self.sites.len() >= MAX_SERVICE_SITES {
            return Err(ServiceError::TooManySites(self.sites.len()));
        }
        let mut site = ServiceSite::default_for(kind, tier);
        site.center = center;
        self.sites.insert(placement_id, site);
        Ok(())
    }

    /// Remove one site by placement id.
    pub fn remove_site(&mut self, placement_id: u64) -> bool {
        self.sites.remove(&placement_id).is_some()
    }

    /// Remove a placement's site. Alias for [`Self::remove_site`].
    pub fn remove_placement(&mut self, placement_id: u64) -> bool {
        self.remove_site(placement_id)
    }

    /// Remove every site and reset service.
    pub fn clear(&mut self) {
        self.sites.clear();
        self.school_supply = 0.0;
        self.clinic_supply = 0.0;
        self.fire_supply = 0.0;
        self.police_supply = 0.0;
        self.research_per_day = 0.0;
    }

    /// Recompute service coverage from completed consumers.
    ///
    /// Demand is counted for every consumer. A consumer is covered when at
    /// least one site of that kind is within its radius. Served demand is
    /// `min(total capacity, covered demand)`, so uncovered demand also lowers
    /// the city-wide service share. Schools add research when they cover at
    /// least one consumer with school demand.
    pub fn refresh(&mut self, consumers: &[ServiceConsumer]) -> ServiceReport {
        let mut report = ServiceReport {
            consumers: consumers.len(),
            sites: self.sites.len(),
            ..ServiceReport::default()
        };
        for kind in ServiceKind::ALL {
            let demand: f64 = consumers.iter().map(|consumer| consumer.demand(kind)).sum();
            let capacity: f64 = self
                .sites
                .values()
                .filter(|site| site.kind == kind)
                .map(|site| site.capacity)
                .sum();
            let covered_demand: f64 = consumers
                .iter()
                .filter(|consumer| covered(consumer, kind, &self.sites))
                .map(|consumer| consumer.demand(kind))
                .sum();
            let service = supply_share(demand, covered_demand, capacity, self.count_of(kind));
            match kind {
                ServiceKind::School => {
                    self.school_supply = service.supply;
                    report.school_supply = service.supply;
                    report.school_demand = demand;
                    report.school_served = service.served;
                    report.school_capacity = capacity;
                    report.school_coverage = service.coverage;
                }
                ServiceKind::Clinic => {
                    self.clinic_supply = service.supply;
                    report.clinic_supply = service.supply;
                    report.clinic_demand = demand;
                    report.clinic_served = service.served;
                    report.clinic_capacity = capacity;
                    report.clinic_coverage = service.coverage;
                }
                ServiceKind::Fire => {
                    self.fire_supply = service.supply;
                    report.fire_supply = service.supply;
                    report.fire_demand = demand;
                    report.fire_served = service.served;
                    report.fire_capacity = capacity;
                    report.fire_coverage = service.coverage;
                }
                ServiceKind::Police => {
                    self.police_supply = service.supply;
                    report.police_supply = service.supply;
                    report.police_demand = demand;
                    report.police_served = service.served;
                    report.police_capacity = capacity;
                    report.police_coverage = service.coverage;
                }
            }
            report.sites_of[kind as usize] = self.count_of(kind);
            report.covered_buildings[kind as usize] = consumers
                .iter()
                .filter(|consumer| {
                    consumer.demand(kind) > 0.0 && covered(consumer, kind, &self.sites)
                })
                .count();
        }

        // Allocate the served school demand across sites in id order. Flat
        // per-site output is only counted for a site that receives part of the
        // served demand, so overlapping schools cannot stack research.
        let mut remaining = report.school_served;
        let mut research = 0.0_f64;
        for site in self
            .sites
            .values()
            .filter(|site| site.kind == ServiceKind::School)
        {
            if remaining <= 0.0 || site.capacity <= 0.0 {
                continue;
            }
            let covered_demand: f64 = consumers
                .iter()
                .filter(|consumer| consumer.school_demand() > 0.0 && site_covers(site, consumer))
                .map(ServiceConsumer::school_demand)
                .sum();
            let allocated = covered_demand.min(site.capacity).min(remaining);
            if allocated > 0.0 {
                research += site.research_per_day;
                remaining -= allocated;
            }
        }
        self.research_per_day = research.min(MAX_SERVICE_RESEARCH_PER_DAY);
        report.research_per_day = self.research_per_day;
        report
    }

    /// Serialize as pretty JSON.
    pub fn to_json_pretty(&self) -> Result<String, ServiceError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse and validate a service save.
    pub fn from_json(json: &str) -> Result<Self, ServiceError> {
        let store: Self = serde_json::from_str(json)?;
        store.validate()?;
        Ok(store)
    }

    /// Validate all ranges and state invariants.
    pub fn validate(&self) -> Result<(), ServiceError> {
        if self.format_version != SERVICES_FORMAT_VERSION {
            return Err(ServiceError::UnsupportedVersion(self.format_version));
        }
        if self.sites.len() > MAX_SERVICE_SITES {
            return Err(ServiceError::TooManySites(self.sites.len()));
        }
        for (name, supply) in [
            ("school supply", self.school_supply),
            ("clinic supply", self.clinic_supply),
            ("fire supply", self.fire_supply),
            ("police supply", self.police_supply),
        ] {
            if !supply.is_finite() || !(0.0..=1.0).contains(&supply) {
                return Err(ServiceError::Invalid(name));
            }
        }
        if !self.research_per_day.is_finite()
            || !(0.0..=MAX_SERVICE_RESEARCH_PER_DAY).contains(&self.research_per_day)
        {
            return Err(ServiceError::Invalid("research output"));
        }
        for (&id, site) in &self.sites {
            if id > i64::MAX as u64 {
                return Err(ServiceError::Invalid("site id is above i64::MAX"));
            }
            if site.center.iter().any(|value| !value.is_finite()) {
                return Err(ServiceError::Invalid("site centre"));
            }
            if !site.radius_m.is_finite() || !(0.0..=MAX_SERVICE_RADIUS_M).contains(&site.radius_m)
            {
                return Err(ServiceError::Invalid("site radius"));
            }
            if !site.capacity.is_finite() || !(0.0..=MAX_SERVICE_CAPACITY).contains(&site.capacity)
            {
                return Err(ServiceError::Invalid("site capacity"));
            }
            if !site.research_per_day.is_finite()
                || !(0.0..=MAX_SERVICE_RESEARCH_PER_DAY).contains(&site.research_per_day)
            {
                return Err(ServiceError::Invalid("site research"));
            }
            if site.kind.produces_research() != (site.research_per_day > 0.0) {
                return Err(ServiceError::Invalid("site research kind"));
            }
            let expected = ServiceSite::default_for(site.kind, site.tier);
            if site.radius_m != expected.radius_m
                || site.capacity != expected.capacity
                || site.research_per_day != expected.research_per_day
            {
                return Err(ServiceError::Invalid("site tier stats"));
            }
        }
        Ok(())
    }

    /// FNV-1a checksum over the compact JSON.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let json = serde_json::to_vec(self).expect("services always serialize");
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in json {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
}

/// Result of one service refresh.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ServiceReport {
    /// School service share, 0..=1.
    pub school_supply: f32,
    /// Clinic service share, 0..=1.
    pub clinic_supply: f32,
    /// Fire service share, 0..=1.
    pub fire_supply: f32,
    /// Police service share, 0..=1.
    pub police_supply: f32,
    /// School demand.
    pub school_demand: f64,
    /// Clinic demand.
    pub clinic_demand: f64,
    /// Fire demand.
    pub fire_demand: f64,
    /// Police demand.
    pub police_demand: f64,
    /// School demand served.
    pub school_served: f64,
    /// Clinic demand served.
    pub clinic_served: f64,
    /// Fire demand served.
    pub fire_served: f64,
    /// Police demand served.
    pub police_served: f64,
    /// Installed school capacity.
    pub school_capacity: f64,
    /// Installed clinic capacity.
    pub clinic_capacity: f64,
    /// Installed fire capacity.
    pub fire_capacity: f64,
    /// Installed police capacity.
    pub police_capacity: f64,
    /// Share of school demand inside a school radius.
    pub school_coverage: f32,
    /// Share of clinic demand inside a clinic radius.
    pub clinic_coverage: f32,
    /// Share of fire demand inside a fire radius.
    pub fire_coverage: f32,
    /// Share of police demand inside a police radius.
    pub police_coverage: f32,
    /// Sites per kind, indexed by `ServiceKind as usize`.
    pub sites_of: [usize; 4],
    /// Covered buildings per kind, indexed by `ServiceKind as usize`.
    pub covered_buildings: [usize; 4],
    /// Research produced per game day.
    pub research_per_day: f64,
    /// Consumers evaluated.
    pub consumers: usize,
    /// Total sites.
    pub sites: usize,
}

#[derive(Debug, Clone, Copy)]
struct ServiceShare {
    supply: f32,
    served: f64,
    coverage: f32,
}

fn site_covers(site: &ServiceSite, consumer: &ServiceConsumer) -> bool {
    let dx = f64::from(site.center[0]) - f64::from(consumer.center[0]);
    let dz = f64::from(site.center[2]) - f64::from(consumer.center[1]);
    dx * dx + dz * dz <= f64::from(site.radius_m) * f64::from(site.radius_m)
}

fn covered(
    consumer: &ServiceConsumer,
    kind: ServiceKind,
    sites: &BTreeMap<u64, ServiceSite>,
) -> bool {
    sites
        .values()
        .any(|site| site.kind == kind && site_covers(site, consumer))
}

fn supply_share(demand: f64, covered: f64, capacity: f64, sites: usize) -> ServiceShare {
    if sites == 0 {
        return ServiceShare {
            supply: 0.0,
            served: 0.0,
            coverage: 0.0,
        };
    }
    if demand <= 0.0 {
        return ServiceShare {
            supply: 1.0,
            served: 0.0,
            coverage: 1.0,
        };
    }
    let served = capacity.min(covered).max(0.0);
    ServiceShare {
        supply: (served / demand).clamp(0.0, 1.0) as f32,
        served,
        coverage: (covered / demand).clamp(0.0, 1.0) as f32,
    }
}

/// Errors returned by the civic service store.
#[derive(Debug)]
pub enum ServiceError {
    /// JSON parse/serialize error.
    Json(serde_json::Error),
    /// Unsupported save version.
    UnsupportedVersion(u16),
    /// A range or state invariant failed.
    Invalid(&'static str),
    /// Too many sites for one save.
    TooManySites(usize),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "service JSON error: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported service version {version}")
            }
            Self::Invalid(message) => write!(formatter, "invalid service: {message}"),
            Self::TooManySites(count) => {
                write!(
                    formatter,
                    "services have {count} sites; limit is {MAX_SERVICE_SITES}"
                )
            }
        }
    }
}

impl std::error::Error for ServiceError {}

impl From<serde_json::Error> for ServiceError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn consumer(id: u64, x: f32, residents: u32, workers: u32) -> ServiceConsumer {
        ServiceConsumer {
            id,
            center: [x, 0.0],
            residents,
            workers,
        }
    }

    #[test]
    fn an_empty_store_has_no_service() {
        let mut store = ServiceStore::new(2026);
        let report = store.refresh(&[consumer(1, 0.0, 4, 2)]);
        assert_eq!(report.school_supply, 0.0);
        assert_eq!(report.clinic_supply, 0.0);
        assert_eq!(report.fire_supply, 0.0);
        assert_eq!(report.police_supply, 0.0);
        assert_eq!(report.research_per_day, 0.0);
        assert_eq!(store.site_count(), 0);
    }

    #[test]
    fn a_school_covers_residents_and_produces_research() {
        let mut store = ServiceStore::new(2026);
        store
            .add_site(1, ServiceKind::School, ServiceTier::Basic, [0.0, 0.0, 0.0])
            .expect("site");
        let report = store.refresh(&[consumer(1, 10.0, 4, 0)]);
        assert!(report.school_supply > 0.99);
        assert!(report.research_per_day > 7.9);
        assert_eq!(store.count_of(ServiceKind::School), 1);
        assert_eq!(report.covered_buildings[ServiceKind::School as usize], 1);
    }

    #[test]
    fn clinic_fire_and_police_use_civic_demand() {
        let mut store = ServiceStore::new(2026);
        for (id, kind) in [
            (1, ServiceKind::Clinic),
            (2, ServiceKind::Fire),
            (3, ServiceKind::Police),
        ] {
            store
                .add_site(id, kind, ServiceTier::Basic, [0.0, 0.0, 0.0])
                .expect("site");
        }
        let report = store.refresh(&[consumer(10, 20.0, 4, 2)]);
        assert!(report.clinic_supply > 0.99);
        assert!(report.fire_supply > 0.99);
        assert!(report.police_supply > 0.99);
        assert_eq!(report.research_per_day, 0.0);
    }

    #[test]
    fn a_capacity_shortage_lowers_supply() {
        let mut store = ServiceStore::new(2026);
        store
            .add_site(1, ServiceKind::School, ServiceTier::Basic, [0.0, 0.0, 0.0])
            .expect("site");
        let report = store.refresh(&[consumer(1, 0.0, 4_000, 0)]);
        assert!(report.school_supply < 0.5);
        assert!(report.school_coverage > 0.99);
    }

    #[test]
    fn a_site_out_of_range_does_not_cover() {
        let mut store = ServiceStore::new(2026);
        store
            .add_site(1, ServiceKind::Clinic, ServiceTier::Basic, [0.0, 0.0, 0.0])
            .expect("site");
        let report = store.refresh(&[consumer(1, 500.0, 4, 0)]);
        assert_eq!(report.clinic_supply, 0.0);
        assert_eq!(report.clinic_coverage, 0.0);
    }

    #[test]
    fn overlapping_sites_add_capacity_but_not_demand() {
        let mut store = ServiceStore::new(2026);
        store
            .add_site(1, ServiceKind::Fire, ServiceTier::Basic, [0.0, 0.0, 0.0])
            .expect("site");
        store
            .add_site(2, ServiceKind::Fire, ServiceTier::Basic, [0.0, 0.0, 0.0])
            .expect("site");
        let report = store.refresh(&[consumer(10, 0.0, 4, 0)]);
        assert!(report.fire_capacity > 799.0);
        assert!(report.fire_demand < 5.0);
        assert!(report.fire_supply > 0.99);
    }

    #[test]
    fn zero_demand_with_a_site_reports_full_service_but_no_research() {
        let mut store = ServiceStore::new(2026);
        store
            .add_site(1, ServiceKind::School, ServiceTier::Basic, [0.0, 0.0, 0.0])
            .expect("site");
        let report = store.refresh(&[]);
        assert_eq!(report.school_supply, 1.0);
        assert_eq!(report.school_coverage, 1.0);
        assert_eq!(report.research_per_day, 0.0);
    }

    #[test]
    fn overlapping_schools_do_not_stack_research() {
        let mut store = ServiceStore::new(2026);
        for id in 1..=3 {
            store
                .add_site(id, ServiceKind::School, ServiceTier::Basic, [0.0, 0.0, 0.0])
                .expect("site");
        }
        store.sites.get_mut(&3).expect("site").capacity = 0.0;
        let report = store.refresh(&[consumer(10, 0.0, 4, 0)]);
        assert!(report.school_supply > 0.99);
        assert!((report.research_per_day - 8.0).abs() < 0.001);
        assert!((store.research_per_day - 8.0).abs() < 0.001);
    }

    #[test]
    fn tier_stats_must_match_the_table() {
        let mut store = ServiceStore::new(2026);
        store
            .add_site(1, ServiceKind::School, ServiceTier::Basic, [0.0, 0.0, 0.0])
            .expect("site");
        store.sites.get_mut(&1).expect("site").capacity = 999.0;
        assert!(store.validate().is_err());
    }

    #[test]
    fn tiers_and_aliases_are_stable() {
        assert_eq!(ServiceTier::from_age(Age::Wood), ServiceTier::Basic);
        assert_eq!(
            ServiceTier::from_age(Age::Industrial),
            ServiceTier::Improved
        );
        assert_eq!(
            ServiceTier::from_age(Age::Information),
            ServiceTier::Advanced
        );
        assert_eq!(ServiceKind::parse("firehouse"), Some(ServiceKind::Fire));
        assert_eq!(ServiceKind::parse("health"), Some(ServiceKind::Clinic));
        assert_eq!(ServiceTier::parse("modern"), Some(ServiceTier::Advanced));
        assert_eq!(ServiceKind::parse("library"), None);
    }

    #[test]
    fn remove_and_clear_are_deterministic() {
        let mut store = ServiceStore::new(2026);
        store
            .add_site(1, ServiceKind::School, ServiceTier::Basic, [0.0, 0.0, 0.0])
            .expect("site");
        store
            .add_site(2, ServiceKind::Clinic, ServiceTier::Basic, [0.0, 0.0, 0.0])
            .expect("site");
        assert!(store.remove_placement(1));
        assert!(!store.remove_placement(1));
        assert_eq!(store.site_count(), 1);
        store.clear();
        assert_eq!(store.site_count(), 0);
        assert_eq!(store.research_per_day, 0.0);
    }

    #[test]
    fn json_round_trip_and_checksum() {
        let mut store = ServiceStore::new(2026);
        store
            .add_site(
                1,
                ServiceKind::School,
                ServiceTier::Improved,
                [4.0, 1.0, 6.0],
            )
            .expect("site");
        store.refresh(&[consumer(1, 5.0, 8, 2)]);
        let json = store.to_json_pretty().expect("json");
        let decoded = ServiceStore::from_json(&json).expect("decode");
        assert_eq!(decoded, store);
        assert_eq!(decoded.checksum(), store.checksum());
    }

    #[test]
    fn invalid_service_is_rejected() {
        let mut store = ServiceStore::new(2026);
        store.school_supply = f32::NAN;
        assert!(store.validate().is_err());

        let mut store = ServiceStore::new(2026);
        store
            .add_site(1, ServiceKind::School, ServiceTier::Basic, [0.0, 0.0, 0.0])
            .expect("site");
        store.sites.get_mut(&1).expect("site").radius_m = -1.0;
        assert!(store.validate().is_err());
    }
}
