//! Soft settlement boundaries and ownership.
//!
//! A settlement is a player-named circle on the terrain. Boundaries are soft
//! and may overlap; the nearest containing settlement owns a point. This slice
//! stores identity and ownership only. District boundaries, taxes, policies,
//! and service coverage are later work.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Current settlement save format.
pub const SETTLEMENT_FORMAT_VERSION: u16 = 1;

/// Smallest allowed settlement radius in metres.
pub const MIN_SETTLEMENT_RADIUS_M: u32 = 16;

/// Largest allowed settlement radius in metres.
pub const MAX_SETTLEMENT_RADIUS_M: u32 = 2_000;

/// Hard cap for one save.
pub const MAX_SETTLEMENTS: usize = 1_000;

/// Longest settlement name in characters.
pub const MAX_SETTLEMENT_NAME_CHARS: usize = 48;

/// Longest owner name in characters.
pub const MAX_SETTLEMENT_OWNER_CHARS: usize = 48;

/// Largest id representable by the GDScript bridge.
pub const MAX_SETTLEMENT_ID: u64 = i64::MAX as u64;

const MAX_SETTLEMENT_COORD: i32 = 1_000_000;

/// One named settlement and its soft boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settlement {
    /// Stable id within the world.
    pub id: u64,
    /// Player-facing name.
    pub name: String,
    /// Owner name, group, or server role.
    pub owner: String,
    /// Centre `[x, z]` in metres.
    pub center: [i32; 2],
    /// Soft boundary radius in metres.
    pub radius_m: u32,
}

impl Settlement {
    /// Whether a world cell is inside the soft boundary.
    #[must_use]
    pub fn contains(&self, x: i32, z: i32) -> bool {
        let dx = i128::from(x) - i128::from(self.center[0]);
        let dz = i128::from(z) - i128::from(self.center[1]);
        let radius = i128::from(self.radius_m);
        dx * dx + dz * dz <= radius * radius
    }

    /// Squared distance from the centre, used for nearest-owner ties.
    #[must_use]
    pub fn distance_squared(&self, x: i32, z: i32) -> i128 {
        let dx = i128::from(x) - i128::from(self.center[0]);
        let dz = i128::from(z) - i128::from(self.center[1]);
        dx * dx + dz * dz
    }
}

/// A request to found one settlement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementRequest {
    /// Player-facing name.
    pub name: String,
    /// Owner name, group, or server role.
    pub owner: String,
    /// Centre `[x, z]` in metres.
    pub center: [i32; 2],
    /// Soft boundary radius in metres.
    pub radius_m: u32,
}

/// Settlement validation and persistence errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettlementError {
    /// The name is empty or whitespace.
    EmptyName,
    /// The name exceeds the character limit.
    NameTooLong {
        /// Maximum length.
        limit: usize,
    },
    /// The owner is empty or whitespace.
    EmptyOwner,
    /// The owner exceeds the character limit.
    OwnerTooLong {
        /// Maximum length.
        limit: usize,
    },
    /// Radius is outside the allowed range.
    InvalidRadius {
        /// Requested radius.
        radius: u32,
    },
    /// A coordinate is outside the world range.
    CoordinateOutOfRange,
    /// Another settlement already uses the name.
    DuplicateName,
    /// The store is full.
    TooMany,
    /// JSON could not be parsed.
    InvalidJson(String),
    /// The save format version is not supported.
    UnsupportedVersion {
        /// Version found in the save.
        version: u16,
    },
    /// Two records share an id.
    DuplicateId {
        /// Duplicate id.
        id: u64,
    },
    /// A record is malformed.
    InvalidRecord {
        /// Record id, or zero if the id itself is invalid.
        id: u64,
        /// Short reason.
        reason: &'static str,
    },
    /// The next id is zero or not greater than every stored id.
    InvalidNextId {
        /// Next id found in the save.
        next_id: u64,
    },
    /// No ids remain.
    IdExhausted,
}

impl SettlementError {
    /// Stable machine-readable code used by the Godot bridge.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::EmptyName => "empty_name",
            Self::NameTooLong { .. } => "name_too_long",
            Self::EmptyOwner => "empty_owner",
            Self::OwnerTooLong { .. } => "owner_too_long",
            Self::InvalidRadius { .. } => "invalid_radius",
            Self::CoordinateOutOfRange => "coordinate_out_of_range",
            Self::DuplicateName => "duplicate_name",
            Self::TooMany => "too_many",
            Self::InvalidJson(_) => "invalid_json",
            Self::UnsupportedVersion { .. } => "unsupported_version",
            Self::DuplicateId { .. } => "duplicate_id",
            Self::InvalidRecord { .. } => "invalid_record",
            Self::InvalidNextId { .. } => "invalid_next_id",
            Self::IdExhausted => "id_exhausted",
        }
    }
}

