//! Persistence for the authoritative [`ServerWorld`].
//!
//! v0 persists one world to one pretty-printed JSON file. The file contains the
//! seed, clock, static population counters, paused flag, and the serialized
//! [`Economy`] and [`RoadStore`]. Loading validates the format version, seeds,
//! counts, road records, and economy invariants before a world is constructed.
//!
//! Writes are atomic: the new document is written to `<path>.tmp`, flushed and
//! synced, then renamed over the primary. The previous valid primary is rotated
//! to `<path>.bak` first. A load that finds a corrupt primary falls back to the
//! backup and reports `recovered_from_backup`.
//!
//! There is no journal, compression, remote storage, multi-world support, or
//! authentication here. Those are later milestones; see
//! `docs/12 - Technical Architecture.md` §7.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use core_city::construction::{ConstructionError, ConstructionStore};
use core_city::economy::{Economy, EconomyError};
use core_city::placement::{PlacementError, PlacementStore};
use core_city::player::{PlayerError, PlayerRegistry};
use core_city::road::{RoadError, RoadStore};
use core_city::settlement::{SettlementError, SettlementStore};
use serde::{Deserialize, Serialize};

use crate::world::ServerWorld;

/// Current server save format.
pub const SERVER_SAVE_FORMAT_VERSION: u16 = 1;

/// Suffix appended to the save path for the previous valid save.
pub const SAVE_BACKUP_SUFFIX: &str = ".bak";

/// Suffix appended to the save path for the in-flight temp file.
pub const SAVE_TEMP_SUFFIX: &str = ".tmp";

/// Stable save error codes. These are API contracts; do not rename them.
pub mod codes {
    /// A filesystem operation failed.
    pub const IO: &str = "save.io";
    /// The document is not UTF-8 or not valid JSON/schema.
    pub const INVALID_JSON: &str = "save.invalid_json";
    /// The save format version is not supported.
    pub const UNSUPPORTED_VERSION: &str = "save.unsupported_version";
    /// The economy or road store seed does not match the world seed.
    pub const SEED_MISMATCH: &str = "save.seed_mismatch";
    /// Population, employment, or building counts are inconsistent.
    pub const INVALID_COUNTS: &str = "save.invalid_counts";
    /// The road store failed validation.
    pub const INVALID_ROADS: &str = "save.invalid_roads";
    /// The economy failed validation.
    pub const INVALID_ECONOMY: &str = "save.invalid_economy";
    /// The placement store failed validation.
    pub const INVALID_PLACEMENTS: &str = "save.invalid_placements";
    /// The construction store failed validation.
    pub const INVALID_CONSTRUCTION: &str = "save.invalid_construction";
    /// The player registry failed validation.
    pub const INVALID_PLAYERS: &str = "save.invalid_players";
    /// The settlement store failed validation.
    pub const INVALID_SETTLEMENTS: &str = "save.invalid_settlements";
    /// Neither the primary save nor its backup exists.
    pub const NOT_FOUND: &str = "save.not_found";
    /// The primary and its backup are both unusable.
    pub const CORRUPT: &str = "save.corrupt";
}

/// On-disk server save, format version 1.
///
/// The struct is deliberately a plain data record: the economy and road store
/// keep their own versioned JSON shapes, so this file layer only adds the
/// session-level fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerSave {
    /// Save format version; currently [`SERVER_SAVE_FORMAT_VERSION`].
    pub format_version: u16,
    /// World seed.
    pub seed: u64,
    /// Authoritative command/tick counter.
    pub tick: u64,
    /// Static population count.
    pub population: u32,
    /// Static employed-citizen count.
    pub employed: u32,
    /// Static completed-building count.
    pub buildings: u32,
    /// Whether `advance_day` is frozen.
    pub paused: bool,
    /// Serialized treasury.
    pub economy: Economy,
    /// Serialized road network.
    pub roads: RoadStore,
    /// Authoritative blueprint placements (D-032).
    ///
    /// Omitted when the world has no placement store, so a world that never
    /// placed a blueprint writes byte-identical JSON to a pre-D-032 save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placements: Option<PlacementStore>,
    /// Authoritative construction sites (D-023/D-028).
    ///
    /// Omitted when the world has no construction store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub construction: Option<ConstructionStore>,
    /// Save-local player registry (D-033).
    ///
    /// Omitted while the registry is empty, so a world without players writes
    /// byte-identical JSON to a pre-D-033 save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub players: Option<PlayerRegistry>,
    /// Optional settlement layer (D-019/D-033).
    ///
    /// Omitted when the world has no settlement store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlements: Option<SettlementStore>,
}

