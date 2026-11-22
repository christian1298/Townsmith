//! Deterministic construction sites and phased progress.
//!
//! Placing a blueprint creates a site; the final game builds it through trade
//! crews, material deliveries, and inspections (D-023). This module is the v0
//! state machine: seven phases, a shared crew pool, a material stockpile, and a
//! small JSON save format. It is deliberately engine-agnostic and does not
//! model individual workers, vehicles, or a full economy yet.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::economy::DAY_SECONDS;
use crate::trades::{Trade, TradeError, TradePool};

/// Current construction save format.
pub const CONSTRUCTION_FORMAT_VERSION: u16 = 1;

/// Maximum simulated step accepted by [`ConstructionStore::tick`].
pub const MAX_TICK_SECONDS: f64 = 3600.0;

/// Maximum construction sites in one save, a guard against malformed input.
///
/// The Phase 5 1M-city shape has 250,000 buildings and one site per building;
/// the cap leaves room for single-building sites up to the 1M-resident save
/// without letting a malformed save allocate unbounded memory.
pub const MAX_SITES: usize = 1_000_000;

/// Upper bound for one construction site's total placement price.
pub const MAX_SITE_COST: f64 = 1_000_000_000.0;

/// Default campaign pacing: one game day takes 60 real seconds at 1×.
pub const CAMPAIGN_DEFAULT_SECONDS_PER_GAME_DAY: f64 = 60.0;

/// Fastest accepted campaign pacing, in real seconds per game day.
pub const CAMPAIGN_MIN_SECONDS_PER_GAME_DAY: f64 = 10.0;

/// Slowest accepted campaign pacing, in real seconds per game day.
pub const CAMPAIGN_MAX_SECONDS_PER_GAME_DAY: f64 = 3600.0;

/// Solid litres in the cottage reference used by the campaign duration curve.
pub const CAMPAIGN_COTTAGE_VOXELS: u64 = 10_000;

/// Campaign build days for a cottage-scale reference building.
pub const CAMPAIGN_COTTAGE_DAYS: f64 = 3.0;

/// Smallest uncapped campaign build total; a cottage never finishes faster.
pub const CAMPAIGN_MIN_BUILD_DAYS: f64 = 2.0;

/// Largest uncapped campaign build total, a guard against malformed sizes.
pub const CAMPAIGN_MAX_BUILD_DAYS: f64 = 210.0;

/// Exponent that turns the size ratio into a campaign build-time factor.
pub const CAMPAIGN_SIZE_EXPONENT: f64 = 0.6;

/// Hard cap on one campaign phase (D-029).
pub const CAMPAIGN_MAX_PHASE_DAYS: f64 = 30.0;

/// Share of a campaign build over which its materials are delivered.
///
/// The v0 material gate is a stockpile check, not a per-phase procurement
/// model. In campaign mode a rate tied to the full multi-day build would make
/// every phase wait for its whole material target and roughly double the
/// authored durations, so procurement is modelled as the first tenth of the
/// build. A small stockpile still stalls the site.
pub const CAMPAIGN_MATERIAL_LEAD_FRACTION: f64 = 0.1;

fn is_zero_cost(value: &f64) -> bool {
    *value == 0.0
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// One construction phase, in execution order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstructionPhase {
    /// Survey and site preparation.
    Survey,
    /// Foundation and ground works.
    Foundation,
    /// Structure and framing.
    Structure,
    /// Envelope, roof, windows, and doors.
    Envelope,
    /// Utility rough-in.
    Utilities,
    /// Interior finish.
    Finish,
    /// Final inspection.
    Inspection,
    /// Complete and ready to occupy.
    Complete,
}

impl ConstructionPhase {
    /// Every phase, including complete.
    pub const ALL: [Self; 8] = [
        Self::Survey,
        Self::Foundation,
        Self::Structure,
        Self::Envelope,
        Self::Utilities,
        Self::Finish,
        Self::Inspection,
        Self::Complete,
    ];

    /// Stable code used by save files and HUDs.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Survey => "survey",
            Self::Foundation => "foundation",
            Self::Structure => "structure",
            Self::Envelope => "envelope",
            Self::Utilities => "utilities",
            Self::Finish => "finish",
            Self::Inspection => "inspection",
            Self::Complete => "complete",
        }
    }

    /// Human-readable label for the developer HUD.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Survey => "Survey",
            Self::Foundation => "Foundation",
            Self::Structure => "Structure",
            Self::Envelope => "Envelope",
            Self::Utilities => "Utilities",
            Self::Finish => "Finish",
            Self::Inspection => "Inspection",
            Self::Complete => "Complete",
        }
    }

    /// Crew slots needed for this phase.
    #[must_use]
    pub const fn crew_required(self) -> u16 {
        match self {
            Self::Survey | Self::Inspection => 1,
            Self::Foundation | Self::Envelope | Self::Finish => 3,
            Self::Structure | Self::Utilities => 4,
            Self::Complete => 0,
        }
    }

    /// Share of the total build time spent in this phase.
    #[must_use]
    pub const fn weight(self) -> f32 {
        match self {
            Self::Survey => 0.03,
            Self::Foundation => 0.17,
            Self::Structure => 0.35,
            Self::Envelope => 0.20,
            Self::Utilities => 0.10,
            Self::Finish => 0.12,
            Self::Inspection => 0.03,
            Self::Complete => 0.0,
        }
    }

    /// Share of the placement price billed for this phase (D-028).
    ///
    /// The installments mirror the build-time weights: Survey 3%, Foundation
    /// 17%, Structure 35%, Envelope 20%, Utilities 10%, Finish 12%, Inspection
    /// 3%, and Complete 0%.
    #[must_use]
    pub const fn billing_weight(self) -> f64 {
        match self {
            Self::Survey => 0.03,
            Self::Foundation => 0.17,
            Self::Structure => 0.35,
            Self::Envelope => 0.20,
            Self::Utilities => 0.10,
            Self::Finish => 0.12,
            Self::Inspection => 0.03,
            Self::Complete => 0.0,
        }
    }

    /// Preferred trade for this phase.
    #[must_use]
    pub const fn preferred_trade(self) -> Trade {
        match self {
            Self::Survey => Trade::Laborer,
            Self::Foundation => Trade::Mason,
            Self::Structure | Self::Envelope | Self::Finish => Trade::Carpenter,
            Self::Utilities => Trade::Electrician,
            Self::Inspection => Trade::Engineer,
            Self::Complete => Trade::Laborer,
        }
    }

    /// The next phase, or `Complete`.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Survey => Self::Foundation,
            Self::Foundation => Self::Structure,
            Self::Structure => Self::Envelope,
            Self::Envelope => Self::Utilities,
            Self::Utilities => Self::Finish,
            Self::Finish => Self::Inspection,
            Self::Inspection | Self::Complete => Self::Complete,
        }
    }

    /// Whether this phase is finished.
    #[must_use]
    pub const fn is_complete(self) -> bool {
        matches!(self, Self::Complete)
    }
}

