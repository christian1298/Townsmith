//! Deterministic named-citizen identities.
//!
//! The layer is derived from the aggregate household runtime: every resident
//! slot gets a stable 63-bit id and a generated name from the world seed,
//! placement id, slot, and identity version. N4 adds an optional save object so
//! a committed identity survives a roundtrip; absent saves still rebuild the
//! pool from the households. N4 also persists a whole-day age per identity and
//! provides a deterministic, day-based lifecycle tick that ages citizens and
//! retires them at 100 years.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::population::Household;
use crate::traffic::RoadGraph;

/// Identity function version. Changing this intentionally renames everyone.
pub const IDENTITY_VERSION: u64 = 1;

/// Current named-citizen save format.
pub const NAMED_FORMAT_VERSION: u16 = 1;

/// Maximum identities kept for one city.
pub const MAX_NAMED_CITIZENS: usize = 4_096;

/// Maximum display-name length in bytes.
pub const MAX_NAMED_NAME_BYTES: usize = 64;

/// Maximum persisted age in whole days (100 years).
///
/// [`NamedCitizens::tick_lifecycle`] removes a citizen when the advanced age
/// reaches this value. Validation accepts ages up to and including this cap so
/// a save written at the boundary still loads; that record dies on the next
/// positive lifecycle tick.
pub const MAX_AGE_DAYS: u32 = 36_500;

/// Hard cap for the city-wide active materialization set.
pub const MAX_ACTIVE_CITIZENS: usize = 2_000;

/// Hard cap for the per-client detailed materialization set.
pub const MAX_DETAILED_CITIZENS: usize = 200;

/// Maximum materialization changes applied in one server tick.
///
/// One added, removed, or updated citizen counts as one change. The budget is
/// enforced by [`diff_selection`]; unchanged retained citizens do not count.
pub const MAX_MATERIALIZATION_CHANGES: usize = 64;

const FIRST_NAMES: [&str; 32] = [
    "Ada", "Bram", "Corin", "Dara", "Ewan", "Faye", "Geth", "Hana", "Ivo", "Jori", "Kael", "Lena",
    "Mira", "Nilo", "Orla", "Pia", "Quin", "Rhea", "Sol", "Tamsin", "Ulric", "Vera", "Wren", "Xan",
    "Yara", "Zeno", "Alba", "Bren", "Cass", "Dain", "Elin", "Fen",
];

const LAST_NAMES: [&str; 32] = [
    "Ash", "Birch", "Brook", "Clay", "Dale", "Ember", "Field", "Glen", "Heath", "Holt", "Ivy",
    "Juniper", "Kirk", "Lark", "Moss", "North", "Oak", "Pike", "Reed", "Ridge", "Rowen", "Sage",
    "Shore", "Stone", "Thorne", "Vale", "West", "Willow", "Wren", "Yale", "Fern", "Marsh",
];

/// SplitMix64 finalizer.
#[must_use]
pub fn mix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// Stable 63-bit identity for one resident slot.
#[must_use]
pub fn citizen_id(seed: u64, placement_id: u64, slot: u32) -> u64 {
    let mut value = mix64(seed ^ 0x6a09_e667_f3bc_c909);
    value ^= mix64(placement_id.wrapping_add(0xbb67_ae85_84ca_a73b));
    value ^= mix64(u64::from(slot).wrapping_add(0x3c6e_f372_fe94_f82b));
    value ^= mix64(IDENTITY_VERSION.wrapping_add(0xa54f_f53a_5f1d_36f1));
    (mix64(value) & 0x7fff_ffff_ffff_ffff).max(1)
}

/// Generated display name for an identity.
#[must_use]
pub fn name_for(id: u64) -> String {
    let first = FIRST_NAMES[(mix64(id ^ 0x243f_6a88_85a3_08d3) as usize) % FIRST_NAMES.len()];
    let last = LAST_NAMES[(mix64(id ^ 0x1319_8a2e_0370_7344) as usize) % LAST_NAMES.len()];
    format!("{first} {last}")
}

/// Derived display age in whole years for an identity.
///
/// Ages are deterministic in the 63-bit identity id and span `18..=64`. This
/// is the initial age band, not the persisted state: [`NamedCitizens::sync`]
/// seeds [`NamedCitizen::age_days`] from this function when the persisted age
/// is zero. The result is stable for a given id, and changing this function
/// changes the starting age of every identity that has not aged yet.
#[must_use]
pub fn age_years(id: u64) -> u8 {
    18 + (mix64(id ^ 0x4528_21e6_38d0_1377) % 47) as u8
}

/// Initial persisted age for an identity, in whole days.
fn initial_age_days(id: u64) -> u32 {
    u32::from(age_years(id)) * 365
}

/// Whether a resident slot is employed according to the household aggregate.
///
/// The household stores a worker count, not per-slot jobs, so employment is
/// derived in ascending slot order: slots `0..household.workers` are employed
/// and the remaining resident slots are not. This is a placeholder until
/// per-slot jobs exist; it is read-only and never mutates or saves the
/// household.
#[must_use]
pub fn slot_is_employed(household: &Household, slot: u32) -> bool {
    slot < household.workers
}

/// One named resident.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedCitizen {
    /// Stable 63-bit identity.
    pub id: u64,
    /// Home placement id.
    pub placement_id: u64,
    /// Resident slot inside the aggregate household.
    pub slot: u32,
    /// Generated display name.
    pub name: String,
    /// Persisted age in whole days.
    ///
    /// `0` means "not initialized": old saves without this field load as zero
    /// and [`NamedCitizens::sync`] replaces it with the deterministic starting
    /// age. v1 has no births, so a legitimate zero-day-old named citizen does
    /// not exist yet.
    #[serde(default)]
    pub age_days: u32,
}

/// Caps for one materialization selection pass.
///
/// The defaults match the accepted N3 budget: 2,000 city-wide active citizens
/// and 200 detailed citizens per client. Both caps may be lowered, but
/// [`MaterializationBudget::validate`] rejects values above the hard caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterializationBudget {
    /// Maximum citizens in the active set.
    pub active_max: usize,
    /// Maximum citizens in the detailed set.
    pub detailed_max: usize,
}

impl Default for MaterializationBudget {
    fn default() -> Self {
        Self {
            active_max: MAX_ACTIVE_CITIZENS,
            detailed_max: MAX_DETAILED_CITIZENS,
        }
    }
}

impl MaterializationBudget {
    /// Validate both caps against each other and the documented hard caps.
    ///
    /// # Errors
    ///
    /// Returns [`NamedError`] when the active cap is zero or above
    /// [`MAX_ACTIVE_CITIZENS`], the detailed cap is above
    /// [`MAX_DETAILED_CITIZENS`], or the detailed cap exceeds the active cap.
    pub fn validate(&self) -> Result<(), NamedError> {
        if self.active_max == 0 || self.active_max > MAX_ACTIVE_CITIZENS {
            return Err(NamedError::Invalid("active materialization budget"));
        }
        if self.detailed_max > MAX_DETAILED_CITIZENS {
            return Err(NamedError::Invalid("detailed materialization budget"));
        }
        if self.detailed_max > self.active_max {
            return Err(NamedError::Invalid("detailed budget exceeds active budget"));
        }
        Ok(())
    }
}

/// One named citizen selected for materialization.
///
/// This is a presentation view. It is derived on each selection pass, is not
/// saved, and carries no route or motion state.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterializedCitizen {
    /// Stable 63-bit identity.
    pub id: u64,
    /// Home placement id.
    pub placement_id: u64,
    /// Resident slot inside the aggregate household.
    pub slot: u32,
    /// Generated display name.
    pub name: String,
    /// Home position in metres.
    pub home: [f32; 2],
    /// Straight-line distance from the selection centre to the home, in metres.
    pub distance: f32,
}

/// Result of one deterministic materialization selection pass.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MaterializationSelection {
    /// Nearest citizens inside the active radius, capped by the active budget.
    pub active: Vec<MaterializedCitizen>,
    /// Nearest citizens inside the detailed radius, capped by the detailed
    /// budget. Every detailed citizen is also present in `active`.
    pub detailed: Vec<MaterializedCitizen>,
}

