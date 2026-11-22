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

/// Campaign lead time before a new utility site is operational, in game days.
///
/// Developer mode keeps the instant default; only the opt-in campaign clock
/// (`ConstructionTimeScale::CampaignDays`) assigns this lead time.
pub const CAMPAIGN_UTILITY_BUILD_DAYS: f32 = 1.0;

/// Largest accepted site lead time, a guard against malformed saves.
pub const MAX_UTILITY_BUILD_DAYS: f32 = 365.0;

/// Share of the construction cost due while a site is still building.
///
/// The remainder is due once the lead time elapses and the site becomes
/// operational. Developer-mode sites have no lead time and owe the full cost
/// immediately.
pub const UTILITY_BUILD_PAYMENT_SHARE: f64 = 0.5;

fn is_zero_f32(value: &f32) -> bool {
    *value == 0.0
}

fn is_zero_f64(value: &f64) -> bool {
    *value == 0.0
}

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

    /// Campaign lead time before a new site is operational, in game days.
    ///
    /// Both utility kinds take one game day. Developer mode ignores this and
    /// adds sites immediately operational.
    #[must_use]
    pub fn campaign_build_days(self) -> f32 {
        CAMPAIGN_UTILITY_BUILD_DAYS
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
    /// Water is 2,500; power is 5,000. The bridge charges this through the
    /// city treasury in two installments when the opt-in campaign clock is
    /// active; developer mode charges it immediately.
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

impl UtilitySite {
    /// Default site for a kind.
    #[must_use]
    pub fn default_for(kind: UtilityKind) -> Self {
        match kind {
            UtilityKind::Water => Self {
                center: [0.0, 0.0, 0.0],
                radius_m: DEFAULT_WATER_RADIUS_M,
                capacity: WATER_CAPACITY_PER_SITE,
                build_days_remaining: 0.0,
                cost_total: 0.0,
                cost_paid: 0.0,
            },
            UtilityKind::Power => Self {
                center: [0.0, 0.0, 0.0],
                radius_m: DEFAULT_POWER_RADIUS_M,
                capacity: POWER_CAPACITY_PER_SITE,
                build_days_remaining: 0.0,
                cost_total: 0.0,
                cost_paid: 0.0,
            },
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
    /// time owes [`UTILITY_BUILD_PAYMENT_SHARE`] of it. A zero-cost legacy
    /// site always owes zero.
    #[must_use]
    pub fn target_paid(&self) -> f64 {
        if self.cost_total <= 0.0 {
            return 0.0;
        }
        if self.is_operational() {
            self.cost_total
        } else {
            self.cost_total * UTILITY_BUILD_PAYMENT_SHARE
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
    pub fn mark_paid(&mut self, amount: f64) -> Result<(), UtilityError> {
        let due = self.payment_due();
        if !amount.is_finite() || amount < 0.0 || amount > due {
            return Err(UtilityError::Invalid(
                "payment amount must be finite and within the amount due",
            ));
        }
        self.cost_paid = (self.cost_paid + amount).min(self.target_paid());
        Ok(())
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

    /// Sites of one kind that are operational.
    #[must_use]
    pub fn operational_site_count(&self, kind: UtilityKind) -> usize {
        self.sites(kind)
            .values()
            .filter(|site| site.is_operational())
            .count()
    }

    /// Operational sites across both kinds.
    #[must_use]
    pub fn operational_total(&self) -> usize {
        UtilityKind::ALL
            .iter()
            .map(|&kind| self.operational_site_count(kind))
            .sum()
    }

    /// Operational sites across both kinds; alias for [`Self::operational_total`].
    #[must_use]
    pub fn ready_count(&self) -> usize {
        self.operational_total()
    }

    /// Sites still inside their lead time.
    #[must_use]
    pub fn building_count(&self) -> usize {
        self.site_count() - self.ready_count()
    }

    /// Sites of one kind.
    #[must_use]
    pub fn sites(&self, kind: UtilityKind) -> &BTreeMap<u64, UtilitySite> {
        match kind {
            UtilityKind::Water => &self.water_sites,
            UtilityKind::Power => &self.power_sites,
        }
    }

    /// One site of a kind.
    #[must_use]
    pub fn site(&self, placement_id: u64, kind: UtilityKind) -> Option<&UtilitySite> {
        self.sites(kind).get(&placement_id)
    }

    /// One site of a kind, mutable.
    #[must_use]
    pub fn site_mut(&mut self, placement_id: u64, kind: UtilityKind) -> Option<&mut UtilitySite> {
        self.sites_mut(kind).get_mut(&placement_id)
    }

    /// Construction price still owed on one site.
    #[must_use]
    pub fn payment_due(&self, placement_id: u64, kind: UtilityKind) -> f64 {
        self.site(placement_id, kind)
            .map_or(0.0, UtilitySite::payment_due)
    }

    /// Construction price still owed across both kinds.
    #[must_use]
    pub fn total_payment_due(&self) -> f64 {
        self.water_sites
            .values()
            .chain(self.power_sites.values())
            .map(UtilitySite::payment_due)
            .sum()
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

    /// Operating upkeep of every operational site of one kind per game day.
    ///
    /// Sites still inside their campaign lead time are not staffed yet, so
    /// they do not bill operating upkeep.
    #[must_use]
    pub fn upkeep_per_day_of(&self, kind: UtilityKind) -> f64 {
        self.sites(kind)
            .values()
            .filter(|site| site.is_operational())
            .map(|_| kind.upkeep_per_day())
            .sum()
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
    ///
    /// The site is immediately operational; use
    /// [`Self::add_site_with_lead_days`] for the campaign lead time.
    pub fn add_site(
        &mut self,
        placement_id: u64,
        kind: UtilityKind,
        center: [f32; 3],
    ) -> Result<(), UtilityError> {
        self.add_site_with_lead_days(placement_id, kind, center, 0.0)
    }

    /// Add or replace a utility site with a lead time in game days.
    ///
    /// `lead_days` must be finite and within `0.0..=MAX_UTILITY_BUILD_DAYS`.
    /// A non-zero lead time hides the site from supply, coverage, and
    /// operating upkeep until [`Self::tick`] clears it. The site is added as
    /// a legacy zero-cost site; use [`Self::add_priced_site_with_lead_days`]
    /// to record a construction price and two-installment schedule.
    pub fn add_site_with_lead_days(
        &mut self,
        placement_id: u64,
        kind: UtilityKind,
        center: [f32; 3],
        lead_days: f32,
    ) -> Result<(), UtilityError> {
        self.add_priced_site_with_lead_days(placement_id, kind, center, lead_days, 0.0)
    }

    /// Add or replace a utility site with a lead time and construction price.
    ///
    /// `lead_days` must be finite and within `0.0..=MAX_UTILITY_BUILD_DAYS`.
    /// `cost_total` must be finite and non-negative; zero keeps the legacy
    /// prepaid behavior. A priced site owes half of `cost_total` while it is
    /// building and the remainder once it is operational.
    pub fn add_priced_site_with_lead_days(
        &mut self,
        placement_id: u64,
        kind: UtilityKind,
        center: [f32; 3],
        lead_days: f32,
        cost_total: f64,
    ) -> Result<(), UtilityError> {
        if !lead_days.is_finite() || !(0.0..=MAX_UTILITY_BUILD_DAYS).contains(&lead_days) {
            return Err(UtilityError::Invalid("site lead days"));
        }
        if !cost_total.is_finite() || cost_total < 0.0 {
            return Err(UtilityError::Invalid("site construction cost"));
        }
        if placement_id > i64::MAX as u64 {
            return Err(UtilityError::Invalid("site id is above i64::MAX"));
        }
        let sites = self.sites_mut(kind);
        if !sites.contains_key(&placement_id) && sites.len() >= MAX_UTILITY_SITES {
            return Err(UtilityError::TooManySites(sites.len()));
        }
        let (paid, remaining) = sites
            .get(&placement_id)
            .map_or((0.0, lead_days), |existing| {
                (
                    existing.cost_paid.min(cost_total),
                    existing.build_days_remaining.min(lead_days),
                )
            });
        let mut site = UtilitySite::default_for(kind);
        site.center = center;
        site.build_days_remaining = remaining;
        site.cost_total = cost_total;
        site.cost_paid = paid;
        sites.insert(placement_id, site);
        Ok(())
    }

    /// Advance every site's lead time by `days`, clamping at zero.
    ///
    /// Negative or non-finite input is rejected rather than silently ignored.
    /// A zero step is a valid no-op.
    pub fn tick(&mut self, days: f32) -> Result<(), UtilityError> {
        if !days.is_finite() || days < 0.0 {
            return Err(UtilityError::Invalid("tick days"));
        }
        if days == 0.0 {
            return Ok(());
        }
        for sites in [&mut self.water_sites, &mut self.power_sites] {
            for site in sites.values_mut() {
                site.build_days_remaining = (site.build_days_remaining - days).max(0.0);
            }
        }
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
    /// least one operational site of that kind is within its radius. Served
    /// demand is `min(operational capacity, covered demand)`, so uncovered
    /// demand also lowers the city-wide service share. Sites still inside
    /// their campaign lead time contribute no capacity or coverage.
    pub fn refresh(&mut self, consumers: &[UtilityConsumer]) -> UtilityReport {
        let water_demand: f64 = consumers.iter().map(UtilityConsumer::water_demand).sum();
        let power_demand: f64 = consumers.iter().map(UtilityConsumer::power_demand).sum();
        let water_capacity: f64 = self
            .water_sites
            .values()
            .filter(|site| site.is_operational())
            .map(|site| site.capacity)
            .sum();
        let power_capacity: f64 = self
            .power_sites
            .values()
            .filter(|site| site.is_operational())
            .map(|site| site.capacity)
            .sum();
        let water_covered = covered_demand(consumers, &self.water_sites, UtilityDemand::Water);
        let power_covered = covered_demand(consumers, &self.power_sites, UtilityDemand::Power);
        let water = supply_share(
            water_demand,
            water_covered,
            water_capacity,
            self.operational_site_count(UtilityKind::Water),
        );
        let power = supply_share(
            power_demand,
            power_covered,
            power_capacity,
            self.operational_site_count(UtilityKind::Power),
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
                if !site.build_days_remaining.is_finite()
                    || !(0.0..=MAX_UTILITY_BUILD_DAYS).contains(&site.build_days_remaining)
                {
                    return Err(UtilityError::Invalid("site lead days"));
                }
                if !site.cost_total.is_finite() || site.cost_total < 0.0 {
                    return Err(UtilityError::Invalid("site construction cost"));
                }
                if !site.cost_paid.is_finite()
                    || site.cost_paid < 0.0
                    || site.cost_paid > site.cost_total
                {
                    return Err(UtilityError::Invalid("site construction paid"));
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
        && sites
            .values()
            .any(|site| site.is_operational() && site_covers(site, consumer))
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
        let mut store = UtilityStore::new(2026);
        store
            .add_site(1, UtilityKind::Water, [0.0, 0.0, 0.0])
            .expect("site");
        store
            .water_sites
            .get_mut(&1)
            .expect("site")
            .build_days_remaining = -0.5;
        assert!(store.validate().is_err());
        store
            .water_sites
            .get_mut(&1)
            .expect("site")
            .build_days_remaining = f32::NAN;
        assert!(store.validate().is_err());
    }

    #[test]
    fn developer_sites_are_operational_and_omit_the_lead_field() {
        let mut store = UtilityStore::new(2026);
        store
            .add_site(1, UtilityKind::Water, [1.0, 2.0, 3.0])
            .expect("water");
        store
            .add_site(2, UtilityKind::Power, [4.0, 5.0, 6.0])
            .expect("power");
        assert_eq!(store.site_count(), 2);
        assert_eq!(store.ready_count(), 2);
        assert_eq!(store.building_count(), 0);
        assert_eq!(store.operational_total(), 2);
        let json = serde_json::to_string(&store).expect("json");
        assert!(!json.contains("build_days_remaining"));
        // A legacy save without the field decodes as immediately operational.
        let legacy: UtilityStore = serde_json::from_str(&json).expect("legacy");
        assert_eq!(legacy, store);
        assert!(legacy.water_sites.values().all(UtilitySite::is_operational));
        assert!(legacy.power_sites.values().all(UtilitySite::is_operational));
    }

    #[test]
    fn campaign_lead_time_hides_a_site_until_the_days_pass() {
        let mut store = UtilityStore::new(2026);
        store
            .add_site_with_lead_days(1, UtilityKind::Water, [0.0, 0.0, 0.0], 1.0)
            .expect("site");
        assert_eq!(store.site_count(), 1);
        assert_eq!(store.ready_count(), 0);
        assert_eq!(store.building_count(), 1);
        assert_eq!(store.operational_site_count(UtilityKind::Water), 0);
        assert_eq!(store.total_upkeep_per_day(), 0.0);
        let report = store.refresh(&[consumer(2, 4, 0, 0.0, 0.0)]);
        assert_eq!(report.water_supply, 0.0);
        assert_eq!(report.water_capacity, 0.0);
        assert_eq!(report.water_covered_buildings, 0);

        store.tick(0.5).expect("tick");
        assert_eq!(store.ready_count(), 0);
        assert_eq!(store.total_upkeep_per_day(), 0.0);
        assert_eq!(
            store.refresh(&[consumer(2, 4, 0, 0.0, 0.0)]).water_supply,
            0.0
        );

        store.tick(0.5).expect("tick");
        assert_eq!(store.ready_count(), 1);
        assert_eq!(store.building_count(), 0);
        assert_eq!(store.total_upkeep_per_day(), 2.0);
        let report = store.refresh(&[consumer(2, 4, 0, 0.0, 0.0)]);
        assert_eq!(report.water_supply, 1.0);
        assert_eq!(report.water_capacity, 300.0);
    }

    #[test]
    fn building_sites_add_no_capacity_or_upkeep() {
        let mut store = UtilityStore::new(2026);
        store
            .add_site(1, UtilityKind::Power, [0.0, 0.0, 0.0])
            .expect("ready");
        store
            .add_site_with_lead_days(2, UtilityKind::Power, [0.0, 0.0, 0.0], 2.0)
            .expect("building");
        assert_eq!(store.count_of(UtilityKind::Power), 2);
        assert_eq!(store.operational_site_count(UtilityKind::Power), 1);
        assert_eq!(store.upkeep_per_day_of(UtilityKind::Power), 3.0);
        assert_eq!(store.total_upkeep_per_day(), 3.0);
        // Only the operational site's 300 capacity is installed, even though
        // both sites cover the consumer.
        let report = store.refresh(&[consumer(3, 4, 1_200, 0.0, 0.0)]);
        assert_eq!(report.power_capacity, 300.0);
        assert_eq!(report.power_served, 300.0);
        assert!(report.power_supply < 0.25);
        assert!(report.power_coverage > 0.99);
        assert_eq!(report.power_sites, 2);
    }

    #[test]
    fn tick_rejects_invalid_days() {
        let mut store = UtilityStore::new(2026);
        store
            .add_site_with_lead_days(1, UtilityKind::Water, [0.0, 0.0, 0.0], 1.0)
            .expect("site");
        assert!(store.tick(f32::NAN).is_err());
        assert!(store.tick(f32::INFINITY).is_err());
        assert!(store.tick(-1.0).is_err());
        assert!(store.tick(0.0).is_ok());
        assert_eq!(store.water_sites[&1].build_days_remaining, 1.0);
    }

    #[test]
    fn lead_time_and_operational_state_round_trip() {
        let mut store = UtilityStore::new(2026);
        store
            .add_site_with_lead_days(1, UtilityKind::Water, [1.0, 2.0, 3.0], 2.5)
            .expect("site");
        store.tick(1.0).expect("tick");
        assert_eq!(store.water_sites[&1].build_days_remaining, 1.5);
        let json = store.to_json_pretty().expect("json");
        let decoded = UtilityStore::from_json(&json).expect("decode");
        assert_eq!(decoded, store);
        assert_eq!(decoded.water_sites[&1].build_days_remaining, 1.5);
        assert_eq!(decoded.checksum(), store.checksum());
    }

    #[test]
    fn a_priced_developer_site_owes_the_full_cost_at_add() {
        let mut store = UtilityStore::new(2026);
        store
            .add_priced_site_with_lead_days(1, UtilityKind::Water, [0.0; 3], 0.0, 2_500.0)
            .expect("water");
        store
            .add_priced_site_with_lead_days(2, UtilityKind::Power, [0.0; 3], 0.0, 5_000.0)
            .expect("power");
        let water = store.site(1, UtilityKind::Water).expect("water");
        assert!(water.is_operational());
        assert_eq!(water.target_paid(), 2_500.0);
        assert_eq!(water.payment_due(), 2_500.0);
        assert_eq!(store.payment_due(1, UtilityKind::Water), 2_500.0);
        assert_eq!(store.payment_due(2, UtilityKind::Power), 5_000.0);
        assert_eq!(store.total_payment_due(), 7_500.0);

        store
            .site_mut(1, UtilityKind::Water)
            .expect("water")
            .mark_paid(2_500.0)
            .expect("paid");
        store
            .site_mut(2, UtilityKind::Power)
            .expect("power")
            .mark_paid(5_000.0)
            .expect("paid");
        assert_eq!(store.total_payment_due(), 0.0);
    }

    #[test]
    fn a_priced_campaign_site_owes_half_while_building_and_half_when_ready() {
        let mut store = UtilityStore::new(2026);
        store
            .add_priced_site_with_lead_days(1, UtilityKind::Power, [0.0; 3], 1.0, 5_000.0)
            .expect("site");
        {
            let site = store.site(1, UtilityKind::Power).expect("site");
            assert!(!site.is_operational());
            assert_eq!(site.target_paid(), 2_500.0);
            assert_eq!(site.payment_due(), 2_500.0);
        }
        store
            .site_mut(1, UtilityKind::Power)
            .expect("site")
            .mark_paid(2_500.0)
            .expect("first installment");
        assert_eq!(store.payment_due(1, UtilityKind::Power), 0.0);

        store.tick(1.0).expect("tick");
        {
            let site = store.site(1, UtilityKind::Power).expect("site");
            assert!(site.is_operational());
            assert_eq!(site.target_paid(), 5_000.0);
            assert_eq!(site.payment_due(), 2_500.0);
        }
        store
            .site_mut(1, UtilityKind::Power)
            .expect("site")
            .mark_paid(2_500.0)
            .expect("second installment");
        let site = store.site(1, UtilityKind::Power).expect("site");
        assert_eq!(site.cost_paid, 5_000.0);
        assert_eq!(site.payment_due(), 0.0);
    }

    #[test]
    fn a_paid_installment_is_not_charged_twice() {
        let mut store = UtilityStore::new(2026);
        store
            .add_priced_site_with_lead_days(1, UtilityKind::Water, [0.0; 3], 1.0, 2_500.0)
            .expect("site");
        let site = store.site_mut(1, UtilityKind::Water).expect("site");
        site.mark_paid(1_250.0).expect("first installment");
        assert!(site.mark_paid(1_250.0).is_err());
        assert_eq!(site.cost_paid, 1_250.0);

        store.tick(1.0).expect("tick");
        let site = store.site_mut(1, UtilityKind::Water).expect("site");
        site.mark_paid(1_250.0).expect("second installment");
        assert!(site.mark_paid(1_250.0).is_err());
        assert!(site.mark_paid(0.0).is_ok());
        assert_eq!(site.cost_paid, 2_500.0);
    }

    #[test]
    fn an_unpriced_site_keeps_the_legacy_zero_cost_default() {
        let mut store = UtilityStore::new(2026);
        store
            .add_site(1, UtilityKind::Water, [0.0; 3])
            .expect("site");
        let site = store.site(1, UtilityKind::Water).expect("site");
        assert_eq!(site.cost_total, 0.0);
        assert_eq!(site.cost_paid, 0.0);
        assert_eq!(site.target_paid(), 0.0);
        assert_eq!(site.payment_due(), 0.0);
        assert_eq!(store.total_payment_due(), 0.0);
        let json = serde_json::to_string(&store).expect("json");
        assert!(!json.contains("cost_total"));
        assert!(!json.contains("cost_paid"));
    }

    #[test]
    fn invalid_site_cost_amounts_are_rejected() {
        let mut store = UtilityStore::new(2026);
        for invalid in [-1.0, f64::NAN, f64::INFINITY] {
            assert!(store
                .add_priced_site_with_lead_days(1, UtilityKind::Water, [0.0; 3], 0.0, invalid)
                .is_err());
        }
        assert_eq!(store.site_count(), 0);

        store
            .add_priced_site_with_lead_days(1, UtilityKind::Water, [0.0; 3], 0.0, 100.0)
            .expect("site");
        let site = store.site_mut(1, UtilityKind::Water).expect("site");
        assert!(site.mark_paid(-1.0).is_err());
        assert!(site.mark_paid(f64::NAN).is_err());
        assert!(site.mark_paid(100.5).is_err());
        assert!(site.mark_paid(100.0).is_ok());
        assert!(store.validate().is_ok());

        store
            .site_mut(1, UtilityKind::Water)
            .expect("site")
            .cost_paid = 101.0;
        assert!(store.validate().is_err());
        store
            .site_mut(1, UtilityKind::Water)
            .expect("site")
            .cost_paid = f64::NAN;
        assert!(store.validate().is_err());
        store
            .site_mut(1, UtilityKind::Water)
            .expect("site")
            .cost_total = -1.0;
        assert!(store.validate().is_err());
        store
            .site_mut(1, UtilityKind::Water)
            .expect("site")
            .cost_total = f64::INFINITY;
        assert!(store.validate().is_err());
    }

    #[test]
    fn priced_site_payments_round_trip() {
        let mut store = UtilityStore::new(2026);
        store
            .add_priced_site_with_lead_days(1, UtilityKind::Water, [1.0, 2.0, 3.0], 2.0, 2_500.0)
            .expect("site");
        store
            .site_mut(1, UtilityKind::Water)
            .expect("site")
            .mark_paid(1_250.0)
            .expect("installment");
        let json = store.to_json_pretty().expect("json");
        let decoded = UtilityStore::from_json(&json).expect("decode");
        assert_eq!(decoded, store);
        assert_eq!(decoded.checksum(), store.checksum());
        assert_eq!(decoded.payment_due(1, UtilityKind::Water), 0.0);

        let mut decoded = decoded;
        decoded.tick(2.0).expect("tick");
        assert_eq!(decoded.payment_due(1, UtilityKind::Water), 1_250.0);
        decoded
            .site_mut(1, UtilityKind::Water)
            .expect("site")
            .mark_paid(1_250.0)
            .expect("second installment");
        assert_eq!(decoded.total_payment_due(), 0.0);
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

    #[test]
    fn replacing_a_priced_site_preserves_paid_installments_and_lead() {
        let mut store = UtilityStore::new(2026);
        store
            .add_priced_site_with_lead_days(1, UtilityKind::Water, [0.0; 3], 1.0, 1_000.0)
            .unwrap();
        store
            .site_mut(1, UtilityKind::Water)
            .unwrap()
            .mark_paid(500.0)
            .unwrap();
        store
            .add_priced_site_with_lead_days(1, UtilityKind::Water, [0.0; 3], 2.0, 1_000.0)
            .unwrap();
        let site = store.site(1, UtilityKind::Water).unwrap();
        assert_eq!(site.cost_paid, 500.0);
        assert_eq!(site.build_days_remaining, 1.0);
        assert_eq!(site.payment_due(), 0.0);
    }
}