impl fmt::Display for ConstructionPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// Clock used to turn construction phase weights into simulated seconds.
///
/// [`DeveloperSeconds`] is the default and keeps the original 8–120 second
/// vertical-slice curve, including its checksums. [`CampaignDays`] follows
/// D-029: phase targets are authored in game days on the same 86,400-second
/// simulated clock as population, needs, production, and the economy.
///
/// [`DeveloperSeconds`]: Self::DeveloperSeconds
/// [`CampaignDays`]: Self::CampaignDays
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstructionTimeScale {
    /// Original developer clock from [`total_seconds_for`].
    #[default]
    DeveloperSeconds,
    /// Campaign clock; phase targets are game days.
    CampaignDays {
        /// Real seconds per game day at 1×, used by the bridge and HUD.
        seconds_per_game_day: f64,
    },
}

impl ConstructionTimeScale {
    /// Default campaign pacing: 60 real seconds per game day.
    pub const DEFAULT_SECONDS_PER_GAME_DAY: f64 = CAMPAIGN_DEFAULT_SECONDS_PER_GAME_DAY;

    /// Fastest accepted campaign pacing.
    pub const MIN_SECONDS_PER_GAME_DAY: f64 = CAMPAIGN_MIN_SECONDS_PER_GAME_DAY;

    /// Slowest accepted campaign pacing.
    pub const MAX_SECONDS_PER_GAME_DAY: f64 = CAMPAIGN_MAX_SECONDS_PER_GAME_DAY;

    /// Build the default campaign clock.
    #[must_use]
    pub const fn campaign_default() -> Self {
        Self::CampaignDays {
            seconds_per_game_day: Self::DEFAULT_SECONDS_PER_GAME_DAY,
        }
    }

    /// Build a campaign clock after validating its real-time pacing.
    pub fn campaign(seconds_per_game_day: f64) -> Result<Self, ConstructionError> {
        let scale = Self::CampaignDays {
            seconds_per_game_day,
        };
        scale.validate()?;
        Ok(scale)
    }

    /// Whether this is the default developer clock.
    #[must_use]
    pub const fn is_developer(&self) -> bool {
        matches!(*self, Self::DeveloperSeconds)
    }

    /// Stable identifier used by reports and save files.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::DeveloperSeconds => "developer_seconds",
            Self::CampaignDays { .. } => "campaign_days",
        }
    }

    /// Campaign pacing in real seconds per game day, or `None` for developer.
    #[must_use]
    pub const fn seconds_per_game_day(&self) -> Option<f64> {
        match *self {
            Self::DeveloperSeconds => None,
            Self::CampaignDays {
                seconds_per_game_day,
            } => Some(seconds_per_game_day),
        }
    }

    /// Validate campaign pacing: finite and within 10–3600 s per game day.
    pub fn validate(&self) -> Result<(), ConstructionError> {
        if let Self::CampaignDays {
            seconds_per_game_day,
        } = *self
        {
            if !seconds_per_game_day.is_finite()
                || !(Self::MIN_SECONDS_PER_GAME_DAY..=Self::MAX_SECONDS_PER_GAME_DAY)
                    .contains(&seconds_per_game_day)
            {
                return Err(ConstructionError::InvalidTimeScale);
            }
        }
        Ok(())
    }

    /// Total build time for `solid_voxels` under this clock, in simulated seconds.
    #[must_use]
    pub fn total_seconds_for(self, solid_voxels: u64) -> f32 {
        match self {
            Self::DeveloperSeconds => total_seconds_for(solid_voxels),
            Self::CampaignDays { .. } => {
                let days: f64 = ConstructionPhase::ALL
                    .iter()
                    .map(|phase| campaign_phase_days_for(*phase, solid_voxels))
                    .sum();
                seconds_from_days(days)
            }
        }
    }

    /// Duration of one phase under this clock, in simulated seconds.
    #[must_use]
    pub fn phase_seconds_for(self, phase: ConstructionPhase, solid_voxels: u64) -> f32 {
        match self {
            Self::DeveloperSeconds => total_seconds_for(solid_voxels) * phase.weight(),
            Self::CampaignDays { .. } => {
                seconds_from_days(campaign_phase_days_for(phase, solid_voxels))
            }
        }
    }
}

fn seconds_from_days(days: f64) -> f32 {
    (days * DAY_SECONDS) as f32
}

/// Campaign build time in game days before the per-phase cap.
///
/// The curve is deterministic: the [`CAMPAIGN_COTTAGE_VOXELS`] reference takes
/// [`CAMPAIGN_COTTAGE_DAYS`], and larger buildings scale by
/// `(litres / reference)^CAMPAIGN_SIZE_EXPONENT`. Phase durations multiply the
/// total by the existing phase weights and then cap each phase at
/// [`CAMPAIGN_MAX_PHASE_DAYS`] (D-029).
#[must_use]
pub fn campaign_total_days_for(solid_voxels: u64) -> f64 {
    let ratio = solid_voxels.max(1) as f64 / CAMPAIGN_COTTAGE_VOXELS as f64;
    (CAMPAIGN_COTTAGE_DAYS * ratio.powf(CAMPAIGN_SIZE_EXPONENT))
        .clamp(CAMPAIGN_MIN_BUILD_DAYS, CAMPAIGN_MAX_BUILD_DAYS)
}

/// One campaign phase in game days, capped at [`CAMPAIGN_MAX_PHASE_DAYS`].
#[must_use]
pub fn campaign_phase_days_for(phase: ConstructionPhase, solid_voxels: u64) -> f64 {
    if phase.is_complete() {
        return 0.0;
    }
    (campaign_total_days_for(solid_voxels) * f64::from(phase.weight())).min(CAMPAIGN_MAX_PHASE_DAYS)
}

/// One construction site created by a placement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConstructionSite {
    /// Placement this site builds.
    pub placement_id: u64,
    /// Current phase.
    pub phase: ConstructionPhase,
    /// Progress within the current phase, `0.0..=1.0`.
    pub phase_progress: f32,
    /// Simulated seconds since the site was created.
    pub elapsed_s: f32,
    /// Total build time at full crew and material supply.
    pub total_s: f32,
    /// Material litres required for the whole building.
    pub materials_required: u64,
    /// Material litres delivered so far.
    pub materials_delivered: u64,
    /// Crew slots needed by the current phase.
    pub crew_required: u16,
    /// Crew slots assigned by the last tick.
    pub crew_assigned: u16,
    /// Why the site did not advance, if it stalled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_reason: Option<String>,
    /// Total placement price split across the construction phases.
    ///
    /// Zero is the legacy fully prepaid path: the site is never charged and
    /// the field is omitted from the save so existing checksums stay pinned.
    #[serde(default, skip_serializing_if = "is_zero_cost")]
    pub cost_total: f64,
    /// Placement price paid so far.
    ///
    /// Omitted from the save while zero, keeping legacy saves byte-identical.
    #[serde(default, skip_serializing_if = "is_zero_cost")]
    pub cost_paid: f64,
    /// Whether an unpaid installment stopped the site at a phase boundary.
    #[serde(default, skip_serializing_if = "is_false")]
    pub payment_stalled: bool,
}