/// Deterministic delta between two active materialization selections.
///
/// The diff compares [`MaterializationSelection::active`] only. It carries no
/// route, motion, or fade state, and applying it is the caller's job. `added`,
/// `updated`, and `removed` are ordered by id ascending.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MaterializationDiff {
    /// Active citizens present in `next` but not in `previous`.
    pub added: Vec<MaterializedCitizen>,
    /// Ids present in `previous` but not in `next`, ascending.
    pub removed: Vec<u64>,
    /// Citizens present in both whose identity or home changed.
    ///
    /// A change in [`MaterializedCitizen::distance`] alone is not an update:
    /// distance is derived from the camera and refreshes every selection pass.
    pub updated: Vec<MaterializedCitizen>,
    /// Number of ids present in both `previous` and `next`.
    pub retained: usize,
    /// Number of candidate changes before applying `max_changes`.
    pub changed: usize,
    /// Number of changes kept in this diff, at most `max_changes`.
    pub applied: usize,
    /// Whether some candidate changes were deferred to a later tick.
    pub truncated: bool,
}

impl MaterializationDiff {
    /// Whether this diff contains no changes to apply.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.applied == 0
    }
}

/// Diff two active selections under a per-tick change budget.
///
/// The rules are:
///
/// - `removed`: ids in `previous.active` that are absent from `next.active`;
/// - `added`: ids in `next.active` that were absent from `previous.active`;
/// - `retained`: ids present in both, whether or not their data changed;
/// - `updated`: retained ids whose [`MaterializedCitizen`] data changed, except
///   for [`distance`](MaterializedCitizen::distance), which is derived from the
///   camera and refreshes without a materialization change.
///
/// `previous` is optional; `None` is an empty previous selection. Duplicate ids
/// inside one selection are deduplicated deterministically (the last record in
/// the vector wins), so inputs from older or custom callers are tolerated.
///
/// # Budget
///
/// One added, removed, or updated citizen is one change. If the candidate count
/// is at most `max_changes`, every change is returned and `truncated` is
/// `false`. Otherwise the nearest changes are kept: candidates are ordered by
/// `(distance, id)` using the distance from `next` for added/updated citizens
/// and from `previous` for removed citizens. This is a deterministic tie-break
/// by id, not the full N3 priority list: pinned/story selection and
/// anti-starvation are deliberately not assumed here.
///
/// Truncation is not a scheduler: it does not enforce
/// [`MaterializationBudget::active_max`], balance additions against removals,
/// or track dwell time. A caller applying the diff must keep its own active
/// cap and defer the remainder of [`changed`](Self::changed) to later ticks.
///
/// This function is pure: it allocates the diff and does not mutate either
/// selection or the named pool.
#[must_use]
pub fn diff_selection(
    previous: Option<&MaterializationSelection>,
    next: &MaterializationSelection,
    max_changes: usize,
) -> MaterializationDiff {
    let previous_by_id: BTreeMap<u64, &MaterializedCitizen> = previous
        .map(|selection| {
            selection
                .active
                .iter()
                .map(|citizen| (citizen.id, citizen))
                .collect()
        })
        .unwrap_or_default();
    let next_by_id: BTreeMap<u64, &MaterializedCitizen> = next
        .active
        .iter()
        .map(|citizen| (citizen.id, citizen))
        .collect();

    let removed: Vec<&MaterializedCitizen> = previous_by_id
        .iter()
        .filter(|(id, _)| !next_by_id.contains_key(id))
        .map(|(_, citizen)| *citizen)
        .collect();
    let added: Vec<&MaterializedCitizen> = next_by_id
        .iter()
        .filter(|(id, _)| !previous_by_id.contains_key(id))
        .map(|(_, citizen)| *citizen)
        .collect();
    let updated: Vec<&MaterializedCitizen> = next_by_id
        .iter()
        .filter_map(|(id, citizen)| {
            previous_by_id
                .get(id)
                .filter(|previous| !same_materialization(previous, citizen))
                .map(|_| *citizen)
        })
        .collect();
    let retained = next_by_id
        .keys()
        .filter(|id| previous_by_id.contains_key(id))
        .count();
    let changed = removed.len() + added.len() + updated.len();

    if changed <= max_changes {
        return MaterializationDiff {
            added: added.into_iter().cloned().collect(),
            removed: removed.iter().map(|citizen| citizen.id).collect(),
            updated: updated.into_iter().cloned().collect(),
            retained,
            changed,
            applied: changed,
            truncated: false,
        };
    }

    let mut candidates: Vec<(f32, u64)> = Vec::with_capacity(changed);
    candidates.extend(removed.iter().map(|citizen| (citizen.distance, citizen.id)));
    candidates.extend(added.iter().map(|citizen| (citizen.distance, citizen.id)));
    candidates.extend(updated.iter().map(|citizen| (citizen.distance, citizen.id)));
    candidates.sort_by(|left, right| {
        left.0
            .total_cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
    });
    let kept: BTreeSet<u64> = candidates
        .into_iter()
        .take(max_changes)
        .map(|(_, id)| id)
        .collect();

    MaterializationDiff {
        added: added
            .iter()
            .filter(|citizen| kept.contains(&citizen.id))
            .map(|citizen| (*citizen).clone())
            .collect(),
        removed: removed
            .iter()
            .filter(|citizen| kept.contains(&citizen.id))
            .map(|citizen| citizen.id)
            .collect(),
        updated: updated
            .iter()
            .filter(|citizen| kept.contains(&citizen.id))
            .map(|citizen| (*citizen).clone())
            .collect(),
        retained,
        changed,
        applied: kept.len(),
        truncated: true,
    }
}

/// Whether two records describe the same materialization.
///
/// [`MaterializedCitizen::distance`] is excluded on purpose: moving the camera
/// recomputes every distance and must not consume the change budget.
fn same_materialization(left: &MaterializedCitizen, right: &MaterializedCitizen) -> bool {
    left.id == right.id
        && left.placement_id == right.placement_id
        && left.slot == right.slot
        && left.name == right.name
        && left.home[0].to_bits() == right.home[0].to_bits()
        && left.home[1].to_bits() == right.home[1].to_bits()
}

/// Result of one named-citizen lifecycle tick.
///
/// See [`NamedCitizens::tick_lifecycle`] for the exact counting rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LifecycleTick {
    /// Citizens that received the age advance, including those that died in
    /// this same tick.
    pub aged: usize,
    /// Citizens removed because their age reached [`MAX_AGE_DAYS`].
    pub died: usize,
}

/// Derived named-citizen pool for one city.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedCitizens {
    /// World seed.
    pub seed: u64,
    /// Maximum identities kept.
    pub max: usize,
    /// Identities keyed by `(placement_id, slot)`.
    pub citizens: BTreeMap<(u64, u32), NamedCitizen>,
}

/// Persisted named-citizen pool.
///
/// The world save stores this object beside `population`. Records are sorted
/// by id, then home and slot, so the JSON is deterministic. Each record carries
/// its persisted `age_days`; older records without that field load as zero and
/// [`NamedCitizens::sync`] seeds them from the identity. Absent objects are
/// valid old saves; an unknown [`format_version`](Self::format_version) is
/// rejected as corrupt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedSave {
    /// Save format version; must equal [`NAMED_FORMAT_VERSION`].
    pub format_version: u16,
    /// World seed the identities were generated for.
    pub seed: u64,
    /// Maximum identities the save may contain.
    pub max: u32,
    /// Named identities, sorted by id.
    #[serde(default)]
    pub citizens: Vec<NamedCitizen>,
}