impl ServerSave {
    /// Validate a decoded save before it is turned back into a world.
    pub fn validate(&self) -> Result<(), SaveError> {
        if self.format_version != SERVER_SAVE_FORMAT_VERSION {
            return Err(SaveError::UnsupportedVersion(self.format_version));
        }
        if self.roads.seed != self.seed {
            return Err(SaveError::SeedMismatch {
                field: "roads",
                expected: self.seed,
                found: self.roads.seed,
            });
        }
        if self.economy.seed != self.seed {
            return Err(SaveError::SeedMismatch {
                field: "economy",
                expected: self.seed,
                found: self.economy.seed,
            });
        }
        if self.population == 0 {
            return Err(SaveError::InvalidCounts("population must be at least 1"));
        }
        if self.employed > self.population {
            return Err(SaveError::InvalidCounts(
                "employed must not exceed population",
            ));
        }
        if self.buildings == 0 {
            return Err(SaveError::InvalidCounts("buildings must be at least 1"));
        }
        self.roads
            .validate_loaded()
            .map_err(SaveError::InvalidRoads)?;
        self.economy.validate().map_err(SaveError::InvalidEconomy)?;
        if let Some(placements) = &self.placements {
            if placements.seed != self.seed {
                return Err(SaveError::SeedMismatch {
                    field: "placements",
                    expected: self.seed,
                    found: placements.seed,
                });
            }
            placements
                .validate_loaded()
                .map_err(SaveError::InvalidPlacements)?;
        }
        if let Some(construction) = &self.construction {
            construction
                .validate()
                .map_err(SaveError::InvalidConstruction)?;
        }
        if let Some(players) = &self.players {
            if players.seed != self.seed {
                return Err(SaveError::SeedMismatch {
                    field: "players",
                    expected: self.seed,
                    found: players.seed,
                });
            }
            players
                .validate_loaded()
                .map_err(SaveError::InvalidPlayers)?;
        }
        if let Some(settlements) = &self.settlements {
            if settlements.seed != self.seed {
                return Err(SaveError::SeedMismatch {
                    field: "settlements",
                    expected: self.seed,
                    found: settlements.seed,
                });
            }
            settlements
                .validate_loaded()
                .map_err(SaveError::InvalidSettlements)?;
        }
        Ok(())
    }
}

/// Result of a save or load operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveReport {
    /// Size of the save document in bytes.
    pub bytes: u64,
    /// Whether the previous primary was rotated to `<path>.bak`.
    pub backup_rotated: bool,
    /// Whether [`ServerWorld::load_from`] used the backup.
    pub recovered_from_backup: bool,
}

/// A world restored from disk.
#[derive(Debug, Clone)]
pub struct LoadedWorld {
    /// Restored world. The session starts open and without an accepted hello.
    pub world: ServerWorld,
    /// Load metadata.
    pub report: SaveReport,
}