impl ConstructionSite {
    /// Build a site for a placement with `solid_voxels` material litres.
    #[must_use]
    pub fn new(placement_id: u64, solid_voxels: u64) -> Self {
        let solid_voxels = solid_voxels.max(1);
        Self {
            placement_id,
            phase: ConstructionPhase::Survey,
            phase_progress: 0.0,
            elapsed_s: 0.0,
            total_s: total_seconds_for(solid_voxels),
            materials_required: solid_voxels,
            materials_delivered: 0,
            crew_required: ConstructionPhase::Survey.crew_required(),
            crew_assigned: 0,
            stall_reason: None,
            cost_total: 0.0,
            cost_paid: 0.0,
            payment_stalled: false,
        }
    }

    /// Overall build progress in `0.0..=1.0`.
    #[must_use]
    pub fn progress(&self) -> f32 {
        if self.phase.is_complete() {
            return 1.0;
        }
        let mut done = 0.0;
        for phase in ConstructionPhase::ALL {
            if phase == self.phase {
                return (done + phase.weight() * self.phase_progress).clamp(0.0, 1.0);
            }
            done += phase.weight();
        }
        0.0
    }

    /// Material litres that must be delivered before this phase can progress.
    #[must_use]
    pub fn phase_material_target(&self) -> u64 {
        let mut fraction = 0.0;
        for phase in ConstructionPhase::ALL {
            fraction += phase.weight();
            if phase == self.phase {
                break;
            }
        }
        ((self.materials_required as f64) * f64::from(fraction))
            .ceil()
            .min(self.materials_required as f64) as u64
    }

    /// Whether the site is complete.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.phase.is_complete()
    }

    /// Cumulative price that should be paid through the current phase.
    ///
    /// The Survey installment is due at placement; each later installment
    /// becomes due when the previous phase completes. At [`Complete`] the
    /// target is the full placement price.
    ///
    /// [`Complete`]: ConstructionPhase::Complete
    #[must_use]
    pub fn target_paid(&self) -> f64 {
        if self.cost_total <= 0.0 || self.phase.is_complete() {
            return self.cost_total;
        }
        let mut fraction = 0.0;
        for phase in ConstructionPhase::ALL {
            fraction += phase.billing_weight();
            if phase == self.phase {
                break;
            }
        }
        (self.cost_total * fraction).clamp(0.0, self.cost_total)
    }

    /// Price still owed for the current phase, never negative.
    #[must_use]
    pub fn payment_due(&self) -> f64 {
        (self.target_paid() - self.cost_paid).max(0.0)
    }

    /// Record an installment payment up to the amount currently due.
    ///
    /// The amount must be finite and within `0.0..=payment_due()`. Paying
    /// clears a payment stall; a zero-cost site always accepts zero.
    pub fn mark_paid(&mut self, amount: f64) -> Result<(), ConstructionError> {
        let due = self.payment_due();
        if !amount.is_finite() || amount < 0.0 || amount > due {
            return Err(ConstructionError::InvalidSite(
                "payment amount must be finite and within the amount due",
            ));
        }
        self.cost_paid = (self.cost_paid + amount).min(self.target_paid());
        self.resume_payment();
        Ok(())
    }

    /// Stop the site at a phase boundary because its installment is unpaid.
    ///
    /// Zero-cost and fully paid sites have nothing due, so this is a no-op.
    pub fn stall_for_payment(&mut self) {
        if self.is_complete() || self.payment_due() <= 0.0 {
            return;
        }
        self.payment_stalled = true;
        self.stall_reason = Some("payment".to_string());
    }

    /// Clear a payment stall after the installment has been paid.
    pub fn resume_payment(&mut self) {
        self.payment_stalled = false;
        if self.stall_reason.as_deref() == Some("payment") {
            self.stall_reason = None;
        }
    }

    fn set_phase(&mut self, phase: ConstructionPhase) {
        self.phase = phase;
        self.phase_progress = 0.0;
        self.crew_required = phase.crew_required();
        self.crew_assigned = 0;
        self.stall_reason = None;
    }
}

/// Build time for a building with `solid_voxels` litres, in seconds.
///
/// This is a placeholder tuning curve: small houses finish in seconds in the
/// developer slice, while a large project is capped at two minutes. The real
/// economy and trade availability replace it later.
#[must_use]
pub fn total_seconds_for(solid_voxels: u64) -> f32 {
    (8.0 + solid_voxels as f32 / 4_000.0).clamp(8.0, 120.0)
}

/// Result of one construction tick.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConstructionTick {
    /// Sites that completed during this tick.
    pub completed: Vec<u64>,
    /// Sites that changed phase during this tick.
    pub phase_changed: Vec<u64>,
    /// Sites still active.
    pub active: usize,
    /// Sites that stalled for crew or materials.
    pub stalled: usize,
}

/// One construction save with a shared crew pool and material stockpile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConstructionStore {
    /// Save format version.
    pub format_version: u16,
    /// Crew slots shared by every site.
    pub crew_capacity: u16,
    /// Material litres available for delivery.
    pub materials_stock: u64,
    /// Age-derived trade roster. Missing on legacy saves and the runner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trades: Option<TradePool>,
    /// Construction clock. Omitted from saves at the developer default so old
    /// saves load as [`ConstructionTimeScale::DeveloperSeconds`] and the
    /// legacy checksums stay pinned (D-029).
    #[serde(default, skip_serializing_if = "ConstructionTimeScale::is_developer")]
    pub time_scale: ConstructionTimeScale,
    /// Sites keyed by placement id.
    pub sites: BTreeMap<u64, ConstructionSite>,
}

impl Default for ConstructionStore {
    fn default() -> Self {
        Self::new(4, 1_000_000)
    }
}

impl ConstructionStore {
    /// Create an empty store.
    #[must_use]
    pub fn new(crew_capacity: u16, materials_stock: u64) -> Self {
        Self {
            format_version: CONSTRUCTION_FORMAT_VERSION,
            crew_capacity,
            materials_stock,
            trades: None,
            time_scale: ConstructionTimeScale::default(),
            sites: BTreeMap::new(),
        }
    }

    /// Number of sites.
    #[must_use]
    pub fn site_count(&self) -> usize {
        self.sites.len()
    }

    /// One site.
    #[must_use]
    pub fn site(&self, placement_id: u64) -> Option<&ConstructionSite> {
        self.sites.get(&placement_id)
    }

    /// One site, mutable, for recording payments and stalls.
    #[must_use]
    pub fn site_mut(&mut self, placement_id: u64) -> Option<&mut ConstructionSite> {
        self.sites.get_mut(&placement_id)
    }

    /// Add a site for a placement with no placement price. Repeating an id is rejected.
    ///
    /// This is the legacy fully prepaid path; use [`Self::add_priced_site`] to
    /// track the phased billing schedule.
    pub fn add_site(
        &mut self,
        placement_id: u64,
        solid_voxels: u64,
    ) -> Result<&ConstructionSite, ConstructionError> {
        self.add_priced_site(placement_id, solid_voxels, 0.0)
    }