impl NamedSave {
    /// Validate version, bounds, uniqueness, and names.
    pub fn validate(&self) -> Result<(), NamedError> {
        if self.format_version != NAMED_FORMAT_VERSION {
            return Err(NamedError::Invalid("format_version"));
        }
        let max = usize::try_from(self.max).map_err(|_| NamedError::Invalid("max identities"))?;
        if max == 0 || max > MAX_NAMED_CITIZENS {
            return Err(NamedError::Invalid("max identities"));
        }
        if self.citizens.len() > max {
            return Err(NamedError::Invalid("too many identities"));
        }
        let mut ids = BTreeSet::new();
        let mut slots = BTreeSet::new();
        for citizen in &self.citizens {
            if citizen.id == 0 || citizen.id > i64::MAX as u64 {
                return Err(NamedError::Invalid("identity id"));
            }
            if !ids.insert(citizen.id) {
                return Err(NamedError::Invalid("duplicate identity id"));
            }
            if !slots.insert((citizen.placement_id, citizen.slot)) {
                return Err(NamedError::Invalid("duplicate identity slot"));
            }
            if citizen.age_days > MAX_AGE_DAYS {
                return Err(NamedError::Invalid("identity age"));
            }
            validate_name(&citizen.name)?;
        }
        Ok(())
    }

    /// Serialize and validate this save object.
    ///
    /// # Errors
    ///
    /// Returns [`NamedError`] if the object is invalid or cannot serialize.
    pub fn to_json_pretty(&self) -> Result<String, NamedError> {
        self.validate()?;
        serde_json::to_string_pretty(self).map_err(NamedError::from)
    }

    /// Parse and validate a save object.
    ///
    /// # Errors
    ///
    /// Returns [`NamedError`] for malformed JSON or invalid contents.
    pub fn from_json(json: &str) -> Result<Self, NamedError> {
        let save: Self = serde_json::from_str(json).map_err(NamedError::from)?;
        save.validate()?;
        Ok(save)
    }
}