impl fmt::Display for SettlementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => f.write_str("settlement name is empty"),
            Self::NameTooLong { limit } => {
                write!(f, "settlement name exceeds {limit} characters")
            }
            Self::EmptyOwner => f.write_str("settlement owner is empty"),
            Self::OwnerTooLong { limit } => {
                write!(f, "settlement owner exceeds {limit} characters")
            }
            Self::InvalidRadius { radius } => write!(
                f,
                "settlement radius {radius} m is outside {MIN_SETTLEMENT_RADIUS_M}..={MAX_SETTLEMENT_RADIUS_M} m"
            ),
            Self::CoordinateOutOfRange => f.write_str("settlement centre is outside the world"),
            Self::DuplicateName => f.write_str("settlement name is already in use"),
            Self::TooMany => write!(f, "settlement limit of {MAX_SETTLEMENTS} reached"),
            Self::InvalidJson(message) => write!(f, "invalid settlement JSON: {message}"),
            Self::UnsupportedVersion { version } => {
                write!(f, "unsupported settlement version {version}")
            }
            Self::DuplicateId { id } => write!(f, "duplicate settlement id {id}"),
            Self::InvalidRecord { id, reason } => {
                write!(f, "invalid settlement {id}: {reason}")
            }
            Self::InvalidNextId { next_id } => {
                write!(f, "settlement next_id {next_id} is invalid")
            }
            Self::IdExhausted => f.write_str("settlement id space is exhausted"),
        }
    }
}

impl Error for SettlementError {}

/// Persistent settlement layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementStore {
    /// Save format version.
    pub format_version: u16,
    /// World seed this layer belongs to.
    pub seed: u64,
    /// Next id to assign.
    pub next_id: u64,
    /// Settlements sorted by id.
    pub settlements: Vec<Settlement>,
}

impl SettlementStore {
    /// Create an empty store.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            format_version: SETTLEMENT_FORMAT_VERSION,
            seed,
            next_id: 1,
            settlements: Vec::new(),
        }
    }

    /// Number of settlements.
    #[must_use]
    pub fn count(&self) -> usize {
        self.settlements.len()
    }

    /// Whether the layer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.settlements.is_empty()
    }

    /// Settlements in id order.
    #[must_use]
    pub fn settlements(&self) -> &[Settlement] {
        &self.settlements
    }

    /// Find by id.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&Settlement> {
        self.settlements
            .iter()
            .find(|settlement| settlement.id == id)
    }

    /// Validate a request without mutating.
    pub fn validate(&self, request: &SettlementRequest) -> Result<(), SettlementError> {
        validate_request(request)?;
        let requested = request.name.trim().to_lowercase();
        if self
            .settlements
            .iter()
            .any(|settlement| settlement.name.to_lowercase() == requested)
        {
            return Err(SettlementError::DuplicateName);
        }
        if self.settlements.len() >= MAX_SETTLEMENTS {
            return Err(SettlementError::TooMany);
        }
        Ok(())
    }

    /// Found a settlement and return it.
    pub fn found(&mut self, request: &SettlementRequest) -> Result<Settlement, SettlementError> {
        self.validate(request)?;
        if self.next_id >= MAX_SETTLEMENT_ID {
            return Err(SettlementError::IdExhausted);
        }
        let settlement = Settlement {
            id: self.next_id,
            name: request.name.trim().to_string(),
            owner: request.owner.trim().to_string(),
            center: request.center,
            radius_m: request.radius_m,
        };
        self.next_id += 1;
        self.settlements.push(settlement.clone());
        self.settlements.sort_by_key(|settlement| settlement.id);
        Ok(settlement)
    }

    /// Remove one settlement by id.
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.settlements.len();
        self.settlements.retain(|settlement| settlement.id != id);
        self.settlements.len() != before
    }

    /// Remove every settlement; the seed and next id stay.
    pub fn clear(&mut self) {
        self.settlements.clear();
    }

    /// Nearest settlement containing a world cell.
    #[must_use]
    pub fn owner_at(&self, x: i32, z: i32) -> Option<&Settlement> {
        self.settlements
            .iter()
            .filter(|settlement| settlement.contains(x, z))
            .min_by_key(|settlement| (settlement.distance_squared(x, z), settlement.id))
    }

    /// Settlement containing a world cell, nearest first.
    #[must_use]
    pub fn at(&self, x: i32, z: i32) -> Option<&Settlement> {
        self.owner_at(x, z)
    }

    /// Stable 64-bit checksum over the settlement layer.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let bytes = serde_json::to_vec(self).expect("settlement store serialization cannot fail");
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    /// Pretty JSON used by the world save.
    pub fn to_json_pretty(&self) -> Result<String, SettlementError> {
        serde_json::to_string_pretty(self)
            .map_err(|error| SettlementError::InvalidJson(error.to_string()))
    }

    /// Parse and validate a settlement layer.
    pub fn from_json(json: &str) -> Result<Self, SettlementError> {
        let store: Self = serde_json::from_str(json)
            .map_err(|error| SettlementError::InvalidJson(error.to_string()))?;
        store.validate_loaded()?;
        Ok(store)
    }

    /// Validate a decoded store, including ids and duplicate names.
    pub fn validate_loaded(&self) -> Result<(), SettlementError> {
        if self.format_version != SETTLEMENT_FORMAT_VERSION {
            return Err(SettlementError::UnsupportedVersion {
                version: self.format_version,
            });
        }
        if self.settlements.len() > MAX_SETTLEMENTS {
            return Err(SettlementError::TooMany);
        }
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        let mut max_id = 0;
        for settlement in &self.settlements {
            if settlement.id == 0 || settlement.id > MAX_SETTLEMENT_ID {
                return Err(SettlementError::InvalidRecord {
                    id: settlement.id,
                    reason: "id out of range",
                });
            }
            if !ids.insert(settlement.id) {
                return Err(SettlementError::DuplicateId { id: settlement.id });
            }
            if settlement.name != settlement.name.trim()
                || settlement.owner != settlement.owner.trim()
            {
                return Err(SettlementError::InvalidRecord {
                    id: settlement.id,
                    reason: "untrimmed name or owner",
                });
            }
            let request = SettlementRequest {
                name: settlement.name.clone(),
                owner: settlement.owner.clone(),
                center: settlement.center,
                radius_m: settlement.radius_m,
            };
            validate_request(&request).map_err(|error| SettlementError::InvalidRecord {
                id: settlement.id,
                reason: match error {
                    SettlementError::EmptyName => "empty name",
                    SettlementError::NameTooLong { .. } => "name too long",
                    SettlementError::EmptyOwner => "empty owner",
                    SettlementError::OwnerTooLong { .. } => "owner too long",
                    SettlementError::InvalidRadius { .. } => "invalid radius",
                    SettlementError::CoordinateOutOfRange => "coordinate out of range",
                    _ => "invalid record",
                },
            })?;
            if !names.insert(settlement.name.to_lowercase()) {
                return Err(SettlementError::DuplicateName);
            }
            max_id = max_id.max(settlement.id);
        }
        if self.next_id == 0 || self.next_id <= max_id || self.next_id >= MAX_SETTLEMENT_ID {
            return Err(SettlementError::InvalidNextId {
                next_id: self.next_id,
            });
        }
        Ok(())
    }
}