    /// Add a site with a placement price split across construction phases.
    ///
    /// A zero `cost_total` keeps the legacy prepaid behavior and is never
    /// charged. Repeating an id is rejected.
    pub fn add_priced_site(
        &mut self,
        placement_id: u64,
        solid_voxels: u64,
        cost_total: f64,
    ) -> Result<&ConstructionSite, ConstructionError> {
        if !cost_total.is_finite() || cost_total < 0.0 || cost_total > MAX_SITE_COST {
            return Err(ConstructionError::InvalidSite(
                "site cost must be finite and within the supported range",
            ));
        }
        if placement_id == 0 {
            return Err(ConstructionError::InvalidSite(
                "placement id must be positive",
            ));
        }
        if self.sites.contains_key(&placement_id) {
            return Err(ConstructionError::DuplicateSite(placement_id));
        }
        if self.sites.len() >= MAX_SITES {
            return Err(ConstructionError::TooManySites(MAX_SITES));
        }
        let mut site = ConstructionSite::new(placement_id, solid_voxels);
        site.cost_total = cost_total;
        self.sites.insert(placement_id, site);
        Ok(self.sites.get(&placement_id).expect("just inserted"))
    }

    /// Price due for one site; zero when the site is missing.
    #[must_use]
    pub fn payment_due(&self, placement_id: u64) -> f64 {
        self.sites
            .get(&placement_id)
            .map_or(0.0, ConstructionSite::payment_due)
    }

    /// Total price due across every site, in ascending placement order.
    #[must_use]
    pub fn total_payment_due(&self) -> f64 {
        self.sites.values().map(ConstructionSite::payment_due).sum()
    }

    /// Remove a site.
    pub fn remove_site(&mut self, placement_id: u64) -> bool {
        self.sites.remove(&placement_id).is_some()
    }

    /// Remove every site.
    pub fn clear(&mut self) {
        self.sites.clear();
    }

    /// Set the shared crew pool.
    pub fn set_crew_capacity(&mut self, crew_capacity: u16) {
        self.crew_capacity = crew_capacity;
    }

    /// Replace the trade roster. `None` restores the legacy aggregate crew.
    pub fn set_trades(&mut self, trades: Option<TradePool>) {
        self.trades = trades;
    }

    /// Replace the construction clock after validating campaign pacing.
    pub fn set_time_scale(
        &mut self,
        scale: ConstructionTimeScale,
    ) -> Result<(), ConstructionError> {
        scale.validate()?;
        self.time_scale = scale;
        Ok(())
    }

    /// Current construction clock.
    #[must_use]
    pub const fn time_scale(&self) -> ConstructionTimeScale {
        self.time_scale
    }

    /// Total build time for `solid_voxels` under the current clock.
    ///
    /// Developer mode returns [`total_seconds_for`] exactly. Campaign mode
    /// sums the capped phase targets on the 86,400-second simulation clock.
    #[must_use]
    pub fn total_seconds_for_scaled(&self, solid_voxels: u64) -> f32 {
        self.time_scale.total_seconds_for(solid_voxels)
    }

    /// Crew slots actually available after the trade roster cap.
    #[must_use]
    pub fn effective_crew_capacity(&self) -> u16 {
        self.trades.as_ref().map_or(self.crew_capacity, |trades| {
            self.crew_capacity.min(trades.total())
        })
    }

    /// Preferred trade for the current phase, falling back to a laborer.
    #[must_use]
    pub fn site_trade(&self, placement_id: u64) -> Option<Trade> {
        let site = self.sites.get(&placement_id)?;
        let preferred = site.phase.preferred_trade();
        Some(
            self.trades
                .as_ref()
                .filter(|trades| trades.has(preferred))
                .map_or(Trade::Laborer, |_| preferred),
        )
    }

    /// Add material litres to the stockpile.
    pub fn deliver_materials(&mut self, litres: u64) {
        self.materials_stock = self.materials_stock.saturating_add(litres);
    }

    /// Advance all sites by `delta_seconds`.
    ///
    /// Crew is assigned per phase. A large step still needs the next phase's
    /// crew before it can cross that boundary; it cannot fast-forward through
    /// phases with a smaller pool.
    pub fn tick(&mut self, delta_seconds: f64) -> ConstructionTick {
        // NaN survives `clamp` and would poison every site it reaches, so a
        // non-finite step decays to a no-op instead of being clamped.
        let dt = if delta_seconds.is_finite() {
            delta_seconds.clamp(0.0, MAX_TICK_SECONDS) as f32
        } else {
            0.0
        };
        let mut report = ConstructionTick::default();
        if dt <= 0.0 {
            report.active = self
                .sites
                .values()
                .filter(|site| !site.is_complete())
                .count();
            return report;
        }

        let mut remaining_crew = self.effective_crew_capacity();
        let mut remaining_materials = self.materials_stock;
        let time_scale = self.time_scale;
        let developer_clock = time_scale.is_developer();

        for site in self.sites.values_mut() {
            if site.is_complete() {
                site.crew_assigned = 0;
                site.stall_reason = None;
                continue;
            }

            if site.payment_stalled {
                site.crew_assigned = 0;
                site.stall_reason = Some("payment".to_string());
                continue;
            }

            let required = site.crew_required;
            let mut site_crew = required.min(remaining_crew);
            if site_crew < required {
                site.crew_assigned = site_crew;
                site.stall_reason = Some("crew".to_string());
                continue;
            }
            remaining_crew -= site_crew;

            // Developer mode keeps the exact legacy curve and arithmetic.
            // Campaign mode derives the effective duration from the authoring
            // curve; `site.total_s` stays the saved developer baseline.
            let total_s = if developer_clock {
                site.total_s
            } else {
                time_scale.total_seconds_for(site.materials_required)
            };
            let material_seconds = if developer_clock {
                site.total_s
            } else {
                total_s * CAMPAIGN_MATERIAL_LEAD_FRACTION as f32
            };
            let rate =
                (site.materials_required as f64 / f64::from(material_seconds.max(1.0))).max(1.0);
            let wanted = (rate * f64::from(dt)).ceil() as u64;
            let delivered = wanted.min(remaining_materials).min(
                site.materials_required
                    .saturating_sub(site.materials_delivered),
            );
            site.materials_delivered += delivered;
            remaining_materials -= delivered;
            site.elapsed_s = (site.elapsed_s + dt).min(total_s * 4.0);

            if site.materials_delivered < site.phase_material_target() {
                remaining_crew += site_crew;
                site.crew_assigned = 0;
                site.stall_reason = Some("materials".to_string());
                continue;
            }

            let mut left = dt;
            while left > 0.0 && !site.is_complete() {
                let duration = if developer_clock {
                    site.total_s * site.phase.weight()
                } else {
                    time_scale.phase_seconds_for(site.phase, site.materials_required)
                };
                if duration <= f32::EPSILON {
                    let next = site.phase.next();
                    site.set_phase(next);
                    site.crew_assigned = site_crew;
                    report.phase_changed.push(site.placement_id);
                    if next.is_complete() {
                        remaining_crew += site_crew;
                        site.crew_assigned = 0;
                        site.phase_progress = 1.0;
                        report.completed.push(site.placement_id);
                    }
                    continue;
                }
                let needed = (1.0 - site.phase_progress) * duration;
                if left < needed {
                    site.phase_progress += left / duration;
                    left = 0.0;
                } else {
                    left -= needed;
                    let next = site.phase.next();
                    let next_required = next.crew_required();
                    if site_crew < next_required {
                        let extra = next_required - site_crew;
                        if extra > remaining_crew {
                            site.set_phase(next);
                            site.crew_assigned = site_crew;
                            site.stall_reason = Some("crew".to_string());
                            break;
                        }
                        remaining_crew -= extra;
                        site_crew += extra;
                    } else if site_crew > next_required {
                        remaining_crew += site_crew - next_required;
                        site_crew = next_required;
                    }
                    site.set_phase(next);
                    site.crew_assigned = site_crew;
                    report.phase_changed.push(site.placement_id);
                    if next.is_complete() {
                        remaining_crew += site_crew;
                        site.crew_assigned = 0;
                        site.phase_progress = 1.0;
                        report.completed.push(site.placement_id);
                        break;
                    }
                    if site.materials_delivered < site.phase_material_target() {
                        remaining_crew += site_crew;
                        site.crew_assigned = 0;
                        site.stall_reason = Some("materials".to_string());
                        break;
                    }
                }
            }

            if site.is_complete() {
                site.crew_assigned = 0;
                site.phase_progress = 1.0;
                site.stall_reason = None;
            } else {
                if site.crew_assigned > site.crew_required {
                    remaining_crew += site.crew_assigned - site.crew_required;
                    site.crew_assigned = site.crew_required;
                }
                if site.crew_assigned < site.crew_required {
                    site.stall_reason = Some("crew".to_string());
                }
            }
        }

        self.materials_stock = remaining_materials;
        report.active = self
            .sites
            .values()
            .filter(|site| !site.is_complete())
            .count();
        report.stalled = self
            .sites
            .values()
            .filter(|site| !site.is_complete() && site.stall_reason.is_some())
            .count();
        report
    }