/// A save/load failure with a stable [`Self::code`].
#[derive(Debug)]
pub enum SaveError {
    /// Filesystem error.
    Io(io::Error),
    /// The document is not UTF-8, not JSON, or does not match the schema.
    InvalidJson(String),
    /// Unsupported save format version.
    UnsupportedVersion(u16),
    /// A nested store belongs to a different seed.
    SeedMismatch {
        /// Store name (`roads` or `economy`).
        field: &'static str,
        /// World seed.
        expected: u64,
        /// Seed found in the store.
        found: u64,
    },
    /// Population, employment, or building counts are inconsistent.
    InvalidCounts(&'static str),
    /// The road store failed validation.
    InvalidRoads(RoadError),
    /// The economy failed validation.
    InvalidEconomy(EconomyError),
    /// The placement store failed validation.
    InvalidPlacements(PlacementError),
    /// The construction store failed validation.
    InvalidConstruction(ConstructionError),
    /// The player registry failed validation.
    InvalidPlayers(PlayerError),
    /// The settlement store failed validation.
    InvalidSettlements(SettlementError),
    /// Neither the primary nor the backup exists.
    NotFound(PathBuf),
    /// The primary and its backup are both unusable.
    Corrupt {
        /// Primary path.
        path: PathBuf,
        /// Primary and backup failure details.
        reason: String,
    },
}

impl SaveError {
    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => codes::IO,
            Self::InvalidJson(_) => codes::INVALID_JSON,
            Self::UnsupportedVersion(_) => codes::UNSUPPORTED_VERSION,
            Self::SeedMismatch { .. } => codes::SEED_MISMATCH,
            Self::InvalidCounts(_) => codes::INVALID_COUNTS,
            Self::InvalidRoads(_) => codes::INVALID_ROADS,
            Self::InvalidEconomy(_) => codes::INVALID_ECONOMY,
            Self::InvalidPlacements(_) => codes::INVALID_PLACEMENTS,
            Self::InvalidConstruction(_) => codes::INVALID_CONSTRUCTION,
            Self::InvalidPlayers(_) => codes::INVALID_PLAYERS,
            Self::InvalidSettlements(_) => codes::INVALID_SETTLEMENTS,
            Self::NotFound(_) => codes::NOT_FOUND,
            Self::Corrupt { .. } => codes::CORRUPT,
        }
    }

    fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound(_))
    }
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "save I/O error: {error}"),
            Self::InvalidJson(message) => write!(formatter, "invalid save JSON: {message}"),
            Self::UnsupportedVersion(version) => write!(
                formatter,
                "unsupported server save format version {version}; expected {SERVER_SAVE_FORMAT_VERSION}"
            ),
            Self::SeedMismatch {
                field,
                expected,
                found,
            } => write!(
                formatter,
                "{field} seed {found} does not match world seed {expected}"
            ),
            Self::InvalidCounts(message) => write!(formatter, "invalid save counts: {message}"),
            Self::InvalidRoads(error) => write!(formatter, "invalid saved roads: {error}"),
            Self::InvalidEconomy(error) => write!(formatter, "invalid saved economy: {error}"),
            Self::InvalidPlacements(error) => write!(formatter, "invalid saved placements: {error}"),
            Self::InvalidConstruction(error) => {
                write!(formatter, "invalid saved construction: {error}")
            }
            Self::InvalidPlayers(error) => write!(formatter, "invalid saved players: {error}"),
            Self::InvalidSettlements(error) => {
                write!(formatter, "invalid saved settlements: {error}")
            }
            Self::NotFound(path) => {
                write!(formatter, "no save or backup at {}", path.display())
            }
            Self::Corrupt { path, reason } => {
                write!(formatter, "save {} is corrupt: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for SaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidRoads(error) => Some(error),
            Self::InvalidEconomy(error) => Some(error),
            Self::InvalidPlacements(error) => Some(error),
            Self::InvalidConstruction(error) => Some(error),
            Self::InvalidPlayers(error) => Some(error),
            Self::InvalidSettlements(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for SaveError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for SaveError {
    fn from(error: serde_json::Error) -> Self {
        Self::InvalidJson(error.to_string())
    }
}

impl ServerWorld {
    /// Persistable record for the current state.
    #[must_use]
    pub fn to_save(&self) -> ServerSave {
        ServerSave {
            format_version: SERVER_SAVE_FORMAT_VERSION,
            seed: self.seed,
            tick: self.tick,
            population: self.population,
            employed: self.employed,
            buildings: self.buildings,
            paused: self.paused,
            economy: self.economy.clone(),
            roads: self.roads.clone(),
            placements: self.placements.clone(),
            construction: self.construction.clone(),
            players: (!self.players.is_empty()).then(|| self.players.clone()),
            settlements: self.settlements.clone(),
        }
    }

    /// Serialize the current state as validated pretty JSON.
    pub fn to_save_json_pretty(&self) -> Result<String, SaveError> {
        let save = self.to_save();
        save.validate()?;
        Ok(serde_json::to_string_pretty(&save)?)
    }

    /// Parse and validate a save document, returning a fresh session world.
    ///
    /// The loaded world has no accepted hello and is not closed, so a client
    /// must handshake again. The tick, paused flag, treasury, and roads resume
    /// exactly as saved.
    pub fn from_save_json(json: &str) -> Result<Self, SaveError> {
        let save: ServerSave = serde_json::from_str(json)?;
        save.validate()?;
        Ok(Self::restore(save))
    }

    /// Atomically save to `path`, rotating the previous valid primary to
    /// `<path>.bak`.
    ///
    /// The new document is written to `<path>.tmp`, synced, and renamed. A
    /// corrupt existing primary is not copied over a good backup.
    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<SaveReport, SaveError> {
        let path = path.as_ref();
        let json = self.to_save_json_pretty()?;
        let bytes = json.as_bytes();

        let mut backup_rotated = false;
        if path.is_file() {
            let existing = fs::read(path)?;
            if existing != bytes && is_valid_save(&existing) {
                write_atomic(&backup_path(path), &existing)?;
                backup_rotated = true;
            }
        }

        write_atomic(path, bytes)?;
        Ok(SaveReport {
            bytes: bytes.len() as u64,
            backup_rotated,
            recovered_from_backup: false,
        })
    }

    /// Load a world from `path`, falling back to `<path>.bak` when the primary
    /// is missing or corrupt.
    pub fn load_from(path: impl AsRef<Path>) -> Result<LoadedWorld, SaveError> {
        let path = path.as_ref();
        let backup = backup_path(path);

        let primary = read_save(path);
        match primary {
            Ok((world, bytes)) => {
                return Ok(LoadedWorld {
                    world,
                    report: SaveReport {
                        bytes,
                        backup_rotated: false,
                        recovered_from_backup: false,
                    },
                });
            }
            Err(error) if error.is_not_found() && !backup.is_file() => return Err(error),
            Err(_) => {}
        }

        let primary_error = primary.expect_err("primary load failed");
        match read_save(&backup) {
            Ok((world, bytes)) => Ok(LoadedWorld {
                world,
                report: SaveReport {
                    bytes,
                    backup_rotated: false,
                    recovered_from_backup: true,
                },
            }),
            Err(backup_error) => {
                if primary_error.is_not_found() && backup_error.is_not_found() {
                    Err(SaveError::NotFound(path.to_path_buf()))
                } else {
                    Err(SaveError::Corrupt {
                        path: path.to_path_buf(),
                        reason: format!("primary: {primary_error}; backup: {backup_error}"),
                    })
                }
            }
        }
    }
}

/// Path of the backup for a save path.
#[must_use]
pub fn backup_path(path: &Path) -> PathBuf {
    appended(path, SAVE_BACKUP_SUFFIX)
}

/// Path of the in-flight temp file for a save path.
#[must_use]
pub fn temp_path(path: &Path) -> PathBuf {
    appended(path, SAVE_TEMP_SUFFIX)
}

fn appended(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn is_valid_save(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| ServerWorld::from_save_json(text).ok())
        .is_some()
}

fn read_save(path: &Path) -> Result<(ServerWorld, u64), SaveError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(SaveError::NotFound(path.to_path_buf()));
        }
        Err(error) => return Err(SaveError::Io(error)),
    };
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        SaveError::InvalidJson(format!(
            "not valid UTF-8 at byte {}: {error}",
            error.valid_up_to()
        ))
    })?;
    let world = ServerWorld::from_save_json(text)?;
    Ok((world, bytes.len() as u64))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), SaveError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let temp = temp_path(path);
    let result = write_temp_and_rename(path, &temp, bytes);
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn write_temp_and_rename(path: &Path, temp: &Path, bytes: &[u8]) -> Result<(), SaveError> {
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(temp, path)?;
    sync_parent(path);
    Ok(())
}

