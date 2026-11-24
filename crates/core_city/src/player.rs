//! Server-issued player identity for settlement ownership.
//!
//! Settlement ownership (D-033) needs an identity that survives a display-name
//! change. [`PlayerId`] is a per-save, server-issued id; [`PlayerRegistry`] maps
//! ids to mutable display labels. Authentication and account identity are out
//! of scope: the registry is a save-local address book, not a login system.
//!
//! Ids are nonzero and no larger than [`MAX_PLAYER_ID`], so a GDScript bridge
//! can carry them as signed 64-bit integers. The wire form is a bare number.
//! Display names are labels only: two players never share a name, but a rename
//! does not change any id or settlement reference.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Current player registry save format.
pub const PLAYER_FORMAT_VERSION: u16 = 1;

/// Hard cap for one save.
pub const MAX_PLAYERS: usize = 256;

/// Longest display name in characters.
pub const MAX_PLAYER_NAME_CHARS: usize = 48;

/// Largest id representable by the GDScript bridge.
pub const MAX_PLAYER_ID: u64 = i64::MAX as u64;

/// Stable server-issued player identity within one save.
///
/// The id is authoritative; [`Player::display_name`] is a mutable label. The
/// serde representation is the bare integer, and decoding rejects zero and
/// values above [`MAX_PLAYER_ID`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct PlayerId(u64);

impl PlayerId {
    /// Validate and wrap a raw id.
    ///
    /// # Errors
    ///
    /// Returns [`PlayerError::ZeroId`] for `0` and
    /// [`PlayerError::IdOutOfRange`] above [`MAX_PLAYER_ID`].
    pub fn new(value: u64) -> Result<Self, PlayerError> {
        Self::try_from(value)
    }

    /// Raw numeric id.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for PlayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for PlayerId {
    type Err = PlayerError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let value = text
            .parse::<u64>()
            .map_err(|_| PlayerError::InvalidIdText {
                text: text.to_string(),
            })?;
        Self::new(value)
    }
}

impl TryFrom<u64> for PlayerId {
    type Error = PlayerError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 0 {
            return Err(PlayerError::ZeroId);
        }
        if value > MAX_PLAYER_ID {
            return Err(PlayerError::IdOutOfRange { id: value });
        }
        Ok(Self(value))
    }
}

impl From<PlayerId> for u64 {
    fn from(id: PlayerId) -> Self {
        id.0
    }
}

/// One registered player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Player {
    /// Stable identity.
    pub id: PlayerId,
    /// Mutable player-facing label.
    pub display_name: String,
}

/// Player registry validation and persistence errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerError {
    /// A player id is zero.
    ZeroId,
    /// A player id exceeds [`MAX_PLAYER_ID`].
    IdOutOfRange {
        /// Rejected id.
        id: u64,
    },
    /// An id string is not a decimal `u64`.
    InvalidIdText {
        /// The rejected text.
        text: String,
    },
    /// The display name is empty or whitespace.
    EmptyName,
    /// The display name exceeds the character limit.
    NameTooLong {
        /// Maximum length.
        limit: usize,
    },
    /// Another player already uses the display name.
    DuplicateName,
    /// Two records share an id.
    DuplicateId {
        /// Duplicate id.
        id: PlayerId,
    },
    /// No player has this id.
    UnknownPlayer {
        /// Missing id.
        id: PlayerId,
    },
    /// The registry is full.
    TooMany,
    /// No ids remain.
    IdExhausted,
    /// JSON could not be parsed.
    InvalidJson(String),
    /// The save format version is not supported.
    UnsupportedVersion {
        /// Version found in the save.
        version: u16,
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
}

impl PlayerError {
    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ZeroId => "zero_id",
            Self::IdOutOfRange { .. } => "id_out_of_range",
            Self::InvalidIdText { .. } => "invalid_id_text",
            Self::EmptyName => "empty_name",
            Self::NameTooLong { .. } => "name_too_long",
            Self::DuplicateName => "duplicate_name",
            Self::DuplicateId { .. } => "duplicate_id",
            Self::UnknownPlayer { .. } => "unknown_player",
            Self::TooMany => "too_many",
            Self::IdExhausted => "id_exhausted",
            Self::InvalidJson(_) => "invalid_json",
            Self::UnsupportedVersion { .. } => "unsupported_version",
            Self::InvalidRecord { .. } => "invalid_record",
            Self::InvalidNextId { .. } => "invalid_next_id",
        }
    }
}