    /// Serialize as pretty JSON.
    pub fn to_json_pretty(&self) -> Result<String, ConstructionError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse and validate a construction save.
    pub fn from_json(json: &str) -> Result<Self, ConstructionError> {
        let store: Self = serde_json::from_str(json)?;
        store.validate()?;
        Ok(store)
    }

    /// Validate all ranges and state invariants.
    pub fn validate(&self) -> Result<(), ConstructionError> {
        if self.format_version != CONSTRUCTION_FORMAT_VERSION {
            return Err(ConstructionError::UnsupportedVersion(self.format_version));
        }
        if self.crew_capacity > 64 {
            return Err(ConstructionError::InvalidSite("crew capacity is too large"));
        }
        self.time_scale.validate()?;
        if let Some(trades) = &self.trades {
            trades.validate()?;
        }
        if self.sites.len() > MAX_SITES {
            return Err(ConstructionError::TooManySites(self.sites.len()));
        }
        for (id, site) in &self.sites {
            if *id == 0 || site.placement_id != *id {
                return Err(ConstructionError::InvalidSite("site id does not match"));
            }
            if !site.total_s.is_finite() || site.total_s <= 0.0 {
                return Err(ConstructionError::InvalidSite(
                    "build time must be positive",
                ));
            }
            if !site.cost_total.is_finite()
                || site.cost_total < 0.0
                || site.cost_total > MAX_SITE_COST
            {
                return Err(ConstructionError::InvalidSite(
                    "site cost must be finite and within the supported range",
                ));
            }
            if !site.cost_paid.is_finite()
                || site.cost_paid < 0.0
                || site.cost_paid > site.cost_total
            {
                return Err(ConstructionError::InvalidSite(
                    "paid cost must be finite and within the site cost",
                ));
            }
            if !site.phase_progress.is_finite() || !(0.0..=1.0).contains(&site.phase_progress) {
                return Err(ConstructionError::InvalidSite("phase progress is invalid"));
            }
            if !site.elapsed_s.is_finite() || site.elapsed_s < 0.0 {
                return Err(ConstructionError::InvalidSite("elapsed time is invalid"));
            }
            if site.materials_delivered > site.materials_required {
                return Err(ConstructionError::InvalidSite(
                    "delivered materials exceed the requirement",
                ));
            }
            if site.crew_assigned > site.crew_required {
                return Err(ConstructionError::InvalidSite("crew assignment is invalid"));
            }
            if site.is_complete() && site.phase_progress < 1.0 {
                return Err(ConstructionError::InvalidSite(
                    "complete site is not fully progressed",
                ));
            }
        }
        Ok(())
    }

    /// Stable FNV-1a checksum of the compact JSON.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let bytes = serde_json::to_vec(self).expect("construction store serialization cannot fail");
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
}

/// Construction state or persistence error.
#[derive(Debug)]
pub enum ConstructionError {
    /// JSON serialization or parse error.
    Json(serde_json::Error),
    /// Unsupported save format.
    UnsupportedVersion(u16),
    /// A site is structurally invalid.
    InvalidSite(&'static str),
    /// A placement already has a site.
    DuplicateSite(u64),
    /// Too many sites.
    TooManySites(usize),
    /// Invalid trade roster.
    Trade(TradeError),
    /// Invalid campaign time scale.
    InvalidTimeScale,
}

impl fmt::Display for ConstructionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "construction JSON error: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(
                    formatter,
                    "unsupported construction format version {version}"
                )
            }
            Self::InvalidSite(message) => write!(formatter, "invalid construction site: {message}"),
            Self::DuplicateSite(id) => write!(formatter, "placement {id} already has a site"),
            Self::TooManySites(count) => write!(formatter, "too many construction sites: {count}"),
            Self::Trade(error) => write!(formatter, "invalid construction trades: {error}"),
            Self::InvalidTimeScale => {
                write!(formatter, "invalid construction time scale")
            }
        }
    }
}

impl std::error::Error for ConstructionError {}

impl From<serde_json::Error> for ConstructionError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<TradeError> for ConstructionError {
    fn from(error: TradeError) -> Self {
        Self::Trade(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(store: &ConstructionStore, id: u64) -> &ConstructionSite {
        store.site(id).expect("site")
    }

    #[test]
    fn placement_creates_a_survey_site() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_site(1, 64_224).expect("site");
        let site = site(&store, 1);
        assert_eq!(site.phase, ConstructionPhase::Survey);
        assert_eq!(site.materials_required, 64_224);
        assert_eq!(site.progress(), 0.0);
        assert!(site.total_s > 20.0);
    }

    #[test]
    fn full_crew_and_materials_complete_the_build() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_site(1, 10_000).expect("site");
        let report = store.tick(1_000.0);
        assert_eq!(report.completed, vec![1]);
        assert!(site(&store, 1).is_complete());
        assert_eq!(site(&store, 1).progress(), 1.0);
        assert_eq!(site(&store, 1).phase_progress, 1.0);
    }

    #[test]
    fn no_crew_stalls_the_site() {
        let mut store = ConstructionStore::new(0, 1_000_000);
        store.add_site(1, 10_000).expect("site");
        let report = store.tick(10.0);
        assert!(report.completed.is_empty());
        assert_eq!(report.stalled, 1);
        assert_eq!(site(&store, 1).stall_reason.as_deref(), Some("crew"));
        assert_eq!(site(&store, 1).phase, ConstructionPhase::Survey);
    }

