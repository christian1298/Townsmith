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

/// School construction cost by tier: basic, improved, advanced.
pub const SCHOOL_CONSTRUCTION_COST_PER_TIER: [f64; 3] = [4_000.0, 6_000.0, 10_000.0];

/// Clinic construction cost by tier: basic, improved, advanced.
pub const CLINIC_CONSTRUCTION_COST_PER_TIER: [f64; 3] = [3_000.0, 5_000.0, 8_000.0];

/// Fire and police construction cost by tier: basic, improved, advanced.
pub const FIRE_POLICE_CONSTRUCTION_COST_PER_TIER: [f64; 3] = [2_500.0, 4_000.0, 6_000.0];

/// School operating upkeep per game day by tier: basic, improved, advanced.
pub const SCHOOL_UPKEEP_PER_DAY_PER_TIER: [f64; 3] = [3.0, 5.0, 8.0];

/// Clinic operating upkeep per game day by tier: basic, improved, advanced.
pub const CLINIC_UPKEEP_PER_DAY_PER_TIER: [f64; 3] = [2.0, 4.0, 6.0];

/// Fire and police operating upkeep per game day by tier: basic, improved,
/// advanced.
pub const FIRE_POLICE_UPKEEP_PER_DAY_PER_TIER: [f64; 3] = [1.5, 3.0, 5.0];

/// Campaign lead time before a new school is operational, by tier.
pub const CAMPAIGN_SCHOOL_BUILD_DAYS_PER_TIER: [f32; 3] = [2.0, 2.5, 3.0];

/// Campaign lead time before a new clinic is operational, by tier.
pub const CAMPAIGN_CLINIC_BUILD_DAYS_PER_TIER: [f32; 3] = [1.5, 2.0, 2.5];

/// Campaign lead time before a new fire or police site is operational, by tier.
pub const CAMPAIGN_FIRE_POLICE_BUILD_DAYS_PER_TIER: [f32; 3] = [1.0, 1.5, 2.0];

/// Largest accepted site lead time, a guard against malformed saves.
pub const MAX_SERVICE_BUILD_DAYS: f32 = 365.0;

/// Share of the construction cost due while a site is still building.
///
/// The remainder is due once the lead time elapses and the site becomes
/// operational. Developer-mode sites have no lead time and owe the full cost
/// immediately.
pub const SERVICE_BUILD_PAYMENT_SHARE: f64 = 0.5;

fn is_zero_f32(value: &f32) -> bool {
    *value == 0.0
}