impl fmt::Display for PlayerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroId => f.write_str("player id must not be zero"),
            Self::IdOutOfRange { id } => write!(f, "player id {id} exceeds {MAX_PLAYER_ID}"),
            Self::InvalidIdText { text } => write!(f, "player id {text:?} is not a decimal u64"),
            Self::EmptyName => f.write_str("player display name is empty"),
            Self::NameTooLong { limit } => {
                write!(f, "player display name exceeds {limit} characters")
            }
            Self::DuplicateName => f.write_str("player display name is already in use"),
            Self::DuplicateId { id } => write!(f, "duplicate player id {id}"),
            Self::UnknownPlayer { id } => write!(f, "unknown player id {id}"),
            Self::TooMany => write!(f, "player limit of {MAX_PLAYERS} reached"),
            Self::IdExhausted => f.write_str("player id space is exhausted"),
            Self::InvalidJson(message) => write!(f, "invalid player JSON: {message}"),
            Self::UnsupportedVersion { version } => {
                write!(f, "unsupported player registry version {version}")
            }
            Self::InvalidRecord { id, reason } => write!(f, "invalid player {id}: {reason}"),
            Self::InvalidNextId { next_id } => write!(f, "player next_id {next_id} is invalid"),
        }
    }
}

impl Error for PlayerError {}

/// Persistent player address book for one save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerRegistry {
    /// Save format version.
    pub format_version: u16,
    /// World seed this registry belongs to.
    pub seed: u64,
    /// Next id to assign.
    pub next_id: u64,
    /// Players sorted by id.
    pub players: Vec<Player>,
}

impl PlayerRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            format_version: PLAYER_FORMAT_VERSION,
            seed,
            next_id: 1,
            players: Vec::new(),
        }
    }

    /// Number of registered players.
    #[must_use]
    pub fn count(&self) -> usize {
        self.players.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.players.is_empty()
    }

    /// Players in ascending id order.
    #[must_use]
    pub fn players(&self) -> &[Player] {
        &self.players
    }

    /// Find by id.
    #[must_use]
    pub fn get(&self, id: PlayerId) -> Option<&Player> {
        self.players.iter().find(|player| player.id == id)
    }

    /// Find every player whose display name matches, case-insensitively.
    ///
    /// A valid registry never has more than one match. The method returns all
    /// of them so a caller can treat a malformed registry as ambiguous instead
    /// of silently picking one.
    #[must_use]
    pub fn find_by_display_name(&self, display_name: &str) -> Vec<&Player> {
        let needle = display_name.trim().to_lowercase();
        self.players
            .iter()
            .filter(|player| player.display_name.to_lowercase() == needle)
            .collect()
    }

    /// Register a player and return it.
    ///
    /// # Errors
    ///
    /// Rejects an empty or over-long name, a duplicate name, a full registry,
    /// and id exhaustion.
    pub fn register(&mut self, display_name: &str) -> Result<Player, PlayerError> {
        let name = validate_display_name(display_name)?;
        if self
            .players
            .iter()
            .any(|player| names_equal(&player.display_name, name))
        {
            return Err(PlayerError::DuplicateName);
        }
        if self.players.len() >= MAX_PLAYERS {
            return Err(PlayerError::TooMany);
        }
        if self.next_id >= MAX_PLAYER_ID {
            return Err(PlayerError::IdExhausted);
        }
        let player = Player {
            id: PlayerId::new(self.next_id)?,
            display_name: name.to_string(),
        };
        self.next_id += 1;
        self.players.push(player.clone());
        self.players.sort_by_key(|player| player.id);
        Ok(player)
    }

    /// Rename a player without changing its id.
    ///
    /// # Errors
    ///
    /// Rejects an unknown id, an empty or over-long name, and a name already
    /// used by another player.
    pub fn rename(&mut self, id: PlayerId, display_name: &str) -> Result<Player, PlayerError> {
        let name = validate_display_name(display_name)?;
        let index = self
            .players
            .iter()
            .position(|player| player.id == id)
            .ok_or(PlayerError::UnknownPlayer { id })?;
        let duplicate = self
            .players
            .iter()
            .enumerate()
            .any(|(other, player)| other != index && names_equal(&player.display_name, name));
        if duplicate {
            return Err(PlayerError::DuplicateName);
        }
        self.players[index].display_name = name.to_string();
        Ok(self.players[index].clone())
    }

    /// Stable 64-bit checksum over the registry.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let bytes = serde_json::to_vec(self).expect("player registry serialization cannot fail");
        fnv1a_64(&bytes)
    }

    /// Pretty JSON used by the world save.
    pub fn to_json_pretty(&self) -> Result<String, PlayerError> {
        serde_json::to_string_pretty(self)
            .map_err(|error| PlayerError::InvalidJson(error.to_string()))
    }

    /// Parse and validate a player registry.
    pub fn from_json(json: &str) -> Result<Self, PlayerError> {
        let registry: Self = serde_json::from_str(json)
            .map_err(|error| PlayerError::InvalidJson(error.to_string()))?;
        registry.validate_loaded()?;
        Ok(registry)
    }

    /// Validate a decoded registry, including ids, ordering, names, and version.
    pub fn validate_loaded(&self) -> Result<(), PlayerError> {
        if self.format_version != PLAYER_FORMAT_VERSION {
            return Err(PlayerError::UnsupportedVersion {
                version: self.format_version,
            });
        }
        if self.players.len() > MAX_PLAYERS {
            return Err(PlayerError::TooMany);
        }
        let mut names = HashSet::new();
        let mut previous: Option<PlayerId> = None;
        let mut max_id = 0;
        for player in &self.players {
            if let Some(previous_id) = previous {
                if player.id == previous_id {
                    return Err(PlayerError::DuplicateId { id: player.id });
                }
                if player.id < previous_id {
                    return Err(PlayerError::InvalidRecord {
                        id: player.id.get(),
                        reason: "players are not sorted",
                    });
                }
            }
            if player.display_name != player.display_name.trim() {
                return Err(PlayerError::InvalidRecord {
                    id: player.id.get(),
                    reason: "display name is not trimmed",
                });
            }
            validate_display_name(&player.display_name)?;
            if !names.insert(player.display_name.to_lowercase()) {
                return Err(PlayerError::DuplicateName);
            }
            previous = Some(player.id);
            max_id = max_id.max(player.id.get());
        }
        if self.next_id == 0 || self.next_id <= max_id || self.next_id >= MAX_PLAYER_ID {
            return Err(PlayerError::InvalidNextId {
                next_id: self.next_id,
            });
        }
        Ok(())
    }
}

