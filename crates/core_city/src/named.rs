//! Deterministic named-citizen identities.
//!
//! The layer is derived from the aggregate household runtime: every resident
//! slot gets a stable 63-bit id and a generated name from the world seed,
//! placement id, slot, and identity version. N4 adds an optional save object so
//! a committed identity survives a roundtrip; absent saves still rebuild the
//! pool from the households.

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

/// Hard cap for the city-wide active materialization set.
pub const MAX_ACTIVE_CITIZENS: usize = 2_000;

/// Hard cap for the per-client detailed materialization set.
pub const MAX_DETAILED_CITIZENS: usize = 200;

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
/// is a derived placeholder, not persisted state: the household layer stores
/// aggregate counts only, so there is no `age_days` field to read yet. The
/// result is stable for a given id, and changing this function changes the
/// displayed age of every identity.
#[must_use]
pub fn age_years(id: u64) -> u8 {
    18 + (mix64(id ^ 0x4528_21e6_38d0_1377) % 47) as u8
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
/// by id, then home and slot, so the JSON is deterministic. Absent objects are
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
            self.citizens.entry(key).or_insert_with(|| {
                let (placement_id, slot) = key;
                let id = citizen_id(self.seed, placement_id, slot);
                NamedCitizen {
                    id,
                    placement_id,
                    slot,
                    name: name_for(id),
                }
            });
        }
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

    /// Stable FNV-1a checksum over identities and names.
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
}