    #[test]
    fn large_step_cannot_skip_crew_requirements() {
        let mut store = ConstructionStore::new(1, 1_000_000);
        store.add_site(1, 64_224).expect("site");
        let report = store.tick(1_000.0);
        assert!(report.completed.is_empty());
        let site = site(&store, 1);
        assert!(!site.is_complete());
        assert_eq!(site.phase, ConstructionPhase::Foundation);
        assert_eq!(site.stall_reason.as_deref(), Some("crew"));
    }

    #[test]
    fn crew_stall_does_not_consume_materials() {
        let mut store = ConstructionStore::new(0, 100_000);
        store.add_site(1, 10_000).expect("site");
        store.tick(1_000.0);
        assert_eq!(store.materials_stock, 100_000);
        assert_eq!(site(&store, 1).materials_delivered, 0);
    }

    #[test]
    fn missing_materials_stalls_until_delivery() {
        let mut store = ConstructionStore::new(4, 0);
        store.add_site(1, 10_000).expect("site");
        let report = store.tick(10.0);
        assert_eq!(report.stalled, 1);
        assert_eq!(site(&store, 1).stall_reason.as_deref(), Some("materials"));
        store.deliver_materials(10_000);
        let report = store.tick(1_000.0);
        assert_eq!(report.completed, vec![1]);
    }

    #[test]
    fn crew_is_shared_between_sites() {
        let mut store = ConstructionStore::new(1, 1_000_000);
        store.add_site(1, 10_000).expect("site");
        store.add_site(2, 10_000).expect("site");
        let report = store.tick(1.0);
        assert!(report.stalled >= 1);
        assert_eq!(site(&store, 1).crew_assigned, 1);
        assert_eq!(site(&store, 2).crew_assigned, 0);
        assert_eq!(site(&store, 2).stall_reason.as_deref(), Some("crew"));
    }