fn validate_display_name(display_name: &str) -> Result<&str, PlayerError> {
    let name = display_name.trim();
    if name.is_empty() {
        return Err(PlayerError::EmptyName);
    }
    if name.chars().any(char::is_control) {
        return Err(PlayerError::InvalidRecord {
            id: 0,
            reason: "display name contains a control character",
        });
    }
    if name.chars().count() > MAX_PLAYER_NAME_CHARS {
        return Err(PlayerError::NameTooLong {
            limit: MAX_PLAYER_NAME_CHARS,
        });
    }
    Ok(name)
}

fn names_equal(left: &str, right: &str) -> bool {
    left.to_lowercase() == right.to_lowercase()
}

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: u64) -> PlayerId {
        PlayerId::new(value).expect("player id")
    }

    #[test]
    fn ids_reject_zero_and_oversized_values() {
        assert_eq!(PlayerId::new(0), Err(PlayerError::ZeroId));
        assert_eq!(
            PlayerId::new(MAX_PLAYER_ID + 1),
            Err(PlayerError::IdOutOfRange {
                id: MAX_PLAYER_ID + 1
            })
        );
        assert_eq!(id(MAX_PLAYER_ID).get(), MAX_PLAYER_ID);
    }

    #[test]
    fn id_round_trips_through_text_and_json() {
        let player = id(42);
        assert_eq!(player.to_string(), "42");
        assert_eq!("42".parse::<PlayerId>(), Ok(player));
        assert!(matches!("0".parse::<PlayerId>(), Err(PlayerError::ZeroId)));
        assert!(matches!(
            "abc".parse::<PlayerId>(),
            Err(PlayerError::InvalidIdText { .. })
        ));
        assert_eq!(serde_json::to_string(&player).expect("json"), "42");
        assert_eq!(
            serde_json::from_str::<PlayerId>("42").expect("decode"),
            player
        );
        assert!(serde_json::from_str::<PlayerId>("0").is_err());
    }

    #[test]
    fn register_assigns_sequential_ids_and_trims_names() {
        let mut registry = PlayerRegistry::new(2026);
        let first = registry.register("  Ada  ").expect("register");
        let second = registry.register("Grace").expect("register");
        assert_eq!(first.id, id(1));
        assert_eq!(first.display_name, "Ada");
        assert_eq!(second.id, id(2));
        assert_eq!(registry.count(), 2);
        assert_eq!(registry.next_id, 3);
        assert_eq!(
            registry.get(id(2)).map(|p| p.display_name.as_str()),
            Some("Grace")
        );
    }

    #[test]
    fn register_rejects_duplicate_names_case_insensitively() {
        let mut registry = PlayerRegistry::new(2026);
        registry.register("Ada").expect("register");
        assert_eq!(registry.register("  ada "), Err(PlayerError::DuplicateName));
        assert_eq!(registry.count(), 1);
    }

    #[test]
    fn register_rejects_empty_and_over_long_names() {
        let mut registry = PlayerRegistry::new(2026);
        assert_eq!(registry.register("   "), Err(PlayerError::EmptyName));
        assert_eq!(
            registry.register(&"a".repeat(MAX_PLAYER_NAME_CHARS + 1)),
            Err(PlayerError::NameTooLong {
                limit: MAX_PLAYER_NAME_CHARS
            })
        );
        assert_eq!(
            registry.register("bad\u{7}name"),
            Err(PlayerError::InvalidRecord {
                id: 0,
                reason: "display name contains a control character"
            })
        );
        assert!(registry.is_empty());
    }

    #[test]
    fn rename_keeps_the_id_and_rejects_other_players_names() {
        let mut registry = PlayerRegistry::new(2026);
        let ada = registry.register("Ada").expect("register");
        registry.register("Grace").expect("register");
        let renamed = registry.rename(ada.id, "Ada Lovelace").expect("rename");
        assert_eq!(renamed.id, ada.id);
        assert_eq!(
            registry.get(ada.id).map(|p| p.display_name.as_str()),
            Some("Ada Lovelace")
        );
        assert_eq!(
            registry.rename(ada.id, "grace"),
            Err(PlayerError::DuplicateName)
        );
        assert_eq!(
            registry.rename(id(99), "Nobody"),
            Err(PlayerError::UnknownPlayer { id: id(99) })
        );
    }

    #[test]
    fn find_by_display_name_returns_every_case_insensitive_match() {
        let mut registry = PlayerRegistry::new(2026);
        let player = Player {
            id: id(1),
            display_name: "Ada".to_string(),
        };
        registry.players = vec![
            player.clone(),
            Player {
                id: id(2),
                display_name: "ada".to_string(),
            },
            Player {
                id: id(3),
                display_name: "Grace".to_string(),
            },
        ];
        let matches = registry.find_by_display_name(" ADA ");
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].id, id(1));
        assert_eq!(matches[1].id, id(2));
        assert!(registry.find_by_display_name("nobody").is_empty());
    }

    #[test]
    fn registry_json_round_trip_and_checksum_are_stable() {
        let mut registry = PlayerRegistry::new(2026);
        registry.register("Ada").expect("register");
        registry.register("Grace").expect("register");
        let json = registry.to_json_pretty().expect("json");
        let decoded = PlayerRegistry::from_json(&json).expect("decode");
        assert_eq!(decoded, registry);
        assert_eq!(decoded.checksum(), registry.checksum());
        assert_ne!(registry.checksum(), PlayerRegistry::new(2026).checksum());
        registry.rename(id(1), "Ada L").expect("rename");
        assert_ne!(decoded.checksum(), registry.checksum());
    }

    #[test]
    fn validation_rejects_duplicate_ids_unsorted_records_and_bad_next_id() {
        let mut registry = PlayerRegistry::new(2026);
        registry.register("Ada").expect("register");
        let player = registry.players[0].clone();

        let mut duplicate = registry.clone();
        duplicate.players.push(player.clone());
        assert_eq!(
            duplicate.validate_loaded(),
            Err(PlayerError::DuplicateId { id: id(1) })
        );

        let mut unsorted = PlayerRegistry::new(2026);
        unsorted.players = vec![
            Player {
                id: id(2),
                display_name: "Grace".to_string(),
            },
            player.clone(),
        ];
        unsorted.next_id = 3;
        assert!(matches!(
            unsorted.validate_loaded(),
            Err(PlayerError::InvalidRecord {
                reason: "players are not sorted",
                ..
            })
        ));

        let mut bad_next = registry.clone();
        bad_next.next_id = 1;
        assert_eq!(
            bad_next.validate_loaded(),
            Err(PlayerError::InvalidNextId { next_id: 1 })
        );
    }

    #[test]
    fn validation_rejects_untrimmed_duplicate_and_unknown_version_records() {
        let mut registry = PlayerRegistry::new(2026);
        registry.register("Ada").expect("register");

        let mut padded = registry.clone();
        padded.players[0].display_name = " Ada ".to_string();
        assert!(matches!(
            padded.validate_loaded(),
            Err(PlayerError::InvalidRecord { .. })
        ));

        let mut duplicate = registry.clone();
        duplicate.players[0].display_name = "Ada".to_string();
        duplicate.players.push(Player {
            id: id(2),
            display_name: "ADA".to_string(),
        });
        duplicate.next_id = 3;
        assert_eq!(duplicate.validate_loaded(), Err(PlayerError::DuplicateName));

        let mut versioned = registry.clone();
        versioned.format_version = 99;
        assert_eq!(
            versioned.validate_loaded(),
            Err(PlayerError::UnsupportedVersion { version: 99 })
        );
    }

    #[test]
    fn registry_enforces_the_player_limit() {
        let mut registry = PlayerRegistry::new(2026);
        for index in 0..MAX_PLAYERS {
            registry
                .register(&format!("player {index}"))
                .expect("register");
        }
        assert_eq!(registry.count(), MAX_PLAYERS);
        assert_eq!(registry.register("one too many"), Err(PlayerError::TooMany));
    }
}