impl NamedCitizens {
    /// Create an empty pool.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            max: MAX_NAMED_CITIZENS,
            citizens: BTreeMap::new(),
        }
    }

    /// Rebuild the pool from the current households.
    ///
    /// Identities are added for every resident slot, removed when a household
    /// shrinks or disappears, and capped at `max` in household-id order.
    pub fn sync(&mut self, households: &BTreeMap<u64, Household>) {
        let mut wanted: BTreeSet<(u64, u32)> = BTreeSet::new();
        for (&placement_id, household) in households {
            if wanted.len() >= self.max {
                break;
            }
            for slot in 0..household.residents {
                if wanted.len() >= self.max {
                    break;
                }
                wanted.insert((placement_id, slot));
            }
        }
        self.citizens.retain(|key, _| wanted.contains(key));
        for key in wanted {
            let citizen = self.citizens.entry(key).or_insert_with(|| {
                let (placement_id, slot) = key;
                let id = citizen_id(self.seed, placement_id, slot);
                NamedCitizen {
                    id,
                    placement_id,
                    slot,
                    name: name_for(id),
                    age_days: initial_age_days(id),
                }
            });
            if citizen.age_days == 0 {
                citizen.age_days = initial_age_days(citizen.id);
            }
        }
    }

    /// Advance every named citizen by whole simulated days and remove the dead.
    ///
    /// The tick is deterministic and uses integer arithmetic only:
    ///
    /// - `days` is truncated toward zero to whole days. A non-positive or
    ///   non-finite value is a no-op, and a fractional part is discarded rather
    ///   than accumulated. Callers should pass whole simulation days, because
    ///   the lifecycle is day-based.
    /// - A persisted age of zero is treated as uninitialized and seeded from
    ///   the identity (the same rule as [`Self::sync`]), so ticking a pool
    ///   loaded from an old save is safe even before a sync.
    /// - Every citizen then receives the day advance, saturating at
    ///   [`MAX_AGE_DAYS`].
    /// - Citizens whose advanced age is at least [`MAX_AGE_DAYS`] (100 years)
    ///   are removed in `(placement_id, slot)` key order.
    ///
    /// `aged` counts citizens that received the advance, including citizens
    /// removed in the same tick; `died` counts the removals. The named layer
    /// does not change household `residents`, `workers`, or needs: applying
    /// population consequences is a caller concern.
    pub fn tick_lifecycle(&mut self, days: f64) -> LifecycleTick {
        if !days.is_finite() || days <= 0.0 {
            return LifecycleTick::default();
        }
        // Float-to-int casts saturate, so an absurd `days` becomes `u32::MAX`.
        let whole_days = days.floor() as u32;
        if whole_days == 0 {
            return LifecycleTick::default();
        }

        let mut tick = LifecycleTick::default();
        let mut dead: Vec<(u64, u32)> = Vec::new();
        for (key, citizen) in &mut self.citizens {
            if citizen.age_days == 0 {
                citizen.age_days = initial_age_days(citizen.id);
            }
            citizen.age_days = citizen
                .age_days
                .saturating_add(whole_days)
                .min(MAX_AGE_DAYS);
            tick.aged += 1;
            if citizen.age_days >= MAX_AGE_DAYS {
                dead.push(*key);
            }
        }
        for key in dead {
            if self.citizens.remove(&key).is_some() {
                tick.died += 1;
            }
        }
        tick
    }

    /// Oldest persisted age in whole days, or `None` for an empty pool.
    #[must_use]
    pub fn oldest_age_days(&self) -> Option<u32> {
        self.citizens.values().map(|citizen| citizen.age_days).max()
    }

    /// Mean persisted age in whole days, truncated; zero for an empty pool.
    #[must_use]
    pub fn average_age_days(&self) -> u32 {
        let count = u64::try_from(self.citizens.len()).unwrap_or(u64::MAX);
        if count == 0 {
            return 0;
        }
        let total: u64 = self
            .citizens
            .values()
            .map(|citizen| u64::from(citizen.age_days))
            .sum();
        u32::try_from(total / count).unwrap_or(MAX_AGE_DAYS)
    }

    /// Remove every identity.
    pub fn clear(&mut self) {
        self.citizens.clear();
    }

    /// Number of named residents.
    #[must_use]
    pub fn len(&self) -> usize {
        self.citizens.len()
    }

    /// Whether the pool is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.citizens.is_empty()
    }

    /// Residents of one home, sorted by slot.
    #[must_use]
    pub fn citizens_of(&self, placement_id: u64) -> Vec<&NamedCitizen> {
        self.citizens
            .range((placement_id, 0)..=(placement_id, u32::MAX))
            .map(|(_, citizen)| citizen)
            .collect()
    }

    #[cfg(test)]
    fn get_mut_for_test(&mut self, id: u64) -> &mut NamedCitizen {
        self.citizens
            .iter_mut()
            .find(|(_, citizen)| citizen.id == id)
            .map(|(_, citizen)| citizen)
            .expect("citizen")
    }

    /// One resident by home and slot.
    #[must_use]
    pub fn get(&self, placement_id: u64, slot: u32) -> Option<&NamedCitizen> {
        self.citizens.get(&(placement_id, slot))
    }

    /// Select named citizens for materialization around a camera centre.
    ///
    /// Candidates are the named homes within `active_radius_m` of `center`,
    /// using straight-line metres only: this method never builds, caches, or
    /// allocates a route. Results are ordered by `(distance, id)`, and each
    /// identity appears at most once. `active` is capped by
    /// [`MaterializationBudget::active_max`]; `detailed` is the prefix of
    /// `active` within `detailed_radius_m`, capped by
    /// [`MaterializationBudget::detailed_max`]. A detailed radius larger than
    /// the active radius does not widen the candidate set.
    ///
    /// Identities without a finite placement centre are skipped.
    ///
    /// # Errors
    ///
    /// Returns [`NamedError`] if the budget is invalid, the centre is not
    /// finite, or a radius is negative or not finite.
    pub fn select_near(
        &self,
        center: [f32; 2],
        placement_centers: &BTreeMap<u64, [f32; 2]>,
        budget: MaterializationBudget,
        active_radius_m: f32,
        detailed_radius_m: f32,
    ) -> Result<MaterializationSelection, NamedError> {
        budget.validate()?;
        if !center.iter().all(|value| value.is_finite()) {
            return Err(NamedError::Invalid("materialization center"));
        }
        if !active_radius_m.is_finite() || active_radius_m < 0.0 {
            return Err(NamedError::Invalid("active materialization radius"));
        }
        if !detailed_radius_m.is_finite() || detailed_radius_m < 0.0 {
            return Err(NamedError::Invalid("detailed materialization radius"));
        }

        let mut candidates = Vec::new();
        for citizen in self.citizens.values() {
            let Some(&home) = placement_centers.get(&citizen.placement_id) else {
                continue;
            };
            if !home.iter().all(|value| value.is_finite()) {
                continue;
            }
            let distance = RoadGraph::straight_line_m(center, home);
            if distance > active_radius_m {
                continue;
            }
            candidates.push(MaterializedCitizen {
                id: citizen.id,
                placement_id: citizen.placement_id,
                slot: citizen.slot,
                name: citizen.name.clone(),
                home,
                distance,
            });
        }
        candidates.sort_by(|left, right| {
            left.distance
                .total_cmp(&right.distance)
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut seen = BTreeSet::new();
        candidates.retain(|citizen| seen.insert(citizen.id));

        let active: Vec<MaterializedCitizen> =
            candidates.into_iter().take(budget.active_max).collect();
        let detailed = active
            .iter()
            .filter(|citizen| citizen.distance <= detailed_radius_m)
            .take(budget.detailed_max)
            .cloned()
            .collect();
        Ok(MaterializationSelection { active, detailed })
    }

    /// Select named citizens with radius hysteresis and an optional pin.
    ///
    /// This is the stateful variant of [`Self::select_near`]. A citizen in
    /// `previous` is kept while its current distance is at most
    /// `outer_radius_m`, and any citizen within `inner_radius_m` is added even
    /// when it was not selected before. A `pinned` identity is always kept when
    /// it exists in the pool with a finite home centre, even outside the outer
    /// radius.
    ///
    /// Candidates are sorted by `(distance, id)` and deduplicated by id. The
    /// active set is capped by [`MaterializationBudget::active_max`]; a pinned
    /// citizen outside the cap replaces the furthest non-pinned citizen.
    /// `detailed` is the prefix of `active` within `detailed_radius_m`, capped
    /// by [`MaterializationBudget::detailed_max`].
    ///
    /// `previous` is used by identity only. Names, homes, and distances are
    /// recomputed from the current pool and `placement_centers`, so removed or
    /// stale citizens never reappear.
    ///
    /// # Errors
    ///
    /// Returns [`NamedError`] if the budget is invalid, the centre is not
    /// finite, a radius is negative or not finite, or `inner_radius_m >
    /// outer_radius_m`.
    #[allow(clippy::too_many_arguments)]
    pub fn select_near_with_previous(
        &self,
        previous: Option<&MaterializationSelection>,
        center: [f32; 2],
        placement_centers: &BTreeMap<u64, [f32; 2]>,
        budget: MaterializationBudget,
        inner_radius_m: f32,
        outer_radius_m: f32,
        detailed_radius_m: f32,
        pinned: Option<u64>,
    ) -> Result<MaterializationSelection, NamedError> {
        budget.validate()?;
        if !center.iter().all(|value| value.is_finite()) {
            return Err(NamedError::Invalid("materialization center"));
        }
        if !inner_radius_m.is_finite() || inner_radius_m < 0.0 {
            return Err(NamedError::Invalid("inner materialization radius"));
        }
        if !outer_radius_m.is_finite() || outer_radius_m < 0.0 {
            return Err(NamedError::Invalid("outer materialization radius"));
        }
        if inner_radius_m > outer_radius_m {
            return Err(NamedError::Invalid(
                "inner materialization radius exceeds outer",
            ));
        }
        if !detailed_radius_m.is_finite() || detailed_radius_m < 0.0 {
            return Err(NamedError::Invalid("detailed materialization radius"));
        }

        let mut retained = BTreeSet::new();
        if let Some(previous) = previous {
            retained.extend(previous.active.iter().map(|citizen| citizen.id));
            retained.extend(previous.detailed.iter().map(|citizen| citizen.id));
        }

        let mut candidates = Vec::new();
        for citizen in self.citizens.values() {
            let Some(&home) = placement_centers.get(&citizen.placement_id) else {
                continue;
            };
            if !home.iter().all(|value| value.is_finite()) {
                continue;
            }
            let distance = RoadGraph::straight_line_m(center, home);
            let keep = distance <= inner_radius_m
                || (retained.contains(&citizen.id) && distance <= outer_radius_m)
                || pinned == Some(citizen.id);
            if !keep {
                continue;
            }
            candidates.push(MaterializedCitizen {
                id: citizen.id,
                placement_id: citizen.placement_id,
                slot: citizen.slot,
                name: citizen.name.clone(),
                home,
                distance,
            });
        }
        candidates.sort_by(|left, right| {
            left.distance
                .total_cmp(&right.distance)
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut seen = BTreeSet::new();
        candidates.retain(|citizen| seen.insert(citizen.id));

        let pinned_citizen =
            pinned.and_then(|id| candidates.iter().find(|citizen| citizen.id == id).cloned());

        let mut active: Vec<MaterializedCitizen> =
            candidates.into_iter().take(budget.active_max).collect();
        if let Some(pinned_citizen) = pinned_citizen {
            if !active.iter().any(|citizen| citizen.id == pinned_citizen.id) {
                if active.len() >= budget.active_max {
                    active.pop();
                }
                active.push(pinned_citizen);
                active.sort_by(|left, right| {
                    left.distance
                        .total_cmp(&right.distance)
                        .then_with(|| left.id.cmp(&right.id))
                });
            }
        }

        let detailed = active
            .iter()
            .filter(|citizen| citizen.distance <= detailed_radius_m)
            .take(budget.detailed_max)
            .cloned()
            .collect();
        Ok(MaterializationSelection { active, detailed })
    }

    /// Build the deterministic save object.
    #[must_use]
    pub fn to_save(&self) -> NamedSave {
        let mut citizens: Vec<NamedCitizen> = self.citizens.values().cloned().collect();
        citizens.sort_by_key(|citizen| (citizen.id, citizen.placement_id, citizen.slot));
        NamedSave {
            format_version: NAMED_FORMAT_VERSION,
            seed: self.seed,
            max: u32::try_from(self.max).unwrap_or(u32::MAX),
            citizens,
        }
    }

    /// Rebuild a pool from a validated save object.
    ///
    /// Loaded identities are preserved as-is; the caller syncs the pool with
    /// the loaded households to drop orphan slots.
    ///
    /// # Errors
    ///
    /// Returns [`NamedError`] if the save is invalid.
    pub fn from_save(save: NamedSave) -> Result<Self, NamedError> {
        save.validate()?;
        let max = usize::try_from(save.max).map_err(|_| NamedError::Invalid("max identities"))?;
        let mut citizens = BTreeMap::new();
        for citizen in save.citizens {
            citizens.insert((citizen.placement_id, citizen.slot), citizen);
        }
        let pool = Self {
            seed: save.seed,
            max,
            citizens,
        };
        pool.validate()?;
        Ok(pool)
    }

    /// Serialize the pool as a validated JSON object.
    ///
    /// # Errors
    ///
    /// Returns [`NamedError`] if the pool is invalid or cannot serialize.
    pub fn to_json_pretty(&self) -> Result<String, NamedError> {
        self.to_save().to_json_pretty()
    }

    /// Parse a JSON object and rebuild the pool.
    ///
    /// # Errors
    ///
    /// Returns [`NamedError`] for malformed JSON or invalid contents.
    pub fn from_json(json: &str) -> Result<Self, NamedError> {
        Self::from_save(NamedSave::from_json(json)?)
    }

    /// Stable FNV-1a checksum over identities, names, and persisted ages.
    ///
    /// This checksum is separate from the city and population checksums, so a
    /// lifecycle change updates only the named checksum.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for ((placement_id, slot), citizen) in &self.citizens {
            for byte in placement_id
                .to_le_bytes()
                .iter()
                .chain(slot.to_le_bytes().iter())
                .chain(citizen.id.to_le_bytes().iter())
                .chain(citizen.name.as_bytes())
                .chain(citizen.age_days.to_le_bytes().iter())
            {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        hash
    }

    /// Validate the pool bounds.
    pub fn validate(&self) -> Result<(), NamedError> {
        if self.max == 0 || self.max > MAX_NAMED_CITIZENS {
            return Err(NamedError::Invalid("max identities"));
        }
        if self.citizens.len() > self.max {
            return Err(NamedError::Invalid("too many identities"));
        }
        let mut ids = BTreeSet::new();
        for ((placement_id, slot), citizen) in &self.citizens {
            if citizen.placement_id != *placement_id || citizen.slot != *slot {
                return Err(NamedError::Invalid("identity key mismatch"));
            }
            if citizen.id == 0 || citizen.id > i64::MAX as u64 {
                return Err(NamedError::Invalid("identity id"));
            }
            if !ids.insert(citizen.id) {
                return Err(NamedError::Invalid("duplicate identity id"));
            }
            if citizen.age_days > MAX_AGE_DAYS {
                return Err(NamedError::Invalid("identity age"));
            }
            validate_name(&citizen.name)?;
        }
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<(), NamedError> {
    if name.trim().is_empty() {
        return Err(NamedError::Invalid("identity name"));
    }
    if name.len() > MAX_NAMED_NAME_BYTES {
        return Err(NamedError::Invalid("identity name length"));
    }
    if name.chars().any(char::is_control) {
        return Err(NamedError::Invalid("identity name control"));
    }
    Ok(())
}

/// Errors returned by the named-citizen layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamedError {
    /// A range or invariant failed.
    Invalid(&'static str),
    /// JSON encoding or decoding failed.
    Json(String),
}

impl fmt::Display for NamedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(formatter, "invalid named citizens: {message}"),
            Self::Json(message) => write!(formatter, "named citizens JSON: {message}"),
        }
    }
}

impl From<serde_json::Error> for NamedError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error.to_string())
    }
}

