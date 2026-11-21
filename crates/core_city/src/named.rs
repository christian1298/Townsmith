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

/// Identity function version. Changing this intentionally renames everyone.
pub const IDENTITY_VERSION: u64 = 1;

/// Current named-citizen save format.
pub const NAMED_FORMAT_VERSION: u16 = 1;

/// Maximum identities kept for one city.
pub const MAX_NAMED_CITIZENS: usize = 4_096;

/// Maximum display-name length in bytes.
pub const MAX_NAMED_NAME_BYTES: usize = 64;

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
            income_per_day: 0.0,
            home: [0.0, 0.0],
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
}