/// Best-effort parent directory sync. Opening a directory is not supported on
/// every platform; a failure here does not undo the rename.
fn sync_parent(path: &Path) {
    let Some(parent) = path.parent() else {
        return;
    };
    let directory = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    if let Ok(handle) = fs::File::open(directory) {
        let _ = handle.sync_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_city::economy::DAY_SECONDS;
    use core_city::placement::PlacementRequest;

    use crate::protocol::{ClientCommand, ServerMessage, TimeAction, LOOPBACK_SEED};

    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "townsmith-server-save-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn hello() -> ClientCommand {
        ClientCommand::Hello {
            protocol_version: crate::protocol::PROTOCOL_VERSION,
            content_hash: "0123456789abcdef".repeat(4),
            player_name: "mayor".to_string(),
        }
    }

    fn played_world() -> ServerWorld {
        let mut world = ServerWorld::with_role(LOOPBACK_SEED, crate::world::ClientRole::Admin);
        world.apply(&hello());
        world.apply(&ClientCommand::SetTax {
            resident_rate: 12,
            business_rate: 8,
        });
        world.apply(&ClientCommand::PlaceRoad {
            start: [0, 0],
            end: [8, 0],
        });
        world.apply(&ClientCommand::TimeControl {
            action: TimeAction::Pause,
        });
        world
    }

    fn save_json(value: &serde_json::Value) -> String {
        serde_json::to_string_pretty(value).expect("json")
    }

    #[test]
    fn save_round_trip_preserves_state_and_checksum() {
        let world = played_world();
        let json = world.to_save_json_pretty().expect("json");
        let loaded = ServerWorld::from_save_json(&json).expect("load");

        assert_eq!(loaded.to_save(), world.to_save());
        assert_eq!(loaded.checksum(), world.checksum());
        assert_eq!(loaded.tick, 2);
        assert_eq!(loaded.economy.days_elapsed, 0.0);
        assert_eq!(loaded.economy.tax_per_resident_per_day, 12.0);
        assert_eq!(loaded.roads.roads.len(), 1);
        assert!(loaded.paused);
        assert_eq!(loaded.session_id(), world.session_id());
        assert!(!loaded.hello_accepted());
        assert!(!loaded.is_closed());
    }

    #[test]
    fn save_round_trip_preserves_players_and_settlements() {
        let mut world = played_world();
        let player = world.player_id().expect("hello assigns a player id");
        let mut settlements = SettlementStore::new(LOOPBACK_SEED);
        settlements
            .found_for_player(
                &core_city::settlement::SettlementRequest {
                    name: "Round Trip".to_string(),
                    owner: "mayor".to_string(),
                    center: [0, 0],
                    radius_m: 64,
                },
                player,
            )
            .expect("found settlement");
        world.settlements = Some(settlements);

        let json = world.to_save_json_pretty().expect("json");
        let loaded = ServerWorld::from_save_json(&json).expect("load");

        assert_eq!(loaded.players, world.players);
        assert_eq!(loaded.settlements, world.settlements);
        assert_eq!(loaded.checksum(), world.checksum());
    }

    #[test]
    fn empty_players_and_settlements_are_omitted_and_byte_identical() {
        let json = ServerWorld::new(LOOPBACK_SEED)
            .to_save_json_pretty()
            .expect("json");
        let value: serde_json::Value = serde_json::from_str(&json).expect("value");
        assert!(value.get("players").is_none());
        assert!(value.get("settlements").is_none());

        let loaded = ServerWorld::from_save_json(&json).expect("load");
        assert_eq!(loaded.to_save_json_pretty().expect("re-save"), json);
    }

    #[test]
    fn foreign_seed_players_and_settlements_are_rejected() {
        for field in ["players", "settlements"] {
            let mut value: serde_json::Value = serde_json::from_str(
                &ServerWorld::new(LOOPBACK_SEED)
                    .to_save_json_pretty()
                    .expect("json"),
            )
            .expect("value");
            let foreign = LOOPBACK_SEED + 1;
            value[field] = match field {
                "players" => serde_json::to_value(PlayerRegistry::new(foreign)).expect("players"),
                _ => serde_json::to_value(SettlementStore::new(foreign)).expect("settlements"),
            };
            let error = ServerWorld::from_save_json(&save_json(&value)).expect_err("foreign seed");
            assert_eq!(error.code(), codes::SEED_MISMATCH, "{field}");
        }
    }

    #[test]
    fn save_json_has_the_v1_shape() {
        let json = ServerWorld::new(LOOPBACK_SEED)
            .to_save_json_pretty()
            .expect("json");
        let value: serde_json::Value = serde_json::from_str(&json).expect("value");
        assert_eq!(value["format_version"], SERVER_SAVE_FORMAT_VERSION);
        assert_eq!(value["seed"], LOOPBACK_SEED);
        assert_eq!(value["tick"], 0);
        assert!(value["economy"].is_object());
        assert!(value["roads"].is_object());
        assert!(json.contains('\n'), "save uses pretty JSON");
    }

    #[test]
    fn old_and_unsupported_versions_are_rejected() {
        let mut value: serde_json::Value = serde_json::from_str(
            &ServerWorld::new(LOOPBACK_SEED)
                .to_save_json_pretty()
                .unwrap(),
        )
        .unwrap();
        for version in [0, SERVER_SAVE_FORMAT_VERSION + 1] {
            value["format_version"] = version.into();
            let error =
                ServerWorld::from_save_json(&save_json(&value)).expect_err("unsupported version");
            assert_eq!(error.code(), codes::UNSUPPORTED_VERSION);
        }
    }

    #[test]
    fn seed_mismatches_are_rejected_for_both_stores() {
        let mut value: serde_json::Value = serde_json::from_str(
            &ServerWorld::new(LOOPBACK_SEED)
                .to_save_json_pretty()
                .unwrap(),
        )
        .unwrap();

        value["roads"]["seed"] = (LOOPBACK_SEED + 1).into();
        let error = ServerWorld::from_save_json(&save_json(&value)).expect_err("road seed");
        assert_eq!(error.code(), codes::SEED_MISMATCH);

        value["roads"]["seed"] = LOOPBACK_SEED.into();
        value["economy"]["seed"] = (LOOPBACK_SEED + 2).into();
        let error = ServerWorld::from_save_json(&save_json(&value)).expect_err("economy seed");
        assert_eq!(error.code(), codes::SEED_MISMATCH);
    }

    #[test]
    fn invalid_counts_are_rejected() {
        let mut value: serde_json::Value = serde_json::from_str(
            &ServerWorld::new(LOOPBACK_SEED)
                .to_save_json_pretty()
                .unwrap(),
        )
        .unwrap();

        value["population"] = 0.into();
        assert_eq!(
            ServerWorld::from_save_json(&save_json(&value))
                .expect_err("population")
                .code(),
            codes::INVALID_COUNTS
        );

        value["population"] = 100.into();
        value["employed"] = 101.into();
        assert_eq!(
            ServerWorld::from_save_json(&save_json(&value))
                .expect_err("employed")
                .code(),
            codes::INVALID_COUNTS
        );

        value["employed"] = 60.into();
        value["buildings"] = 0.into();
        assert_eq!(
            ServerWorld::from_save_json(&save_json(&value))
                .expect_err("buildings")
                .code(),
            codes::INVALID_COUNTS
        );
    }

    #[test]
    fn road_store_validation_runs_on_load() {
        let mut world = played_world();
        world.roads.next_id = 1;
        let json = serde_json::to_string_pretty(&world.to_save()).expect("json");
        let error = ServerWorld::from_save_json(&json).expect_err("roads");
        assert_eq!(error.code(), codes::INVALID_ROADS);
    }

    #[test]
    fn economy_validation_runs_on_load() {
        let mut world = played_world();
        world.economy.tax_per_resident_per_day = -1.0;
        let json = serde_json::to_string_pretty(&world.to_save()).expect("json");
        let error = ServerWorld::from_save_json(&json).expect_err("economy");
        assert_eq!(error.code(), codes::INVALID_ECONOMY);
    }

    #[test]
    fn invalid_json_is_rejected() {
        assert_eq!(
            ServerWorld::from_save_json("{").expect_err("json").code(),
            codes::INVALID_JSON
        );
        assert_eq!(
            ServerWorld::from_save_json(r#"{"format_version":1}"#)
                .expect_err("schema")
                .code(),
            codes::INVALID_JSON
        );
    }

    #[test]
    fn atomic_save_leaves_no_temp_and_rotates_backup() {
        let dir = TempDir::new();
        let path = dir.join("world.json");

        let mut world = ServerWorld::new(LOOPBACK_SEED);
        let first = world.save_to(&path).expect("first save");
        assert!(!first.backup_rotated);
        assert!(first.bytes > 0);
        assert!(path.is_file());
        assert!(!temp_path(&path).exists());
        assert!(!backup_path(&path).exists());

        world.advance_day(DAY_SECONDS);
        let second = world.save_to(&path).expect("second save");
        assert!(second.backup_rotated);
        assert!(!temp_path(&path).exists());
        assert!(backup_path(&path).is_file());

        let backup = ServerWorld::load_from(backup_path(&path)).expect("backup load");
        assert_eq!(backup.world.tick, 0);
    }

    #[test]
    fn corrupt_primary_recovers_the_backup() {
        let dir = TempDir::new();
        let path = dir.join("world.json");

        let mut world = ServerWorld::new(LOOPBACK_SEED);
        world.save_to(&path).expect("first save");
        world.advance_day(DAY_SECONDS);
        world.save_to(&path).expect("second save");
        assert_eq!(
            ServerWorld::load_from(&path).expect("primary").world.tick,
            1
        );

        fs::write(&path, b"{ not json").expect("corrupt primary");
        let loaded = ServerWorld::load_from(&path).expect("recover");
        assert!(loaded.report.recovered_from_backup);
        assert_eq!(loaded.world.tick, 0);
        assert!(!temp_path(&path).exists());
    }

    #[test]
    fn both_corrupt_saves_are_rejected() {
        let dir = TempDir::new();
        let path = dir.join("world.json");
        ServerWorld::new(LOOPBACK_SEED)
            .save_to(&path)
            .expect("save");
        fs::write(&path, b"{").expect("corrupt primary");
        fs::write(backup_path(&path), b"[]").expect("corrupt backup");

        let error = ServerWorld::load_from(&path).expect_err("both corrupt");
        assert_eq!(error.code(), codes::CORRUPT);
        assert!(error.to_string().contains("primary"));
    }

    #[test]
    fn missing_saves_are_rejected() {
        let dir = TempDir::new();
        let error = ServerWorld::load_from(dir.join("absent.json")).expect_err("missing");
        assert_eq!(error.code(), codes::NOT_FOUND);
    }

    #[test]
    fn a_corrupt_primary_does_not_clobber_a_good_backup() {
        let dir = TempDir::new();
        let path = dir.join("world.json");

        let mut world = ServerWorld::new(LOOPBACK_SEED);
        world.save_to(&path).expect("first save");
        world.advance_day(DAY_SECONDS);
        world.save_to(&path).expect("second save");
        assert_eq!(
            ServerWorld::load_from(backup_path(&path))
                .unwrap()
                .world
                .tick,
            0
        );

        fs::write(&path, b"{").expect("corrupt primary");
        world.advance_day(DAY_SECONDS);
        let report = world.save_to(&path).expect("save over corrupt primary");
        assert!(!report.backup_rotated);

        let backup = ServerWorld::load_from(backup_path(&path)).expect("backup");
        assert_eq!(backup.world.tick, 0);
        assert_eq!(
            ServerWorld::load_from(&path).expect("primary").world.tick,
            2
        );
    }

    #[test]
    fn save_after_commands_then_load_resumes_the_tick() {
        let dir = TempDir::new();
        let path = dir.join("world.json");
        let world = played_world();
        world.save_to(&path).expect("save");

        let mut loaded = ServerWorld::load_from(&path).expect("load").world;
        assert_eq!(loaded.tick, 2);
        assert!(matches!(
            loaded.apply(&hello()),
            ServerMessage::Welcome { tick: 2, .. }
        ));
        let snapshot = loaded.apply(&ClientCommand::SetTax {
            resident_rate: 20,
            business_rate: 5,
        });
        match snapshot {
            ServerMessage::Snapshot {
                tick,
                population,
                balance,
                checksum,
                ..
            } => {
                assert_eq!(tick, 3);
                assert_eq!(population, world.population);
                assert_eq!(balance, world.balance_units());
                assert_eq!(checksum, loaded.checksum());
                assert_eq!(checksum.len(), 16);
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
        assert_eq!(loaded.roads.roads.len(), 1);
        assert_eq!(loaded.economy.tax_per_resident_per_day, 20.0);
    }

    #[test]
    fn paused_state_round_trips() {
        let mut world = ServerWorld::new(LOOPBACK_SEED);
        world.paused = true;
        let json = world.to_save_json_pretty().expect("json");
        let loaded = ServerWorld::from_save_json(&json).expect("load");
        assert!(loaded.paused);
        assert_eq!(loaded.checksum(), world.checksum());
    }

    fn world_with_placement_and_site() -> ServerWorld {
        let hash = format!("blake3:{}", "ab".repeat(32));
        let request = PlacementRequest {
            blueprint_id: "townsmith:save_house".to_string(),
            blueprint_hash: hash.clone(),
            blueprint_path: format!("tsbp:blake3:{}", "ab".repeat(32)),
            name: "Save House".to_string(),
            bounds_m: [3.2, 3.2, 3.2],
            center_x: 0.0,
            center_z: 0.0,
            rotation_deg: 0,
            material_cost: 100.0,
            resident_capacity: 2,
            job_slots: 1,
            solid_voxels: 10,
            structure: None,
        };
        let mut placements = PlacementStore::new(LOOPBACK_SEED);
        placements
            .place(&request, |_, _| 40)
            .expect("placement record");
        let mut construction = ConstructionStore::default();
        construction
            .add_priced_site(1, 10, 100.0)
            .expect("construction site");

        let mut world = ServerWorld::new(LOOPBACK_SEED);
        world.placements = Some(placements);
        world.construction = Some(construction);
        world
    }

    #[test]
    fn save_omits_placement_stores_when_absent() {
        let json = ServerWorld::new(LOOPBACK_SEED)
            .to_save_json_pretty()
            .expect("json");
        assert!(!json.contains("\"placements\""));
        assert!(!json.contains("\"construction\""));
        let loaded = ServerWorld::from_save_json(&json).expect("load");
        assert!(loaded.placements.is_none());
        assert!(loaded.construction.is_none());
        assert!(loaded.blueprint_library.is_none());
        assert!(loaded.policies.is_none());
    }

    #[test]
    fn placement_and_construction_round_trip() {
        let world = world_with_placement_and_site();
        let json = world.to_save_json_pretty().expect("json");
        let value: serde_json::Value = serde_json::from_str(&json).expect("value");
        assert!(value["placements"].is_object());
        assert!(value["construction"].is_object());
        assert!(!json.contains("/tmp"));
        assert!(!json.contains("/media"));
        let stored = value["placements"]["placements"][0]["blueprint_path"]
            .as_str()
            .expect("stored reference");
        assert!(stored.starts_with("tsbp:blake3:"));
        assert!(!stored.contains('/'));

        let loaded = ServerWorld::from_save_json(&json).expect("load");
        assert_eq!(loaded.checksum(), world.checksum());
        assert_eq!(loaded.placements, world.placements);
        assert_eq!(loaded.construction, world.construction);
        assert!(loaded.blueprint_library.is_none());
        assert!(loaded.policies.is_none());
    }

    #[test]
    fn placement_and_construction_file_round_trip() {
        let dir = TempDir::new();
        let path = dir.join("world.json");
        let world = world_with_placement_and_site();
        world.save_to(&path).expect("save");
        let loaded = ServerWorld::load_from(&path).expect("load");
        assert_eq!(loaded.world.checksum(), world.checksum());
        assert_eq!(
            loaded
                .world
                .placements
                .as_ref()
                .map(|store| store.placements.len()),
            Some(1)
        );
        assert_eq!(
            loaded
                .world
                .construction
                .as_ref()
                .map(|store| store.site_count()),
            Some(1)
        );
    }

    #[test]
    fn placement_seed_mismatch_is_rejected() {
        let mut value: serde_json::Value = serde_json::from_str(
            &world_with_placement_and_site()
                .to_save_json_pretty()
                .unwrap(),
        )
        .unwrap();
        value["placements"]["seed"] = (LOOPBACK_SEED + 1).into();
        let error = ServerWorld::from_save_json(&save_json(&value)).expect_err("placement seed");
        assert_eq!(error.code(), codes::SEED_MISMATCH);
    }

    #[test]
    fn invalid_placement_store_is_rejected() {
        let mut value: serde_json::Value = serde_json::from_str(
            &world_with_placement_and_site()
                .to_save_json_pretty()
                .unwrap(),
        )
        .unwrap();
        value["placements"]["next_id"] = 1.into();
        let error = ServerWorld::from_save_json(&save_json(&value)).expect_err("placement store");
        assert_eq!(error.code(), codes::INVALID_PLACEMENTS);
    }

    #[test]
    fn invalid_construction_store_is_rejected() {
        let mut value: serde_json::Value = serde_json::from_str(
            &world_with_placement_and_site()
                .to_save_json_pretty()
                .unwrap(),
        )
        .unwrap();
        value["construction"]["crew_capacity"] = 65.into();
        let error =
            ServerWorld::from_save_json(&save_json(&value)).expect_err("construction store");
        assert_eq!(error.code(), codes::INVALID_CONSTRUCTION);
    }
}