fn validate_request(request: &SettlementRequest) -> Result<(), SettlementError> {
    let name = request.name.trim();
    if name.is_empty() {
        return Err(SettlementError::EmptyName);
    }
    if name.chars().any(char::is_control) {
        return Err(SettlementError::InvalidRecord {
            id: 0,
            reason: "name contains a control character",
        });
    }
    if name.chars().count() > MAX_SETTLEMENT_NAME_CHARS {
        return Err(SettlementError::NameTooLong {
            limit: MAX_SETTLEMENT_NAME_CHARS,
        });
    }
    let owner = request.owner.trim();
    if owner.is_empty() {
        return Err(SettlementError::EmptyOwner);
    }
    if owner.chars().any(char::is_control) {
        return Err(SettlementError::InvalidRecord {
            id: 0,
            reason: "owner contains a control character",
        });
    }
    if owner.chars().count() > MAX_SETTLEMENT_OWNER_CHARS {
        return Err(SettlementError::OwnerTooLong {
            limit: MAX_SETTLEMENT_OWNER_CHARS,
        });
    }
    if !(MIN_SETTLEMENT_RADIUS_M..=MAX_SETTLEMENT_RADIUS_M).contains(&request.radius_m) {
        return Err(SettlementError::InvalidRadius {
            radius: request.radius_m,
        });
    }
    if request.center[0].abs() > MAX_SETTLEMENT_COORD
        || request.center[1].abs() > MAX_SETTLEMENT_COORD
    {
        return Err(SettlementError::CoordinateOutOfRange);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(name: &str, x: i32, z: i32, radius: u32) -> SettlementRequest {
        SettlementRequest {
            name: name.to_string(),
            owner: "player".to_string(),
            center: [x, z],
            radius_m: radius,
        }
    }

    #[test]
    fn found_assigns_ids_and_contains_cells() {
        let mut store = SettlementStore::new(2026);
        let settlement = store
            .found(&request("Riverbend", 100, 100, 64))
            .expect("found");
        assert_eq!(settlement.id, 1);
        assert_eq!(store.count(), 1);
        assert!(settlement.contains(100, 100));
        assert!(settlement.contains(164, 100));
        assert!(!settlement.contains(165, 100));
        assert_eq!(store.owner_at(120, 100).map(|s| s.id), Some(1));
        assert!(store.owner_at(0, 0).is_none());
    }

    #[test]
    fn duplicate_names_are_case_insensitive() {
        let mut store = SettlementStore::new(2026);
        store.found(&request("Riverbend", 0, 0, 32)).expect("found");
        let duplicate = store.found(&request("  riverbend ", 100, 0, 32));
        assert_eq!(duplicate, Err(SettlementError::DuplicateName));
    }

    #[test]
    fn rejects_invalid_names_owners_radii_and_coordinates() {
        let mut store = SettlementStore::new(2026);
        assert_eq!(
            store.found(&request("", 0, 0, 32)),
            Err(SettlementError::EmptyName)
        );
        assert_eq!(
            store.found(&request("A", 0, 0, 0)),
            Err(SettlementError::InvalidRadius { radius: 0 })
        );
        assert_eq!(
            store.found(&request("A", 1_000_001, 0, 32)),
            Err(SettlementError::CoordinateOutOfRange)
        );
        let mut invalid_owner = request("A", 0, 0, 32);
        invalid_owner.owner = "  ".to_string();
        assert_eq!(
            store.found(&invalid_owner),
            Err(SettlementError::EmptyOwner)
        );
    }

    #[test]
    fn nearest_containing_settlement_wins() {
        let mut store = SettlementStore::new(2026);
        store.found(&request("Outer", 0, 0, 100)).expect("outer");
        store.found(&request("Inner", 10, 0, 20)).expect("inner");
        assert_eq!(
            store.owner_at(10, 0).map(|s| s.name.as_str()),
            Some("Inner")
        );
        assert_eq!(
            store.owner_at(80, 0).map(|s| s.name.as_str()),
            Some("Outer")
        );
    }

    #[test]
    fn json_round_trip_and_checksum_are_stable() {
        let mut store = SettlementStore::new(2026);
        store
            .found(&request("Riverbend", 100, 100, 64))
            .expect("found");
        let json = store.to_json_pretty().expect("json");
        let decoded = SettlementStore::from_json(&json).expect("decode");
        assert_eq!(decoded, store);
        assert_eq!(decoded.checksum(), store.checksum());
    }

    #[test]
    fn load_rejects_duplicate_ids_and_bad_next_id() {
        let mut store = SettlementStore::new(2026);
        let settlement = store.found(&request("A", 0, 0, 32)).expect("found");
        let mut duplicate = store.clone();
        duplicate.settlements.push(settlement);
        assert_eq!(
            duplicate.validate_loaded(),
            Err(SettlementError::DuplicateId { id: 1 })
        );
        let mut bad_next = store.clone();
        bad_next.next_id = 1;
        assert_eq!(
            bad_next.validate_loaded(),
            Err(SettlementError::InvalidNextId { next_id: 1 })
        );
    }

    #[test]
    fn extreme_lookup_coordinates_do_not_overflow() {
        let mut store = SettlementStore::new(2026);
        let settlement = store
            .found(&request("Edge", -1_000_000, -1_000_000, 16))
            .expect("found");
        assert!(!settlement.contains(i32::MAX, i32::MAX));
        assert!(!settlement.contains(-1_000_000, i32::MAX));
    }

    #[test]
    fn nearest_tie_breaks_on_the_lower_id() {
        let mut store = SettlementStore::new(2026);
        store.found(&request("First", -10, 0, 32)).expect("first");
        store.found(&request("Second", 10, 0, 32)).expect("second");
        assert_eq!(
            store.owner_at(0, 0).map(|settlement| settlement.id),
            Some(1)
        );
    }

    #[test]
    fn load_rejects_untrimmed_names() {
        let mut store = SettlementStore::new(2026);
        let settlement = store.found(&request("A", 0, 0, 32)).expect("found");
        let mut padded = store.clone();
        padded.settlements[0].name = format!("{} ", settlement.name);
        assert!(padded.validate_loaded().is_err());
    }

    #[test]
    fn remove_and_clear_keep_the_seed() {
        let mut store = SettlementStore::new(2026);
        let settlement = store.found(&request("A", 0, 0, 32)).expect("found");
        assert!(store.remove(settlement.id));
        assert!(!store.remove(settlement.id));
        store.found(&request("B", 0, 0, 32)).expect("found");
        store.clear();
        assert!(store.is_empty());
        assert_eq!(store.seed, 2026);
    }
}
