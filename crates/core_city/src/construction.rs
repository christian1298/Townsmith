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

use crate::trades::{Trade, TradeError, TradePool};

/// Current construction save format.
pub const CONSTRUCTION_FORMAT_VERSION: u16 = 1;

/// Maximum simulated step accepted by [`ConstructionStore::tick`].
pub const MAX_TICK_SECONDS: f64 = 3600.0;

/// Maximum construction sites in one save, a guard against malformed input.
pub const MAX_SITES: usize = 100_000;

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

    /// Add a site for a placement. Repeating an id is rejected.
    pub fn add_site(
        &mut self,
        placement_id: u64,
        solid_voxels: u64,
    ) -> Result<&ConstructionSite, ConstructionError> {
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
        self.sites.insert(
            placement_id,
            ConstructionSite::new(placement_id, solid_voxels),
        );
        Ok(self.sites.get(&placement_id).expect("just inserted"))
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
        let dt = delta_seconds.clamp(0.0, MAX_TICK_SECONDS) as f32;
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

        for site in self.sites.values_mut() {
            if site.is_complete() {
                site.crew_assigned = 0;
                site.stall_reason = None;
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

            let rate = (site.materials_required as f64 / f64::from(site.total_s.max(1.0))).max(1.0);
            let wanted = (rate * f64::from(dt)).ceil() as u64;
            let delivered = wanted.min(remaining_materials).min(
                site.materials_required
                    .saturating_sub(site.materials_delivered),
            );
            site.materials_delivered += delivered;
            remaining_materials -= delivered;
            site.elapsed_s = (site.elapsed_s + dt).min(site.total_s * 4.0);

            if site.materials_delivered < site.phase_material_target() {
                remaining_crew += site_crew;
                site.crew_assigned = 0;
                site.stall_reason = Some("materials".to_string());
                continue;
            }

            let mut left = dt;
            while left > 0.0 && !site.is_complete() {
                let duration = site.total_s * site.phase.weight();
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
            if !site.phase_progress.is_finite() || !(0.0..=1.0).contains(&site.phase_progress) {
                return Err(ConstructionError::InvalidSite("phase progress is invalid"));
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
        let bytes = serde_json::to_vec(self).unwrap_or_default();
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
}