impl std::error::Error for NamedError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn household(residents: u32) -> Household {
        Household {
            capacity: 8,
            residents,
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

    fn household_with_workers(residents: u32, workers: u32) -> Household {
        Household {
            workers,
            ..household(residents)
        }
    }

    #[test]
    fn ids_and_names_are_deterministic() {
        let id = citizen_id(2026, 7, 3);
        assert_eq!(id, citizen_id(2026, 7, 3));
        assert_ne!(id, citizen_id(2026, 8, 3));
        assert_ne!(id, citizen_id(2027, 7, 3));
        assert_ne!(id, citizen_id(2026, 7, 4));
        assert_eq!(name_for(id), name_for(id));
        assert!(!name_for(id).is_empty());
        assert!(id <= i64::MAX as u64);
    }

    #[test]
    fn age_years_is_deterministic_and_in_the_adult_band() {
        let mut seen = BTreeSet::new();
        for id in 1..=10_000 {
            let age = age_years(id);
            assert!((18..=64).contains(&age), "id {id} produced age {age}");
            assert_eq!(age_years(id), age, "id {id} must be deterministic");
            seen.insert(age);
        }
        assert!(seen.len() > 1, "ages should cover more than one year");
    }

    #[test]
    fn slot_is_employed_marks_only_leading_worker_slots() {
        let household = household_with_workers(4, 2);
        assert!(slot_is_employed(&household, 0));
        assert!(slot_is_employed(&household, household.workers - 1));
        assert!(!slot_is_employed(&household, household.workers));
        assert!(!slot_is_employed(&household, household.residents - 1));
        assert!(!slot_is_employed(&household, household.residents));

        let no_workers = household_with_workers(3, 0);
        assert!(!slot_is_employed(&no_workers, 0));
        let all_workers = household_with_workers(3, 3);
        assert!(slot_is_employed(&all_workers, 0));
        assert!(slot_is_employed(&all_workers, 2));
        assert!(!slot_is_employed(&all_workers, 3));
    }

    #[test]
    fn sync_fills_slots_and_is_stable() {
        let mut households = BTreeMap::new();
        households.insert(10, household(3));
        households.insert(20, household(1));
        let mut named = NamedCitizens::new(2026);
        named.sync(&households);
        assert_eq!(named.len(), 4);
        assert_eq!(named.citizens_of(10).len(), 3);
        assert_eq!(named.citizens_of(20).len(), 1);
        let before = named.citizens.clone();
        named.sync(&households);
        assert_eq!(named.citizens, before);
        assert!(named.validate().is_ok());
    }

    #[test]
    fn shrinking_and_removal_drop_slots() {
        let mut named = NamedCitizens::new(2026);
        let mut households = BTreeMap::new();
        households.insert(1, household(3));
        named.sync(&households);
        let kept = named.get(1, 0).expect("slot 0").id;
        households.insert(1, household(1));
        named.sync(&households);
        assert_eq!(named.len(), 1);
        assert_eq!(named.get(1, 0).expect("slot 0").id, kept);
        households.clear();
        named.sync(&households);
        assert!(named.is_empty());
        named.clear();
        assert_eq!(named.checksum(), NamedCitizens::new(2026).checksum());
    }

    #[test]
    fn the_pool_is_capped_deterministically() {
        let mut named = NamedCitizens::new(2026);
        named.max = 2;
        let mut households = BTreeMap::new();
        households.insert(1, household(2));
        households.insert(2, household(2));
        named.sync(&households);
        assert_eq!(named.len(), 2);
        assert!(named.get(1, 0).is_some());
        assert!(named.get(1, 1).is_some());
        assert!(named.get(2, 0).is_none());
        assert!(named.validate().is_ok());
    }

    #[test]
    fn four_thousand_identities_generate_quickly() {
        let mut households = BTreeMap::new();
        for id in 1..=1_000 {
            households.insert(id, household(4));
        }
        let start = std::time::Instant::now();
        let mut named = NamedCitizens::new(2026);
        named.sync(&households);
        assert_eq!(named.len(), 4_000);
        assert!(start.elapsed().as_millis() < 100);
    }

    #[test]
    fn save_json_round_trips_without_changing_ids_or_checksum() {
        let mut households = BTreeMap::new();
        households.insert(10, household(3));
        households.insert(20, household(1));
        let mut named = NamedCitizens::new(2026);
        named.sync(&households);

        let save = named.to_save();
        assert_eq!(save.format_version, NAMED_FORMAT_VERSION);
        assert_eq!(save.seed, 2026);
        assert_eq!(save.citizens.len(), 4);
        assert!(save
            .citizens
            .windows(2)
            .all(|pair| pair[0].id <= pair[1].id));

        let json = named.to_json_pretty().expect("serialize");
        let decoded = NamedCitizens::from_json(&json).expect("parse");
        assert_eq!(decoded, named);
        assert_eq!(decoded.checksum(), named.checksum());
        assert!(decoded.validate().is_ok());
    }

    #[test]
    fn save_validation_rejects_bad_version_and_ids() {
        let mut named = NamedCitizens::new(2026);
        let mut households = BTreeMap::new();
        households.insert(1, household(2));
        named.sync(&households);

        let mut save = named.to_save();
        save.format_version = 99;
        assert_eq!(save.validate(), Err(NamedError::Invalid("format_version")));
        let json = serde_json::to_string(&save).expect("json");
        assert_eq!(
            NamedSave::from_json(&json),
            Err(NamedError::Invalid("format_version"))
        );

        let mut save = named.to_save();
        save.citizens[0].id = 0;
        assert_eq!(save.validate(), Err(NamedError::Invalid("identity id")));

        let mut save = named.to_save();
        save.citizens[1].id = save.citizens[0].id;
        assert_eq!(
            save.validate(),
            Err(NamedError::Invalid("duplicate identity id"))
        );

        let mut save = named.to_save();
        save.citizens[1].slot = save.citizens[0].slot;
        assert_eq!(
            save.validate(),
            Err(NamedError::Invalid("duplicate identity slot"))
        );
    }

    #[test]
    fn loaded_pool_syncs_with_households_and_drops_orphans() {
        let mut households = BTreeMap::new();
        households.insert(10, household(2));
        households.insert(20, household(2));
        let mut named = NamedCitizens::new(2026);
        named.sync(&households);
        let kept = named.get(10, 0).expect("slot 0").id;
        let loaded =
            NamedCitizens::from_json(&named.to_json_pretty().expect("json")).expect("load");
        assert_eq!(loaded, named);

        let mut shrunk = BTreeMap::new();
        shrunk.insert(10, household(1));
        let mut pool = loaded;
        pool.sync(&shrunk);
        assert_eq!(pool.len(), 1);
        assert_eq!(pool.get(10, 0).expect("slot 0").id, kept);
        assert!(pool.get(20, 0).is_none());

        shrunk.insert(10, household(2));
        pool.sync(&shrunk);
        assert_eq!(
            pool.get(10, 1).expect("new slot").id,
            citizen_id(2026, 10, 1)
        );
        assert!(pool.validate().is_ok());
    }

    #[test]
    fn empty_save_object_is_valid() {
        let save = NamedSave {
            format_version: NAMED_FORMAT_VERSION,
            seed: 2026,
            max: u32::try_from(MAX_NAMED_CITIZENS).expect("max fits"),
            citizens: Vec::new(),
        };
        assert!(save.validate().is_ok());
        let pool = NamedCitizens::from_save(save).expect("empty pool");
        assert!(pool.is_empty());
        assert_eq!(pool.seed, 2026);
    }

    fn identity(id: u64, placement_id: u64, slot: u32) -> NamedCitizen {
        NamedCitizen {
            id,
            placement_id,
            slot,
            name: name_for(id),
            age_days: initial_age_days(id),
        }
    }

    fn pool_with(identities: &[(u64, u64, u32)]) -> NamedCitizens {
        let mut named = NamedCitizens::new(2026);
        for &(id, placement_id, slot) in identities {
            named
                .citizens
                .insert((placement_id, slot), identity(id, placement_id, slot));
        }
        named
    }

    fn center_map(entries: &[(u64, [f32; 2])]) -> BTreeMap<u64, [f32; 2]> {
        entries.iter().copied().collect()
    }

    fn selected_ids(citizens: &[MaterializedCitizen]) -> Vec<u64> {
        citizens.iter().map(|citizen| citizen.id).collect()
    }

    fn materialized(id: u64, distance: f32) -> MaterializedCitizen {
        MaterializedCitizen {
            id,
            placement_id: id,
            slot: 0,
            name: name_for(id),
            home: [distance, 0.0],
            distance,
        }
    }

    fn selection_with(citizens: &[(u64, f32)]) -> MaterializationSelection {
        MaterializationSelection {
            active: citizens
                .iter()
                .map(|&(id, distance)| materialized(id, distance))
                .collect(),
            detailed: Vec::new(),
        }
    }

    #[test]
    fn select_near_orders_by_distance_then_id() {
        let named = pool_with(&[(11, 101, 0), (12, 102, 0), (13, 103, 0)]);
        let centers = center_map(&[(101, [30.0, 0.0]), (102, [10.0, 0.0]), (103, [20.0, 0.0])]);
        let selection = named
            .select_near(
                [0.0, 0.0],
                &centers,
                MaterializationBudget::default(),
                100.0,
                0.0,
            )
            .expect("select");
        let distances: Vec<f32> = selection
            .active
            .iter()
            .map(|citizen| citizen.distance)
            .collect();
        assert_eq!(distances, vec![10.0, 20.0, 30.0]);
        assert_eq!(selected_ids(&selection.active), vec![12, 13, 11]);
        assert!(selection.detailed.is_empty());

        let nearest = &selection.active[0];
        assert_eq!(nearest.placement_id, 102);
        assert_eq!(nearest.slot, 0);
        assert_eq!(nearest.home, [10.0, 0.0]);
        assert_eq!(nearest.name, name_for(12));
    }

    #[test]
    fn select_near_breaks_ties_by_identity_id() {
        let named = pool_with(&[(9, 1, 0), (3, 2, 0), (5, 3, 0)]);
        let centers = center_map(&[(1, [10.0, 0.0]), (2, [-10.0, 0.0]), (3, [0.0, 10.0])]);
        let selection = named
            .select_near(
                [0.0, 0.0],
                &centers,
                MaterializationBudget::default(),
                10.0,
                10.0,
            )
            .expect("select");
        assert_eq!(selected_ids(&selection.active), vec![3, 5, 9]);
        assert!(selection
            .active
            .iter()
            .all(|citizen| citizen.distance == 10.0));
        assert_eq!(selected_ids(&selection.detailed), vec![3, 5, 9]);
    }

    #[test]
    fn select_near_applies_active_and_detailed_caps() {
        let named = pool_with(&[(1, 1, 0), (2, 2, 0), (3, 3, 0), (4, 4, 0)]);
        let centers = center_map(&[
            (1, [1.0, 0.0]),
            (2, [2.0, 0.0]),
            (3, [3.0, 0.0]),
            (4, [4.0, 0.0]),
        ]);
        let budget = MaterializationBudget {
            active_max: 2,
            detailed_max: 1,
        };
        let selection = named
            .select_near([0.0, 0.0], &centers, budget, 100.0, 100.0)
            .expect("select");
        assert_eq!(selected_ids(&selection.active), vec![1, 2]);
        assert_eq!(selected_ids(&selection.detailed), vec![1]);
        assert_eq!(selection.detailed[0], selection.active[0]);

        let selection = named
            .select_near(
                [0.0, 0.0],
                &centers,
                MaterializationBudget {
                    active_max: 4,
                    detailed_max: 4,
                },
                100.0,
                1.5,
            )
            .expect("select");
        assert_eq!(selected_ids(&selection.active), vec![1, 2, 3, 4]);
        assert_eq!(selected_ids(&selection.detailed), vec![1]);
    }

    #[test]
    fn select_near_excludes_out_of_range_and_unknown_homes() {
        let named = pool_with(&[(1, 1, 0), (2, 2, 0), (3, 3, 0), (4, 4, 0), (5, 5, 0)]);
        let centers = center_map(&[
            (1, [99.0, 0.0]),
            (2, [100.0, 0.0]),
            (3, [101.0, 0.0]),
            (4, [f32::NAN, 0.0]),
        ]);
        let selection = named
            .select_near(
                [0.0, 0.0],
                &centers,
                MaterializationBudget::default(),
                100.0,
                100.0,
            )
            .expect("select");
        assert_eq!(selected_ids(&selection.active), vec![1, 2]);
        assert_eq!(selected_ids(&selection.detailed), vec![1, 2]);
    }

    #[test]
    fn select_near_handles_empty_inputs() {
        let budget = MaterializationBudget::default();
        let empty_pool = NamedCitizens::new(2026);
        let centers = center_map(&[(1, [1.0, 0.0])]);
        let selection = empty_pool
            .select_near([0.0, 0.0], &centers, budget, 80.0, 40.0)
            .expect("select");
        assert!(selection.active.is_empty());
        assert!(selection.detailed.is_empty());

        let named = pool_with(&[(1, 1, 0)]);
        let empty_centers = BTreeMap::new();
        let selection = named
            .select_near([0.0, 0.0], &empty_centers, budget, 80.0, 40.0)
            .expect("select");
        assert!(selection.active.is_empty());
        assert!(selection.detailed.is_empty());
    }

    #[test]
    fn select_near_is_deterministic_and_does_not_mutate_identity() {
        let named = pool_with(&[(5, 1, 0), (2, 2, 0), (9, 3, 0)]);
        let centers = center_map(&[(1, [10.0, 0.0]), (2, [0.0, 10.0]), (3, [-10.0, 0.0])]);
        let before = named.checksum();
        let budget = MaterializationBudget::default();
        let first = named
            .select_near([0.0, 0.0], &centers, budget, 80.0, 40.0)
            .expect("select");
        let second = named
            .select_near([0.0, 0.0], &centers, budget, 80.0, 40.0)
            .expect("select");
        let reversed = center_map(&[(3, [-10.0, 0.0]), (2, [0.0, 10.0]), (1, [10.0, 0.0])]);
        let third = named
            .select_near([0.0, 0.0], &reversed, budget, 80.0, 40.0)
            .expect("select");
        assert_eq!(first, second);
        assert_eq!(first, third);
        assert_eq!(selected_ids(&first.active), vec![2, 5, 9]);
        assert_eq!(named.checksum(), before);
    }

    #[test]
    fn select_near_deduplicates_identity_ids() {
        let named = pool_with(&[(7, 1, 0), (7, 2, 0), (8, 3, 0)]);
        let centers = center_map(&[(1, [10.0, 0.0]), (2, [5.0, 0.0]), (3, [20.0, 0.0])]);
        let selection = named
            .select_near(
                [0.0, 0.0],
                &centers,
                MaterializationBudget::default(),
                100.0,
                100.0,
            )
            .expect("select");
        assert_eq!(selected_ids(&selection.active), vec![7, 8]);
        assert_eq!(selection.active[0].placement_id, 2);
        assert_eq!(selection.active[0].distance, 5.0);
        assert_eq!(selected_ids(&selection.detailed), vec![7, 8]);
    }

    #[test]
    fn materialization_budget_defaults_validate_and_enforce_hard_caps() {
        assert_eq!(MAX_ACTIVE_CITIZENS, 2_000);
        assert_eq!(MAX_DETAILED_CITIZENS, 200);

        let budget = MaterializationBudget::default();
        assert_eq!(budget.active_max, MAX_ACTIVE_CITIZENS);
        assert_eq!(budget.detailed_max, MAX_DETAILED_CITIZENS);
        assert!(budget.validate().is_ok());

        assert_eq!(
            MaterializationBudget {
                active_max: 0,
                ..budget
            }
            .validate(),
            Err(NamedError::Invalid("active materialization budget"))
        );
        assert_eq!(
            MaterializationBudget {
                active_max: MAX_ACTIVE_CITIZENS + 1,
                ..budget
            }
            .validate(),
            Err(NamedError::Invalid("active materialization budget"))
        );
        assert_eq!(
            MaterializationBudget {
                detailed_max: MAX_DETAILED_CITIZENS + 1,
                ..budget
            }
            .validate(),
            Err(NamedError::Invalid("detailed materialization budget"))
        );
        assert_eq!(
            MaterializationBudget {
                active_max: 100,
                detailed_max: 200,
            }
            .validate(),
            Err(NamedError::Invalid("detailed budget exceeds active budget"))
        );
        assert!(MaterializationBudget {
            active_max: 1,
            detailed_max: 0,
        }
        .validate()
        .is_ok());
    }

    fn previous_selection(ids: &[u64]) -> MaterializationSelection {
        MaterializationSelection {
            active: ids
                .iter()
                .map(|&id| MaterializedCitizen {
                    id,
                    placement_id: id,
                    slot: 0,
                    name: name_for(id),
                    home: [0.0, 0.0],
                    distance: 0.0,
                })
                .collect(),
            detailed: Vec::new(),
        }
    }

    #[test]
    fn hysteresis_keeps_between_radii_and_drops_beyond_outer() {
        let named = pool_with(&[(1, 1, 0), (2, 2, 0)]);
        let centers = center_map(&[(1, [100.0, 0.0]), (2, [130.0, 0.0])]);
        let budget = MaterializationBudget::default();

        let fresh = named
            .select_near_with_previous(None, [0.0, 0.0], &centers, budget, 80.0, 120.0, 40.0, None)
            .expect("select");
        assert!(fresh.active.is_empty());

        let previous = previous_selection(&[1, 2]);
        let selection = named
            .select_near_with_previous(
                Some(&previous),
                [0.0, 0.0],
                &centers,
                budget,
                80.0,
                120.0,
                40.0,
                None,
            )
            .expect("select");
        assert_eq!(selected_ids(&selection.active), vec![1]);
        assert_eq!(selection.active[0].distance, 100.0);
        assert!(selection.detailed.is_empty());
    }

    #[test]
    fn hysteresis_adds_new_citizen_inside_inner_radius() {
        let named = pool_with(&[(1, 1, 0), (2, 2, 0), (3, 3, 0)]);
        let centers = center_map(&[(1, [50.0, 0.0]), (2, [100.0, 0.0]), (3, [110.0, 0.0])]);
        let previous = previous_selection(&[2]);
        let selection = named
            .select_near_with_previous(
                Some(&previous),
                [0.0, 0.0],
                &centers,
                MaterializationBudget::default(),
                80.0,
                120.0,
                120.0,
                None,
            )
            .expect("select");
        assert_eq!(selected_ids(&selection.active), vec![1, 2]);
        assert_eq!(selected_ids(&selection.detailed), vec![1, 2]);
    }

    #[test]
    fn hysteresis_pinned_survives_outside_outer_and_active_cap() {
        let named = pool_with(&[(1, 1, 0), (2, 2, 0), (3, 3, 0), (4, 4, 0)]);
        let centers = center_map(&[
            (1, [10.0, 0.0]),
            (2, [20.0, 0.0]),
            (3, [500.0, 0.0]),
            (4, [f32::NAN, 0.0]),
        ]);
        let budget = MaterializationBudget {
            active_max: 2,
            detailed_max: 1,
        };
        let selection = named
            .select_near_with_previous(
                None,
                [0.0, 0.0],
                &centers,
                budget,
                80.0,
                120.0,
                40.0,
                Some(3),
            )
            .expect("select");
        assert_eq!(selected_ids(&selection.active), vec![1, 3]);
        assert_eq!(selected_ids(&selection.detailed), vec![1]);

        let unpinned = named
            .select_near_with_previous(
                None,
                [0.0, 0.0],
                &centers,
                budget,
                80.0,
                120.0,
                40.0,
                Some(99),
            )
            .expect("select");
        assert_eq!(selected_ids(&unpinned.active), vec![1, 2]);

        let invalid_home = named
            .select_near_with_previous(
                None,
                [0.0, 0.0],
                &centers,
                budget,
                80.0,
                120.0,
                40.0,
                Some(4),
            )
            .expect("select");
        assert_eq!(selected_ids(&invalid_home.active), vec![1, 2]);
    }

    #[test]
    fn hysteresis_caps_active_and_detailed_with_validation() {
        let named = pool_with(&[(1, 1, 0), (2, 2, 0), (3, 3, 0), (4, 4, 0)]);
        let centers = center_map(&[
            (1, [10.0, 0.0]),
            (2, [20.0, 0.0]),
            (3, [30.0, 0.0]),
            (4, [40.0, 0.0]),
        ]);
        let previous = previous_selection(&[1, 2, 3, 4]);
        let budget = MaterializationBudget {
            active_max: 2,
            detailed_max: 1,
        };
        let selection = named
            .select_near_with_previous(
                Some(&previous),
                [0.0, 0.0],
                &centers,
                budget,
                15.0,
                100.0,
                55.0,
                None,
            )
            .expect("select");
        assert_eq!(selected_ids(&selection.active), vec![1, 2]);
        assert_eq!(selected_ids(&selection.detailed), vec![1]);
        assert_eq!(selection.detailed[0], selection.active[0]);

        let budget = MaterializationBudget::default();
        let empty = BTreeMap::new();
        assert_eq!(
            named.select_near_with_previous(
                None,
                [0.0, 0.0],
                &empty,
                budget,
                80.0,
                40.0,
                40.0,
                None,
            ),
            Err(NamedError::Invalid(
                "inner materialization radius exceeds outer"
            ))
        );
        assert_eq!(
            named.select_near_with_previous(
                None,
                [0.0, 0.0],
                &empty,
                budget,
                -1.0,
                40.0,
                40.0,
                None,
            ),
            Err(NamedError::Invalid("inner materialization radius"))
        );
        assert_eq!(
            named.select_near_with_previous(
                None,
                [0.0, 0.0],
                &empty,
                budget,
                0.0,
                f32::NAN,
                40.0,
                None,
            ),
            Err(NamedError::Invalid("outer materialization radius"))
        );
        assert_eq!(
            named.select_near_with_previous(
                None,
                [0.0, 0.0],
                &empty,
                budget,
                0.0,
                40.0,
                f32::INFINITY,
                None,
            ),
            Err(NamedError::Invalid("detailed materialization radius"))
        );
    }

    #[test]
    fn select_near_rejects_invalid_arguments() {
        let named = NamedCitizens::new(2026);
        let centers = BTreeMap::new();
        let budget = MaterializationBudget::default();
        assert_eq!(
            named.select_near([f32::NAN, 0.0], &centers, budget, 80.0, 40.0),
            Err(NamedError::Invalid("materialization center"))
        );
        assert_eq!(
            named.select_near([0.0, 0.0], &centers, budget, -1.0, 40.0),
            Err(NamedError::Invalid("active materialization radius"))
        );
        assert_eq!(
            named.select_near([0.0, 0.0], &centers, budget, 80.0, f32::INFINITY),
            Err(NamedError::Invalid("detailed materialization radius"))
        );
        assert_eq!(
            named.select_near(
                [0.0, 0.0],
                &centers,
                MaterializationBudget {
                    active_max: 0,
                    detailed_max: 0,
                },
                80.0,
                40.0,
            ),
            Err(NamedError::Invalid("active materialization budget"))
        );
    }

    #[test]
    fn diff_selection_reports_no_change() {
        let previous = selection_with(&[(3, 10.0), (5, 20.0)]);
        let diff = diff_selection(Some(&previous), &previous, MAX_MATERIALIZATION_CHANGES);
        assert!(diff.is_empty());
        assert_eq!(diff.retained, 2);
        assert_eq!(diff.changed, 0);
        assert_eq!(diff.applied, 0);
        assert!(!diff.truncated);
        assert_eq!(MAX_MATERIALIZATION_CHANGES, 64);
    }

    #[test]
    fn diff_selection_ignores_distance_only_refresh() {
        let mut previous = selection_with(&[(3, 10.0), (5, 20.0)]);
        let mut next = selection_with(&[(3, 12.5), (5, 24.0)]);
        for citizen in &mut previous.active {
            citizen.home = [0.0, 0.0];
        }
        for citizen in &mut next.active {
            citizen.home = [0.0, 0.0];
        }
        let diff = diff_selection(Some(&previous), &next, MAX_MATERIALIZATION_CHANGES);
        assert!(diff.is_empty());
        assert!(diff.updated.is_empty());
        assert_eq!(diff.retained, 2);
        assert_eq!(diff.changed, 0);
        assert!(!diff.truncated);
    }

    #[test]
    fn diff_selection_adds_removes_updates_and_retains() {
        let previous = selection_with(&[(1, 10.0), (2, 20.0), (3, 30.0)]);
        let mut next = selection_with(&[(2, 20.0), (4, 40.0), (5, 50.0)]);
        next.active[0].home = [99.0, 0.0];

        let diff = diff_selection(Some(&previous), &next, MAX_MATERIALIZATION_CHANGES);
        assert_eq!(diff.removed, vec![1, 3]);
        assert_eq!(selected_ids(&diff.added), vec![4, 5]);
        assert_eq!(selected_ids(&diff.updated), vec![2]);
        assert_eq!(diff.updated[0].home, [99.0, 0.0]);
        assert_eq!(diff.retained, 1);
        assert_eq!(diff.changed, 5);
        assert_eq!(diff.applied, 5);
        assert!(!diff.truncated);
    }

    #[test]
    fn diff_selection_caps_to_nearest_changes() {
        let previous = selection_with(&[(1, 1.0), (2, 500.0)]);
        let next = selection_with(&[(1, 1.0), (3, 50.0)]);

        let diff = diff_selection(Some(&previous), &next, 1);
        assert_eq!(selected_ids(&diff.added), vec![3]);
        assert!(diff.removed.is_empty());
        assert_eq!(diff.retained, 1);
        assert_eq!(diff.changed, 2);
        assert_eq!(diff.applied, 1);
        assert!(diff.truncated);

        let none = diff_selection(Some(&previous), &next, 0);
        assert!(none.is_empty());
        assert!(none.truncated);
        assert_eq!(none.changed, 2);
        assert_eq!(none.applied, 0);
    }

    #[test]
    fn diff_selection_ties_break_by_id_and_ignore_input_order() {
        let previous = MaterializationSelection::default();
        let next = selection_with(&[(9, 10.0), (3, 10.0), (5, 10.0)]);
        let reversed = selection_with(&[(5, 10.0), (9, 10.0), (3, 10.0)]);

        let diff = diff_selection(Some(&previous), &next, 2);
        assert_eq!(selected_ids(&diff.added), vec![3, 5]);
        assert_eq!(diff.applied, 2);
        assert!(diff.truncated);
        assert_eq!(diff, diff_selection(Some(&previous), &next, 2));
        assert_eq!(diff, diff_selection(Some(&previous), &reversed, 2));
    }

    #[test]
    fn sync_initializes_age_days_from_identity() {
        let mut households = BTreeMap::new();
        households.insert(1, household(2));
        let mut named = NamedCitizens::new(2026);
        named.sync(&households);
        for citizen in named.citizens.values() {
            assert_eq!(citizen.age_days, initial_age_days(citizen.id));
            assert!(citizen.age_days >= 18 * 365);
            assert!(citizen.age_days < 65 * 365);
        }
        let before = named.citizens.clone();
        named.sync(&households);
        assert_eq!(named.citizens, before);
    }

    #[test]
    fn lifecycle_ages_and_removes_at_one_hundred_years() {
        let mut named = NamedCitizens::new(2026);
        let mut households = BTreeMap::new();
        households.insert(1, household(2));
        named.sync(&households);
        let first_id = named.get(1, 0).expect("first").id;
        let start = named.get(1, 0).expect("first").age_days;
        let tick = named.tick_lifecycle(10.0);
        assert_eq!(tick.aged, 2);
        assert_eq!(tick.died, 0);
        assert_eq!(named.get(1, 0).expect("first").age_days, start + 10);
        assert_eq!(named.oldest_age_days(), Some(start + 10));
        assert!(named.average_age_days() > 0);
        assert!(named.average_age_days() <= named.oldest_age_days().expect("oldest"));
        named.get_mut_for_test(first_id).age_days = MAX_AGE_DAYS - 1;
        let tick = named.tick_lifecycle(1.0);
        assert_eq!(tick.died, 1);
        assert_eq!(named.len(), 1);
        assert!(named.get(1, 0).is_none());
        assert_eq!(named.tick_lifecycle(f64::NAN).aged, 0);
    }

    #[test]
    fn old_save_without_age_days_defaults_and_resyncs() {
        let mut named = NamedCitizens::new(2026);
        let mut households = BTreeMap::new();
        households.insert(1, household(1));
        named.sync(&households);
        let mut save = named.to_save();
        save.citizens[0].age_days = 0;
        let json = serde_json::to_string(&save).expect("json");
        let decoded = NamedCitizens::from_json(&json).expect("decode");
        assert_eq!(
            decoded.citizens.values().next().expect("citizen").age_days,
            0
        );
        let mut decoded = decoded;
        decoded.sync(&households);
        assert!(decoded.citizens.values().next().expect("citizen").age_days >= 18 * 365);
    }

    #[test]
    fn age_days_round_trip_and_reject_over_cap() {
        let mut named = NamedCitizens::new(2026);
        let mut households = BTreeMap::new();
        households.insert(1, household(1));
        named.sync(&households);
        let json = named.to_json_pretty().expect("json");
        let decoded = NamedCitizens::from_json(&json).expect("decode");
        assert_eq!(decoded, named);
        let mut save = named.to_save();
        save.citizens[0].age_days = MAX_AGE_DAYS + 1;
        assert!(save.validate().is_err());
    }

    #[test]
    fn diff_selection_handles_empty_and_missing_previous() {
        let empty = MaterializationSelection::default();
        let diff = diff_selection(None, &empty, MAX_MATERIALIZATION_CHANGES);
        assert!(diff.is_empty());
        assert_eq!(diff.retained, 0);
        assert_eq!(diff.changed, 0);
        assert!(!diff.truncated);

        let next = selection_with(&[(4, 5.0), (2, 6.0)]);
        let diff = diff_selection(None, &next, MAX_MATERIALIZATION_CHANGES);
        assert_eq!(selected_ids(&diff.added), vec![2, 4]);
        assert_eq!(diff.retained, 0);
        assert_eq!(diff.changed, 2);
        assert_eq!(diff.applied, 2);
        assert!(!diff.truncated);

        let previous = selection_with(&[(3, 7.0), (1, 8.0)]);
        let diff = diff_selection(Some(&previous), &empty, MAX_MATERIALIZATION_CHANGES);
        assert_eq!(diff.removed, vec![1, 3]);
        assert_eq!(diff.retained, 0);
        assert_eq!(diff.changed, 2);
        assert_eq!(diff.applied, 2);
        assert!(!diff.truncated);
    }
}