    #[test]
    fn progress_is_monotonic_and_finishes() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_site(1, 20_000).expect("site");
        let mut previous = 0.0;
        for _ in 0..20 {
            store.tick(1.0);
            let progress = site(&store, 1).progress();
            assert!(progress >= previous);
            previous = progress;
        }
        assert!(previous > 0.0);
        store.tick(1_000.0);
        assert!(site(&store, 1).is_complete());
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_site(1, 10_000).expect("site");
        assert!(matches!(
            store.add_site(1, 10_000),
            Err(ConstructionError::DuplicateSite(1))
        ));
        assert!(store.add_site(0, 10_000).is_err());
    }

    #[test]
    fn trade_roster_caps_the_effective_crew() {
        let mut store = ConstructionStore::new(10, 1_000_000);
        let mut trades = TradePool::new();
        trades.set(Trade::Laborer, 3);
        store.set_trades(Some(trades));
        assert_eq!(store.effective_crew_capacity(), 3);
        store.add_site(1, 10_000).expect("site");
        let mutable = store.sites.get_mut(&1).expect("site");
        mutable.phase = ConstructionPhase::Structure;
        mutable.crew_required = 4;
        store.tick(1.0);
        let site = site(&store, 1);
        assert_eq!(site.crew_assigned, 3);
        assert_eq!(site.stall_reason.as_deref(), Some("crew"));
    }

    #[test]
    fn trade_roster_round_trips_and_legacy_saves_stay_undefined() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_site(1, 10_000).expect("site");
        store.set_trades(Some(TradePool::for_age(crate::progression::Age::Wood)));
        let json = store.to_json_pretty().expect("json");
        let decoded = ConstructionStore::from_json(&json).expect("decode");
        assert_eq!(decoded.trades, store.trades);
        assert_eq!(decoded.checksum(), store.checksum());

        let mut value: serde_json::Value = serde_json::from_str(&json).expect("value");
        value.as_object_mut().expect("object").remove("trades");
        let legacy = ConstructionStore::from_json(&value.to_string()).expect("legacy");
        assert!(legacy.trades.is_none());
    }

    #[test]
    fn phase_trades_fall_back_when_specialists_are_missing() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.set_trades(Some(TradePool::for_age(crate::progression::Age::Wood)));
        store.add_site(1, 10_000).expect("site");
        assert_eq!(store.site_trade(1), Some(Trade::Laborer));
        store.sites.get_mut(&1).expect("site").phase = ConstructionPhase::Foundation;
        assert_eq!(store.site_trade(1), Some(Trade::Mason));
        store.sites.get_mut(&1).expect("site").phase = ConstructionPhase::Utilities;
        assert_eq!(store.site_trade(1), Some(Trade::Laborer));
    }

    #[test]
    fn json_round_trip_and_checksum() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_site(1, 64_224).expect("site");
        store.tick(10.0);
        let json = store.to_json_pretty().expect("json");
        let decoded = ConstructionStore::from_json(&json).expect("decode");
        assert_eq!(decoded, store);
        assert_eq!(decoded.checksum(), store.checksum());
    }

    #[test]
    fn malformed_state_is_rejected() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_site(1, 10_000).expect("site");
        store.sites.get_mut(&1).expect("site").materials_delivered = 10_001;
        assert!(store.validate().is_err());
        assert!(ConstructionStore::from_json("{}").is_err());
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn billing_weights_match_the_phase_split() {
        let sum: f64 = ConstructionPhase::ALL
            .iter()
            .map(|phase| phase.billing_weight())
            .sum();
        assert!((sum - 1.0).abs() < 1e-12);
        assert_eq!(ConstructionPhase::Survey.billing_weight(), 0.03);
        assert_eq!(ConstructionPhase::Foundation.billing_weight(), 0.17);
        assert_eq!(ConstructionPhase::Structure.billing_weight(), 0.35);
        assert_eq!(ConstructionPhase::Envelope.billing_weight(), 0.20);
        assert_eq!(ConstructionPhase::Utilities.billing_weight(), 0.10);
        assert_eq!(ConstructionPhase::Finish.billing_weight(), 0.12);
        assert_eq!(ConstructionPhase::Inspection.billing_weight(), 0.03);
        assert_eq!(ConstructionPhase::Complete.billing_weight(), 0.0);
    }

    #[test]
    fn survey_installment_is_due_at_placement() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_priced_site(1, 10_000, 100_000.0).expect("site");
        let site = site(&store, 1);
        assert_eq!(site.cost_total, 100_000.0);
        assert_eq!(site.cost_paid, 0.0);
        assert!(!site.payment_stalled);
        assert_close(site.target_paid(), 3_000.0);
        assert_close(site.payment_due(), 3_000.0);
        assert_close(store.payment_due(1), 3_000.0);
        assert_close(store.total_payment_due(), 3_000.0);
        assert_close(store.payment_due(99), 0.0);
    }

    #[test]
    fn payment_due_grows_at_each_phase_boundary() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_priced_site(1, 10_000, 100_000.0).expect("site");
        let mut previous = store.payment_due(1);
        for phase in [
            ConstructionPhase::Foundation,
            ConstructionPhase::Structure,
            ConstructionPhase::Envelope,
            ConstructionPhase::Utilities,
            ConstructionPhase::Finish,
            ConstructionPhase::Inspection,
        ] {
            store.site_mut(1).expect("site").phase = phase;
            let due = store.payment_due(1);
            assert!(due > previous, "{phase:?} due {due} <= {previous}");
            previous = due;
        }
        assert_close(previous, 100_000.0);
        store.site_mut(1).expect("site").phase = ConstructionPhase::Complete;
        assert_close(store.site(1).expect("site").target_paid(), 100_000.0);
        assert_close(store.payment_due(1), 100_000.0);
    }

    #[test]
    fn tick_transition_moves_to_the_next_installment() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_priced_site(1, 10_000, 100_000.0).expect("site");
        assert_close(store.payment_due(1), 3_000.0);
        let report = store.tick(1.0);
        assert_eq!(report.phase_changed, vec![1]);
        assert_eq!(site(&store, 1).phase, ConstructionPhase::Foundation);
        assert_close(store.payment_due(1), 20_000.0);
    }

    #[test]
    fn mark_paid_clears_the_due_and_the_payment_stall() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_priced_site(1, 10_000, 100_000.0).expect("site");
        let site = store.site_mut(1).expect("site");
        site.stall_for_payment();
        assert!(site.payment_stalled);
        assert_eq!(site.stall_reason.as_deref(), Some("payment"));
        let due = site.payment_due();
        site.mark_paid(due).expect("pay");
        assert_close(site.cost_paid, 3_000.0);
        assert_close(site.payment_due(), 0.0);
        assert!(!site.payment_stalled);
        assert!(site.stall_reason.is_none());
        assert!(site.mark_paid(-1.0).is_err());
        assert!(site.mark_paid(f64::NAN).is_err());
        assert!(site.mark_paid(1.0).is_err());
        assert_close(site.cost_paid, 3_000.0);
    }

    #[test]
    fn mark_paid_accepts_a_partial_installment() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_priced_site(1, 10_000, 100_000.0).expect("site");
        let site = store.site_mut(1).expect("site");
        site.mark_paid(1_000.0).expect("pay");
        assert_close(site.cost_paid, 1_000.0);
        assert_close(site.payment_due(), 2_000.0);
    }

    #[test]
    fn payment_stalled_site_does_not_work_or_consume_materials() {
        let mut store = ConstructionStore::new(4, 100_000);
        store.add_priced_site(1, 10_000, 50_000.0).expect("site");
        store.site_mut(1).expect("site").stall_for_payment();
        let report = store.tick(1_000.0);
        assert!(report.completed.is_empty());
        assert!(report.phase_changed.is_empty());
        assert_eq!(report.stalled, 1);
        assert_eq!(store.materials_stock, 100_000);
        let site = site(&store, 1);
        assert_eq!(site.phase, ConstructionPhase::Survey);
        assert_eq!(site.phase_progress, 0.0);
        assert_eq!(site.materials_delivered, 0);
        assert_eq!(site.crew_assigned, 0);
        assert_eq!(site.stall_reason.as_deref(), Some("payment"));
    }

    #[test]
    fn stall_after_a_transition_holds_the_next_phase() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_priced_site(1, 10_000, 100_000.0).expect("site");
        store.tick(1.0);
        assert_eq!(site(&store, 1).phase, ConstructionPhase::Foundation);
        let delivered = site(&store, 1).materials_delivered;
        let stock = store.materials_stock;
        store.site_mut(1).expect("site").stall_for_payment();
        let report = store.tick(1_000.0);
        assert!(report.completed.is_empty());
        assert!(report.phase_changed.is_empty());
        assert_eq!(report.stalled, 1);
        let site = site(&store, 1);
        assert_eq!(site.phase, ConstructionPhase::Foundation);
        assert_eq!(site.materials_delivered, delivered);
        assert_eq!(store.materials_stock, stock);
        assert_eq!(site.stall_reason.as_deref(), Some("payment"));
    }

    #[test]
    fn resume_payment_restores_construction() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_priced_site(1, 10_000, 100_000.0).expect("site");
        store.site_mut(1).expect("site").stall_for_payment();
        store.tick(1_000.0);
        assert_eq!(site(&store, 1).phase, ConstructionPhase::Survey);
        store.site_mut(1).expect("site").resume_payment();
        let resumed = store.site(1).expect("site");
        assert!(!resumed.payment_stalled);
        assert!(resumed.stall_reason.is_none());
        let report = store.tick(1_000.0);
        assert_eq!(report.completed, vec![1]);
        assert!(site(&store, 1).is_complete());
    }

    #[test]
    fn legacy_zero_cost_sites_are_never_charged() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_site(1, 10_000).expect("site");
        let legacy = site(&store, 1);
        assert_eq!(legacy.cost_total, 0.0);
        assert_eq!(legacy.cost_paid, 0.0);
        assert!(!legacy.payment_stalled);
        assert_eq!(legacy.target_paid(), 0.0);
        assert_eq!(legacy.payment_due(), 0.0);
        assert_eq!(store.payment_due(1), 0.0);
        assert_eq!(store.total_payment_due(), 0.0);
        store.site_mut(1).expect("site").stall_for_payment();
        assert!(!site(&store, 1).payment_stalled);
        assert!(store.site_mut(1).expect("site").mark_paid(0.0).is_ok());
        assert!(store.site_mut(1).expect("site").mark_paid(1.0).is_err());
        let report = store.tick(1_000.0);
        assert_eq!(report.completed, vec![1]);
    }

    #[test]
    fn invalid_site_costs_are_rejected() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        for cost in [
            -1.0,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            MAX_SITE_COST + 1.0,
        ] {
            assert!(
                store.add_priced_site(1, 10_000, cost).is_err(),
                "cost {cost} was accepted"
            );
        }
        assert_eq!(store.site_count(), 0);
        store.add_priced_site(1, 10_000, 10_000.0).expect("site");
        store.site_mut(1).expect("site").cost_total = f64::NAN;
        assert!(store.validate().is_err());
        store.site_mut(1).expect("site").cost_total = 10_000.0;
        store.site_mut(1).expect("site").cost_paid = -1.0;
        assert!(store.validate().is_err());
        store.site_mut(1).expect("site").cost_paid = 10_001.0;
        assert!(store.validate().is_err());
    }

    #[test]
    fn priced_site_round_trips_through_json() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_priced_site(1, 10_000, 50_000.0).expect("site");
        store
            .site_mut(1)
            .expect("site")
            .mark_paid(1_500.0)
            .expect("pay");
        let json = store.to_json_pretty().expect("json");
        assert!(json.contains("cost_total"));
        assert!(json.contains("cost_paid"));
        let decoded = ConstructionStore::from_json(&json).expect("decode");
        assert_eq!(decoded, store);
        assert_eq!(decoded.checksum(), store.checksum());
    }

    #[test]
    fn old_json_without_cost_fields_loads_as_legacy() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_priced_site(1, 10_000, 50_000.0).expect("site");
        let mut value: serde_json::Value =
            serde_json::from_str(&store.to_json_pretty().expect("json")).expect("value");
        let site = value
            .get_mut("sites")
            .and_then(|sites| sites.get_mut("1"))
            .and_then(|site| site.as_object_mut())
            .expect("site object");
        site.remove("cost_total");
        site.remove("cost_paid");
        site.remove("payment_stalled");
        let legacy = ConstructionStore::from_json(&value.to_string()).expect("legacy");
        let site = legacy.site(1).expect("site");
        assert_eq!(site.cost_total, 0.0);
        assert_eq!(site.cost_paid, 0.0);
        assert!(!site.payment_stalled);
        assert_eq!(site.payment_due(), 0.0);
        assert_eq!(legacy.total_payment_due(), 0.0);
    }

    #[test]
    fn total_payment_due_is_deterministic_by_placement_order() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_priced_site(2, 10_000, 100_000.0).expect("site");
        store.add_priced_site(1, 10_000, 50_000.0).expect("site");
        let order: Vec<u64> = store.sites.keys().copied().collect();
        assert_eq!(order, vec![1, 2]);
        assert_close(store.payment_due(1), 1_500.0);
        assert_close(store.payment_due(2), 3_000.0);
        assert_close(store.total_payment_due(), 4_500.0);
    }

    #[test]
    fn zero_cost_json_and_checksum_stay_pinned() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_site(1, 64_224).expect("site");
        store.tick(10.0);
        let json = store.to_json_pretty().expect("json");
        assert!(!json.contains("cost_total"));
        assert!(!json.contains("cost_paid"));
        assert!(!json.contains("payment_stalled"));
        assert_eq!(store.checksum(), 0xc29e_763e_7663_87f4);
        let decoded = ConstructionStore::from_json(&json).expect("decode");
        assert_eq!(decoded, store);
        assert_eq!(decoded.checksum(), store.checksum());

        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_site(1, 10_000).expect("site");
        store.tick(1.0);
        assert_eq!(store.checksum(), 0xefbc_5502_6c81_24d7);
    }

    #[test]
    fn campaign_time_scale_validates_and_defaults_to_developer() {
        assert_eq!(
            ConstructionTimeScale::default(),
            ConstructionTimeScale::DeveloperSeconds
        );
        assert!(ConstructionTimeScale::campaign(10.0).is_ok());
        assert!(ConstructionTimeScale::campaign(3600.0).is_ok());
        for bad in [f64::NAN, f64::INFINITY, 0.0, 9.99, 3600.1] {
            assert!(ConstructionTimeScale::campaign(bad).is_err());
        }
        let mut store = ConstructionStore::new(4, 1_000_000);
        assert_eq!(store.time_scale(), ConstructionTimeScale::DeveloperSeconds);
        store
            .set_time_scale(ConstructionTimeScale::campaign_default())
            .expect("scale");
        assert_eq!(store.time_scale().name(), "campaign_days");
        assert_eq!(store.time_scale().seconds_per_game_day(), Some(60.0));
    }

    #[test]
    fn campaign_duration_curve_matches_d029_ranges() {
        let cottage = campaign_total_days_for(10_000);
        assert!((2.0..=4.0).contains(&cottage), "cottage {cottage}");
        let mid = campaign_total_days_for(150_000);
        assert!((15.0..=40.0).contains(&mid), "mid {mid}");
        let high = campaign_total_days_for(2_000_000);
        let structure = campaign_phase_days_for(ConstructionPhase::Structure, 2_000_000);
        let envelope = campaign_phase_days_for(ConstructionPhase::Envelope, 2_000_000);
        assert!((10.0..=30.0).contains(&structure), "structure {structure}");
        assert!((10.0..=30.0).contains(&envelope), "envelope {envelope}");
        assert!(campaign_phase_days_for(ConstructionPhase::Foundation, 8_000_000) <= 30.0);
        assert!(high >= 2.0);
    }

    #[test]
    fn campaign_tick_is_slower_than_developer_and_deterministic() {
        let mut developer = ConstructionStore::new(4, 10_000_000);
        developer.add_site(1, 10_000).expect("site");
        let mut campaign = ConstructionStore::new(4, 10_000_000);
        campaign.add_site(1, 10_000).expect("site");
        campaign
            .set_time_scale(ConstructionTimeScale::campaign_default())
            .expect("scale");
        assert_eq!(developer.tick(600.0).completed.len(), 1);
        let first = campaign.tick(600.0);
        assert!(first.completed.is_empty());
        let mut clone = campaign.clone();
        let second = campaign.tick(600.0);
        assert_eq!(first.phase_changed, second.phase_changed);
        assert_eq!(campaign.checksum(), {
            let again = clone.tick(600.0);
            assert_eq!(second.phase_changed, again.phase_changed);
            clone.checksum()
        });
    }

    #[test]
    fn campaign_scale_round_trips_and_old_save_defaults() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store
            .set_time_scale(ConstructionTimeScale::campaign(60.0).expect("scale"))
            .expect("set");
        let json = store.to_json_pretty().expect("json");
        let decoded = ConstructionStore::from_json(&json).expect("decode");
        assert_eq!(decoded.time_scale(), store.time_scale());
        let mut value: serde_json::Value = serde_json::from_str(&json).expect("value");
        value.as_object_mut().expect("object").remove("time_scale");
        let old = ConstructionStore::from_json(&value.to_string()).expect("old decode");
        assert_eq!(old.time_scale(), ConstructionTimeScale::DeveloperSeconds);
    }

    #[test]
    fn non_finite_tick_is_a_noop_and_keeps_the_checksum() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_site(1, 10_000).unwrap();
        let before = store.checksum();
        for delta in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            store.tick(delta);
            assert_eq!(store.checksum(), before, "delta {delta}");
            assert!(store.validate().is_ok());
            assert!(store.to_json_pretty().is_ok());
        }
    }

    #[test]
    fn checksum_changes_with_content_and_is_never_empty_bytes() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_site(1, 10_000).unwrap();
        let empty_hash = 0xcbf2_9ce4_8422_2325_u64;
        assert_ne!(store.checksum(), empty_hash);
        let before = store.checksum();
        store.tick(10.0);
        assert_ne!(store.checksum(), before);
    }

    #[test]
    fn campaign_phases_respect_the_phase_cap_for_large_sites() {
        let mut store = ConstructionStore::new(64, 100_000_000);
        store
            .set_time_scale(ConstructionTimeScale::campaign_default())
            .unwrap();
        store.add_site(1, 4_000_000).unwrap();
        let phase = store.site(1).unwrap().phase;
        let duration = store.time_scale().phase_seconds_for(phase, 4_000_000);
        assert!(duration <= (CAMPAIGN_MAX_PHASE_DAYS * DAY_SECONDS) as f32 + 1.0);
        let total = store.time_scale().total_seconds_for(4_000_000);
        let mut remaining = f64::from(total * 0.1 + duration) + 1.0;
        while remaining > 0.0 {
            store.tick(remaining.min(3_600.0));
            remaining -= 3_600.0;
        }
        assert_ne!(store.site(1).unwrap().phase, phase);
    }
}