fn is_zero_f64(value: &f64) -> bool {
    *value == 0.0
}

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

    /// Campaign lead time before a new site is operational, in game days.
    ///
    /// | Kind | Basic | Improved | Advanced |
    /// |---|---:|---:|---:|
    /// | School | 2 | 2.5 | 3 |
    /// | Clinic | 1.5 | 2 | 2.5 |
    /// | Fire | 1 | 1.5 | 2 |
    /// | Police | 1 | 1.5 | 2 |
    ///
    /// Developer mode ignores this and adds sites immediately operational.
    #[must_use]
    pub fn campaign_build_days(self, tier: ServiceTier) -> f32 {
        let days = match self {
            Self::School => CAMPAIGN_SCHOOL_BUILD_DAYS_PER_TIER,
            Self::Clinic => CAMPAIGN_CLINIC_BUILD_DAYS_PER_TIER,
            Self::Fire | Self::Police => CAMPAIGN_FIRE_POLICE_BUILD_DAYS_PER_TIER,
        };
        days[tier.cost_index()]
    }

    /// Construction cost of one site of this kind at a tier.
    ///
    /// | Kind | Basic | Improved | Advanced |
    /// |---|---:|---:|---:|
    /// | School | 4,000 | 6,000 | 10,000 |
    /// | Clinic | 3,000 | 5,000 | 8,000 |
    /// | Fire | 2,500 | 4,000 | 6,000 |
    /// | Police | 2,500 | 4,000 | 6,000 |
    ///
    /// These are data-only catalog values; the bridge charges them through
    /// the city treasury in two installments when the campaign clock is
    /// active and immediately in developer mode.
    #[must_use]
    pub fn construction_cost(self, tier: ServiceTier) -> f64 {
        let costs = match self {
            Self::School => SCHOOL_CONSTRUCTION_COST_PER_TIER,
            Self::Clinic => CLINIC_CONSTRUCTION_COST_PER_TIER,
            Self::Fire | Self::Police => FIRE_POLICE_CONSTRUCTION_COST_PER_TIER,
        };
        costs[tier.cost_index()]
    }

    /// Operating upkeep of one site of this kind per game day at a tier.
    ///
    /// | Kind | Basic | Improved | Advanced |
    /// |---|---:|---:|---:|
    /// | School | 3 | 5 | 8 |
    /// | Clinic | 2 | 4 | 6 |
    /// | Fire | 1.5 | 3 | 5 |
    /// | Police | 1.5 | 3 | 5 |
    ///
    /// These are data-only catalog values and are not charged by the live
    /// economy yet.
    #[must_use]
    pub fn upkeep_per_day(self, tier: ServiceTier) -> f64 {
        let upkeep = match self {
            Self::School => SCHOOL_UPKEEP_PER_DAY_PER_TIER,
            Self::Clinic => CLINIC_UPKEEP_PER_DAY_PER_TIER,
            Self::Fire | Self::Police => FIRE_POLICE_UPKEEP_PER_DAY_PER_TIER,
        };
        upkeep[tier.cost_index()]
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

    /// Index into a per-tier cost table, in display order.
    fn cost_index(self) -> usize {
        match self {
            Self::Basic => 0,
            Self::Improved => 1,
            Self::Advanced => 2,
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
    /// Game days left before the site is operational.
    ///
    /// Zero is the legacy immediate default and is omitted from the save so
    /// existing checksums stay pinned. Only the opt-in campaign clock assigns
    /// a positive lead time.
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub build_days_remaining: f32,
    /// Total construction price of the site.
    ///
    /// Zero is the legacy fully prepaid path: the site is never charged and
    /// the field is omitted from the save so existing checksums stay pinned.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub cost_total: f64,
    /// Construction price paid so far.
    ///
    /// Omitted from the save while zero, keeping legacy saves byte-identical.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub cost_paid: f64,
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
            build_days_remaining: 0.0,
            cost_total: 0.0,
            cost_paid: 0.0,
        }
    }

    /// Whether the site is finished and can supply service.
    #[must_use]
    pub fn is_operational(&self) -> bool {
        self.build_days_remaining <= 0.0
    }

    /// Cumulative construction price that should be paid by now.
    ///
    /// An operational site owes the full price; a site still inside its lead
    /// time owes [`SERVICE_BUILD_PAYMENT_SHARE`] of it. A zero-cost legacy
    /// site always owes zero.
    #[must_use]
    pub fn target_paid(&self) -> f64 {
        if self.cost_total <= 0.0 {
            return 0.0;
        }
        if self.is_operational() {
            self.cost_total
        } else {
            self.cost_total * SERVICE_BUILD_PAYMENT_SHARE
        }
    }

    /// Construction price still owed, never negative.
    #[must_use]
    pub fn payment_due(&self) -> f64 {
        (self.target_paid() - self.cost_paid).max(0.0)
    }

    /// Record an installment payment up to the amount currently due.
    ///
    /// The amount must be finite and within `0.0..=payment_due()`. Paying
    /// more than the current target is rejected, so a site can never be
    /// charged twice for the same installment.
    pub fn mark_paid(&mut self, amount: f64) -> Result<(), ServiceError> {
        let due = self.payment_due();
        if !amount.is_finite() || amount < 0.0 || amount > due {
            return Err(ServiceError::Invalid(
                "payment amount must be finite and within the amount due",
            ));
        }
        self.cost_paid = (self.cost_paid + amount).min(self.target_paid());
        Ok(())
    }

    /// Construction cost of this site from its kind and tier.
    #[must_use]
    pub fn construction_cost(&self) -> f64 {
        self.kind.construction_cost(self.tier)
    }

    /// Operating upkeep of this site per game day from its kind and tier.
    #[must_use]
    pub fn upkeep_per_day(&self) -> f64 {
        self.kind.upkeep_per_day(self.tier)
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

    /// Sites of one kind that are operational.
    #[must_use]
    pub fn operational_site_count(&self, kind: ServiceKind) -> usize {
        self.sites
            .values()
            .filter(|site| site.kind == kind && site.is_operational())
            .count()
    }

    /// Operational sites across every kind.
    #[must_use]
    pub fn operational_total(&self) -> usize {
        self.sites
            .values()
            .filter(|site| site.is_operational())
            .count()
    }

    /// Operational sites across every kind; alias for [`Self::operational_total`].
    #[must_use]
    pub fn ready_count(&self) -> usize {
        self.operational_total()
    }

    /// Sites still inside their lead time.
    #[must_use]
    pub fn building_count(&self) -> usize {
        self.site_count() - self.ready_count()
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

    /// One site by placement id.
    #[must_use]
    pub fn site(&self, placement_id: u64) -> Option<&ServiceSite> {
        self.sites.get(&placement_id)
    }

    /// One site by placement id, mutable.
    #[must_use]
    pub fn site_mut(&mut self, placement_id: u64) -> Option<&mut ServiceSite> {
        self.sites.get_mut(&placement_id)
    }

    /// Construction price still owed on one site.
    #[must_use]
    pub fn payment_due(&self, placement_id: u64) -> f64 {
        self.site(placement_id)
            .map_or(0.0, ServiceSite::payment_due)
    }

    /// Construction price still owed across every site.
    #[must_use]
    pub fn total_payment_due(&self) -> f64 {
        self.sites.values().map(ServiceSite::payment_due).sum()
    }

    /// Construction cost of every site of one kind.
    #[must_use]
    pub fn construction_cost_of(&self, kind: ServiceKind) -> f64 {
        self.sites_of(kind)
            .map(|(_, site)| site.construction_cost())
            .sum()
    }

    /// Operating upkeep of every operational site of one kind per game day.
    ///
    /// Sites still inside their campaign lead time are not staffed yet, so
    /// they do not bill operating upkeep.
    #[must_use]
    pub fn upkeep_per_day_of(&self, kind: ServiceKind) -> f64 {
        self.sites
            .values()
            .filter(|site| site.kind == kind && site.is_operational())
            .map(ServiceSite::upkeep_per_day)
            .sum()
    }

    /// Construction cost of every civic service site.
    #[must_use]
    pub fn total_construction_cost(&self) -> f64 {
        self.sites
            .values()
            .map(ServiceSite::construction_cost)
            .sum()
    }

    /// Operating upkeep of every operational civic service site per game day.
    #[must_use]
    pub fn total_upkeep_per_day(&self) -> f64 {
        self.sites
            .values()
            .filter(|site| site.is_operational())
            .map(ServiceSite::upkeep_per_day)
            .sum()
    }

    /// Add or replace one service site.
    ///
    /// The site is immediately operational; use
    /// [`Self::add_site_with_lead_days`] for the campaign lead time.
    pub fn add_site(
        &mut self,
        placement_id: u64,
        kind: ServiceKind,
        tier: ServiceTier,
        center: [f32; 3],
    ) -> Result<(), ServiceError> {
        self.add_site_with_lead_days(placement_id, kind, tier, center, 0.0)
    }

    /// Add or replace one service site with a lead time in game days.
    ///
    /// `lead_days` must be finite and within `0.0..=MAX_SERVICE_BUILD_DAYS`.
    /// A non-zero lead time hides the site from supply, coverage, research,
    /// and operating upkeep until [`Self::tick`] clears it. The site is added
    /// as a legacy zero-cost site; use
    /// [`Self::add_priced_site_with_lead_days`] to record a construction
    /// price and two-installment schedule.
    pub fn add_site_with_lead_days(
        &mut self,
        placement_id: u64,
        kind: ServiceKind,
        tier: ServiceTier,
        center: [f32; 3],
        lead_days: f32,
    ) -> Result<(), ServiceError> {
        self.add_priced_site_with_lead_days(placement_id, kind, tier, center, lead_days, 0.0)
    }

    /// Add or replace one service site with a lead time and construction price.
    ///
    /// `lead_days` must be finite and within `0.0..=MAX_SERVICE_BUILD_DAYS`.
    /// `cost_total` must be finite and non-negative; zero keeps the legacy
    /// prepaid behavior. A priced site owes half of `cost_total` while it is
    /// building and the remainder once it is operational.
    pub fn add_priced_site_with_lead_days(
        &mut self,
        placement_id: u64,
        kind: ServiceKind,
        tier: ServiceTier,
        center: [f32; 3],
        lead_days: f32,
        cost_total: f64,
    ) -> Result<(), ServiceError> {
        if !lead_days.is_finite() || !(0.0..=MAX_SERVICE_BUILD_DAYS).contains(&lead_days) {
            return Err(ServiceError::Invalid("site lead days"));
        }
        if !cost_total.is_finite() || cost_total < 0.0 {
            return Err(ServiceError::Invalid("site construction cost"));
        }
        if placement_id > i64::MAX as u64 {
            return Err(ServiceError::Invalid("site id is above i64::MAX"));
        }
        if !self.sites.contains_key(&placement_id) && self.sites.len() >= MAX_SERVICE_SITES {
            return Err(ServiceError::TooManySites(self.sites.len()));
        }
        let mut site = ServiceSite::default_for(kind, tier);
        site.center = center;
        site.build_days_remaining = lead_days;
        site.cost_total = cost_total;
        self.sites.insert(placement_id, site);
        Ok(())
    }

    /// Advance every site's lead time by `days`, clamping at zero.
    ///
    /// Negative or non-finite input is rejected rather than silently ignored.
    /// A zero step is a valid no-op.
    pub fn tick(&mut self, days: f32) -> Result<(), ServiceError> {
        if !days.is_finite() || days < 0.0 {
            return Err(ServiceError::Invalid("tick days"));
        }
        if days == 0.0 {
            return Ok(());
        }
        for site in self.sites.values_mut() {
            site.build_days_remaining = (site.build_days_remaining - days).max(0.0);
        }
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
                .filter(|site| site.kind == kind && site.is_operational())
                .map(|site| site.capacity)
                .sum();
            let covered_demand: f64 = consumers
                .iter()
                .filter(|consumer| covered(consumer, kind, &self.sites))
                .map(|consumer| consumer.demand(kind))
                .sum();
            let service = supply_share(
                demand,
                covered_demand,
                capacity,
                self.operational_site_count(kind),
            );
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
            .filter(|site| site.kind == ServiceKind::School && site.is_operational())
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
            if !site.build_days_remaining.is_finite()
                || !(0.0..=MAX_SERVICE_BUILD_DAYS).contains(&site.build_days_remaining)
            {
                return Err(ServiceError::Invalid("site lead days"));
            }
            if !site.cost_total.is_finite() || site.cost_total < 0.0 {
                return Err(ServiceError::Invalid("site construction cost"));
            }
            if !site.cost_paid.is_finite()
                || site.cost_paid < 0.0
                || site.cost_paid > site.cost_total
            {
                return Err(ServiceError::Invalid("site construction paid"));
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
        .any(|site| site.kind == kind && site.is_operational() && site_covers(site, consumer))
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
    fn service_costs_scale_with_tier() {
        assert_eq!(
            ServiceKind::School.construction_cost(ServiceTier::Basic),
            4_000.0
        );
        assert_eq!(
            ServiceKind::School.construction_cost(ServiceTier::Advanced),
            10_000.0
        );
        assert_eq!(
            ServiceKind::Clinic.upkeep_per_day(ServiceTier::Improved),
            4.0
        );
        assert_eq!(
            ServiceKind::Fire.construction_cost(ServiceTier::Basic),
            2_500.0
        );
        assert_eq!(
            ServiceKind::Police.upkeep_per_day(ServiceTier::Advanced),
            5.0
        );
    }

    #[test]
    fn service_store_totals_include_every_site() {
        let mut store = ServiceStore::new(2026);
        store
            .add_site(1, ServiceKind::School, ServiceTier::Basic, [0.0, 0.0, 0.0])
            .expect("school");
        store
            .add_site(
                2,
                ServiceKind::Clinic,
                ServiceTier::Advanced,
                [0.0, 0.0, 0.0],
            )
            .expect("clinic");
        assert_eq!(store.total_construction_cost(), 4_000.0 + 8_000.0);
        assert_eq!(store.total_upkeep_per_day(), 3.0 + 6.0);
        assert_eq!(store.construction_cost_of(ServiceKind::School), 4_000.0);
        assert_eq!(store.upkeep_per_day_of(ServiceKind::Clinic), 6.0);
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
    fn developer_service_sites_are_operational_and_omit_the_lead_field() {
        let mut store = ServiceStore::new(2026);
        store
            .add_site(1, ServiceKind::School, ServiceTier::Basic, [1.0, 2.0, 3.0])
            .expect("school");
        store
            .add_site(2, ServiceKind::Fire, ServiceTier::Advanced, [4.0, 5.0, 6.0])
            .expect("fire");
        assert_eq!(store.site_count(), 2);
        assert_eq!(store.ready_count(), 2);
        assert_eq!(store.building_count(), 0);
        assert_eq!(store.operational_total(), 2);
        let json = serde_json::to_string(&store).expect("json");
        assert!(!json.contains("build_days_remaining"));
        // A legacy save without the field decodes as immediately operational.
        let legacy: ServiceStore = serde_json::from_str(&json).expect("legacy");
        assert_eq!(legacy, store);
        assert!(legacy.sites.values().all(ServiceSite::is_operational));
    }

    #[test]
    fn campaign_lead_times_follow_the_tier_table() {
        for tier in ServiceTier::ALL {
            assert!(ServiceKind::School.campaign_build_days(tier) >= 2.0);
            assert!(ServiceKind::Clinic.campaign_build_days(tier) >= 1.5);
            assert!(ServiceKind::Fire.campaign_build_days(tier) >= 1.0);
            assert!(ServiceKind::Fire.campaign_build_days(tier) <= 2.0);
            assert_eq!(
                ServiceKind::Fire.campaign_build_days(tier),
                ServiceKind::Police.campaign_build_days(tier)
            );
        }
        assert_eq!(
            ServiceKind::School.campaign_build_days(ServiceTier::Basic),
            2.0
        );
        assert_eq!(
            ServiceKind::School.campaign_build_days(ServiceTier::Advanced),
            3.0
        );
        assert_eq!(
            ServiceKind::Clinic.campaign_build_days(ServiceTier::Improved),
            2.0
        );
        assert_eq!(
            ServiceKind::Police.campaign_build_days(ServiceTier::Basic),
            1.0
        );
    }

    #[test]
    fn campaign_lead_time_hides_a_service_until_the_days_pass() {
        let mut store = ServiceStore::new(2026);
        store
            .add_site_with_lead_days(
                1,
                ServiceKind::Clinic,
                ServiceTier::Basic,
                [0.0, 0.0, 0.0],
                1.5,
            )
            .expect("site");
        assert_eq!(store.site_count(), 1);
        assert_eq!(store.ready_count(), 0);
        assert_eq!(store.building_count(), 1);
        assert_eq!(store.operational_site_count(ServiceKind::Clinic), 0);
        assert_eq!(store.total_upkeep_per_day(), 0.0);
        let report = store.refresh(&[consumer(10, 0.0, 4, 2)]);
        assert_eq!(report.clinic_supply, 0.0);
        assert_eq!(report.clinic_capacity, 0.0);
        assert_eq!(report.covered_buildings[ServiceKind::Clinic as usize], 0);

        store.tick(1.0).expect("tick");
        assert_eq!(store.ready_count(), 0);
        assert_eq!(store.total_upkeep_per_day(), 0.0);
        assert_eq!(store.refresh(&[consumer(10, 0.0, 4, 2)]).clinic_supply, 0.0);

        store.tick(0.5).expect("tick");
        assert_eq!(store.ready_count(), 1);
        assert_eq!(store.building_count(), 0);
        assert_eq!(store.total_upkeep_per_day(), 2.0);
        let report = store.refresh(&[consumer(10, 0.0, 4, 2)]);
        assert_eq!(report.clinic_supply, 1.0);
        assert_eq!(report.clinic_capacity, 300.0);
    }

    #[test]
    fn building_sites_add_no_capacity_coverage_research_or_upkeep() {
        let mut store = ServiceStore::new(2026);
        store
            .add_site(1, ServiceKind::School, ServiceTier::Basic, [0.0, 0.0, 0.0])
            .expect("ready");
        store
            .add_site_with_lead_days(
                2,
                ServiceKind::School,
                ServiceTier::Basic,
                [0.0, 0.0, 0.0],
                2.0,
            )
            .expect("building");
        assert_eq!(store.count_of(ServiceKind::School), 2);
        assert_eq!(store.operational_site_count(ServiceKind::School), 1);
        assert_eq!(store.upkeep_per_day_of(ServiceKind::School), 3.0);
        assert_eq!(store.total_upkeep_per_day(), 3.0);
        let report = store.refresh(&[consumer(10, 0.0, 4_000, 0)]);
        // Only the operational school's 200 capacity is installed, and only
        // one school's research is produced.
        assert_eq!(report.school_capacity, 200.0);
        assert_eq!(report.school_served, 200.0);
        assert_eq!(report.school_coverage, 1.0);
        assert!(report.school_supply < 0.21);
        assert!((report.research_per_day - 8.0).abs() < 0.001);
        assert_eq!(report.sites_of[ServiceKind::School as usize], 2);
    }

    #[test]
    fn tick_rejects_invalid_days() {
        let mut store = ServiceStore::new(2026);
        store
            .add_site_with_lead_days(
                1,
                ServiceKind::Fire,
                ServiceTier::Basic,
                [0.0, 0.0, 0.0],
                1.0,
            )
            .expect("site");
        assert!(store.tick(f32::NAN).is_err());
        assert!(store.tick(f32::INFINITY).is_err());
        assert!(store.tick(-1.0).is_err());
        assert!(store.tick(0.0).is_ok());
        assert_eq!(store.sites[&1].build_days_remaining, 1.0);
    }

    #[test]
    fn lead_time_and_operational_state_round_trip() {
        let mut store = ServiceStore::new(2026);
        store
            .add_site_with_lead_days(
                1,
                ServiceKind::Fire,
                ServiceTier::Improved,
                [1.0, 2.0, 3.0],
                1.5,
            )
            .expect("site");
        store.tick(0.5).expect("tick");
        assert_eq!(store.sites[&1].build_days_remaining, 1.0);
        let json = store.to_json_pretty().expect("json");
        let decoded = ServiceStore::from_json(&json).expect("decode");
        assert_eq!(decoded, store);
        assert_eq!(decoded.sites[&1].build_days_remaining, 1.0);
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
        let mut store = ServiceStore::new(2026);
        store
            .add_site(1, ServiceKind::School, ServiceTier::Basic, [0.0, 0.0, 0.0])
            .expect("site");
        store.sites.get_mut(&1).expect("site").build_days_remaining = -0.5;
        assert!(store.validate().is_err());
        store.sites.get_mut(&1).expect("site").build_days_remaining = f32::NAN;
        assert!(store.validate().is_err());
    }

    #[test]
    fn a_priced_developer_service_owes_the_full_cost_at_add() {
        let mut store = ServiceStore::new(2026);
        store
            .add_priced_site_with_lead_days(
                1,
                ServiceKind::School,
                ServiceTier::Basic,
                [0.0; 3],
                0.0,
                4_000.0,
            )
            .expect("school");
        store
            .add_priced_site_with_lead_days(
                2,
                ServiceKind::Clinic,
                ServiceTier::Advanced,
                [0.0; 3],
                0.0,
                8_000.0,
            )
            .expect("clinic");
        assert_eq!(store.payment_due(1), 4_000.0);
        assert_eq!(store.payment_due(2), 8_000.0);
        assert_eq!(store.total_payment_due(), 12_000.0);
        store
            .site_mut(1)
            .expect("school")
            .mark_paid(4_000.0)
            .expect("paid");
        store
            .site_mut(2)
            .expect("clinic")
            .mark_paid(8_000.0)
            .expect("paid");
        assert_eq!(store.total_payment_due(), 0.0);
    }

    #[test]
    fn a_priced_campaign_service_owes_half_while_building_and_half_when_ready() {
        let mut store = ServiceStore::new(2026);
        store
            .add_priced_site_with_lead_days(
                1,
                ServiceKind::Fire,
                ServiceTier::Improved,
                [0.0; 3],
                1.5,
                6_000.0,
            )
            .expect("fire");
        {
            let site = store.site(1).expect("site");
            assert!(!site.is_operational());
            assert_eq!(site.target_paid(), 3_000.0);
            assert_eq!(site.payment_due(), 3_000.0);
        }
        store
            .site_mut(1)
            .expect("site")
            .mark_paid(3_000.0)
            .expect("first installment");
        assert_eq!(store.payment_due(1), 0.0);
        store.tick(1.5).expect("tick");
        {
            let site = store.site(1).expect("site");
            assert!(site.is_operational());
            assert_eq!(site.target_paid(), 6_000.0);
            assert_eq!(site.payment_due(), 3_000.0);
        }
        store
            .site_mut(1)
            .expect("site")
            .mark_paid(3_000.0)
            .expect("second installment");
        assert_eq!(store.site(1).expect("site").cost_paid, 6_000.0);
        assert_eq!(store.payment_due(1), 0.0);
    }

    #[test]
    fn a_paid_service_installment_is_not_charged_twice() {
        let mut store = ServiceStore::new(2026);
        store
            .add_priced_site_with_lead_days(
                1,
                ServiceKind::Police,
                ServiceTier::Basic,
                [0.0; 3],
                1.0,
                4_000.0,
            )
            .expect("police");
        let site = store.site_mut(1).expect("site");
        site.mark_paid(2_000.0).expect("first installment");
        assert!(site.mark_paid(2_000.0).is_err());
        assert_eq!(site.cost_paid, 2_000.0);
        store.tick(1.0).expect("tick");
        let site = store.site_mut(1).expect("site");
        site.mark_paid(2_000.0).expect("second installment");
        assert!(site.mark_paid(2_000.0).is_err());
        assert!(site.mark_paid(0.0).is_ok());
        assert_eq!(site.cost_paid, 4_000.0);
    }

    #[test]
    fn an_unpriced_service_keeps_the_legacy_zero_cost_default() {
        let mut store = ServiceStore::new(2026);
        store
            .add_site(1, ServiceKind::School, ServiceTier::Basic, [0.0; 3])
            .expect("site");
        let site = store.site(1).expect("site");
        assert_eq!(site.cost_total, 0.0);
        assert_eq!(site.cost_paid, 0.0);
        assert_eq!(site.target_paid(), 0.0);
        assert_eq!(site.payment_due(), 0.0);
        assert_eq!(store.total_payment_due(), 0.0);
        let json = store.to_json_pretty().expect("json");
        assert!(!json.contains("cost_total"));
        assert!(!json.contains("cost_paid"));
    }

    #[test]
    fn invalid_service_cost_amounts_are_rejected() {
        let mut store = ServiceStore::new(2026);
        for invalid in [-1.0, f64::NAN, f64::INFINITY] {
            assert!(store
                .add_priced_site_with_lead_days(
                    1,
                    ServiceKind::School,
                    ServiceTier::Basic,
                    [0.0; 3],
                    0.0,
                    invalid,
                )
                .is_err());
        }
        assert_eq!(store.site_count(), 0);
        store
            .add_priced_site_with_lead_days(
                1,
                ServiceKind::School,
                ServiceTier::Basic,
                [0.0; 3],
                0.0,
                100.0,
            )
            .expect("site");
        let site = store.site_mut(1).expect("site");
        assert!(site.mark_paid(-1.0).is_err());
        assert!(site.mark_paid(f64::NAN).is_err());
        assert!(site.mark_paid(100.5).is_err());
        assert!(site.mark_paid(100.0).is_ok());
        assert!(store.validate().is_ok());
        store.site_mut(1).expect("site").cost_paid = 101.0;
        assert!(store.validate().is_err());
        store.site_mut(1).expect("site").cost_paid = f64::NAN;
        assert!(store.validate().is_err());
        store.site_mut(1).expect("site").cost_total = -1.0;
        assert!(store.validate().is_err());
        store.site_mut(1).expect("site").cost_total = f64::INFINITY;
        assert!(store.validate().is_err());
    }

    #[test]
    fn priced_service_payments_round_trip() {
        let mut store = ServiceStore::new(2026);
        store
            .add_priced_site_with_lead_days(
                1,
                ServiceKind::Clinic,
                ServiceTier::Improved,
                [1.0, 2.0, 3.0],
                2.0,
                5_000.0,
            )
            .expect("site");
        store
            .site_mut(1)
            .expect("site")
            .mark_paid(2_500.0)
            .expect("installment");
        let json = store.to_json_pretty().expect("json");
        let decoded = ServiceStore::from_json(&json).expect("decode");
        assert_eq!(decoded, store);
        assert_eq!(decoded.checksum(), store.checksum());
        assert_eq!(decoded.payment_due(1), 0.0);
        let mut decoded = decoded;
        decoded.tick(2.0).expect("tick");
        assert_eq!(decoded.payment_due(1), 2_500.0);
        decoded
            .site_mut(1)
            .expect("site")
            .mark_paid(2_500.0)
            .expect("second installment");
        assert_eq!(decoded.total_payment_due(), 0.0);
    }
}
