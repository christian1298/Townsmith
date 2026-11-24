//! Soft settlement boundaries, ownership, and membership.
//!
//! A settlement is a player-named circle on the terrain. Boundaries are soft
//! and may overlap; the nearest containing settlement owns a point. This slice
//! stores identity, ownership, roles, and permissions. District boundaries,
//! taxes, policies, service coverage, and global server roles are later work.
//!
//! # Save versions
//!
//! [`SETTLEMENT_FORMAT_VERSION_V1`] is the legacy identity-only format. A store
//! created by [`SettlementStore::new`] stays v1 and serializes exactly as it did
//! before membership existed. The first owned or membership mutation upgrades
//! the store to [`SETTLEMENT_FORMAT_VERSION`] (v2), which adds the optional
//! `owner_id`, `members`, and `membership_revision` fields. Both versions load;
//! a v2 store with no membership data normalizes to v1 on load and keeps the
//! v1 checksum.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::player::{PlayerId, PlayerRegistry};

/// Legacy settlement save format: identity and a display-only owner label.
pub const SETTLEMENT_FORMAT_VERSION_V1: u16 = 1;

/// Current settlement save format: adds ownership and membership.
pub const SETTLEMENT_FORMAT_VERSION: u16 = 2;

/// Hard cap for members per settlement, including the owner.
pub const MAX_SETTLEMENT_MEMBERS: usize = 64;

/// Hard cap for settlements owned by one player.
pub const MAX_SETTLEMENTS_OWNED_PER_PLAYER: usize = 16;

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

/// Settlement permission bits.
///
/// The save stores a [`SettlementRole`], never a second copy of these bits, so
/// a role preset and the effective permissions cannot drift. Hand-rolled to
/// avoid a new dependency: a thin `u8` wrapper with `contains` and `union`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SettlementPermission(u8);

impl SettlementPermission {
    /// No permission.
    pub const NONE: Self = Self(0);
    /// Place/remove blueprints, construction sites, and live voxel edits.
    pub const BUILD: Self = Self(1 << 0);
    /// Rename, edit metadata, and initiate a transfer.
    pub const MANAGE: Self = Self(1 << 1);
    /// Create, update, and remove district policies and settlement tax rates.
    pub const TAX: Self = Self(1 << 2);
    /// Invite, remove, and change the role of members.
    pub const MEMBERS: Self = Self(1 << 3);
    /// Every permission bit.
    pub const ALL: Self = Self(0b1111);

    /// Raw permission bits.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Wrap raw bits, rejecting unknown bits.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !Self::ALL.0 == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    /// Whether every bit in `other` is set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Combine two permission sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether no bit is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Settlement role preset.
///
/// [`SettlementRole::Owner`] is implicit [`SettlementPermission::ALL`]; the
/// other presets map to a fixed permission set. A custom per-member override is
/// not part of v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettlementRole {
    /// The single owner: every permission, cannot be removed or demoted.
    Owner,
    /// Full settlement management below the owner.
    Manager,
    /// Construction and placement only.
    Builder,
    /// Read-only participation; no settlement mutation.
    Member,
}

impl SettlementRole {
    /// Every role, in descending authority order.
    pub const ALL: [Self; 4] = [Self::Owner, Self::Manager, Self::Builder, Self::Member];

    /// Stable lowercase name used by the save and diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Manager => "manager",
            Self::Builder => "builder",
            Self::Member => "member",
        }
    }

    /// Permission bits granted by this role.
    #[must_use]
    pub const fn permissions(self) -> SettlementPermission {
        match self {
            Self::Owner => SettlementPermission::ALL,
            Self::Manager => SettlementPermission::BUILD
                .union(SettlementPermission::MANAGE)
                .union(SettlementPermission::TAX)
                .union(SettlementPermission::MEMBERS),
            Self::Builder => SettlementPermission::BUILD,
            Self::Member => SettlementPermission::NONE,
        }
    }

    /// Whether this is the owner role.
    #[must_use]
    pub const fn is_owner(self) -> bool {
        matches!(self, Self::Owner)
    }
}

impl fmt::Display for SettlementRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One settlement member and their role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementMember {
    /// Stable player identity.
    pub player: PlayerId,
    /// Role preset; the save stores the role, not the permission bits.
    pub role: SettlementRole,
}

/// One named settlement and its soft boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settlement {
    /// Stable id within the world.
    pub id: u64,
    /// Player-facing name.
    pub name: String,
    /// Legacy owner display label, group, or server role.
    ///
    /// This field is never identity. When `owner_id` is set, that id is
    /// authoritative and this string is only a cached display label.
    pub owner: String,
    /// Centre `[x, z]` in metres.
    pub center: [i32; 2],
    /// Soft boundary radius in metres.
    pub radius_m: u32,
    /// Authoritative owner identity, when the settlement is owned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<PlayerId>,
    /// Members in ascending player-id order; includes the owner when owned.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<SettlementMember>,
    /// Optimistic-concurrency revision for membership and ownership edits.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub membership_revision: u32,
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

    /// Whether the settlement has an owner.
    #[must_use]
    pub fn is_owned(&self) -> bool {
        self.owner_id.is_some()
    }

    /// Whether this player is the owner.
    #[must_use]
    pub fn is_owner(&self, player: PlayerId) -> bool {
        self.owner_id == Some(player)
    }

    /// Role of a member, if any.
    #[must_use]
    pub fn role_of(&self, player: PlayerId) -> Option<SettlementRole> {
        self.members
            .iter()
            .find(|member| member.player == player)
            .map(|member| member.role)
    }

    /// Effective permission bits for a player.
    ///
    /// The owner always receives every bit, even if a malformed record omits
    /// the owner from `members`. A non-member receives none.
    #[must_use]
    pub fn effective_permissions(&self, player: PlayerId) -> SettlementPermission {
        if self.is_owner(player) {
            return SettlementPermission::ALL;
        }
        self.role_of(player)
            .map_or(SettlementPermission::NONE, SettlementRole::permissions)
    }

    /// Whether a player holds every bit in `permission`.
    #[must_use]
    pub fn has_permission(&self, player: PlayerId, permission: SettlementPermission) -> bool {
        self.effective_permissions(player).contains(permission)
    }

    /// Effective permissions for the current owner, or none when unowned.
    #[must_use]
    pub fn owner_permissions(&self) -> SettlementPermission {
        self.owner_id
            .map_or(SettlementPermission::NONE, |_| SettlementPermission::ALL)
    }

    /// Add a member. The owner is added by [`Self::assign_owner`], never here.
    ///
    /// # Errors
    ///
    /// Rejects an unowned settlement, the owner role, a duplicate player, and a
    /// full member list. On success the membership revision increments.
    pub fn insert_member(
        &mut self,
        player: PlayerId,
        role: SettlementRole,
    ) -> Result<(), SettlementError> {
        if self.owner_id.is_none() {
            return Err(SettlementError::Unowned { id: self.id });
        }
        if role.is_owner() {
            return Err(SettlementError::InvalidRecord {
                id: self.id,
                reason: "owner role requires a transfer",
            });
        }
        if self.members.len() >= MAX_SETTLEMENT_MEMBERS {
            return Err(SettlementError::TooManyMembers {
                limit: MAX_SETTLEMENT_MEMBERS,
            });
        }
        if self.role_of(player).is_some() {
            return Err(SettlementError::InvalidRecord {
                id: self.id,
                reason: "player is already a member",
            });
        }
        self.insert_sorted(SettlementMember { player, role });
        self.bump_revision();
        Ok(())
    }

    /// Remove a member and return the removed record.
    ///
    /// # Errors
    ///
    /// The owner cannot be removed ([`SettlementError::OwnerRequired`]); an
    /// unknown player returns [`SettlementError::NotMember`]. On success the
    /// membership revision increments.
    pub fn remove_member(&mut self, player: PlayerId) -> Result<SettlementMember, SettlementError> {
        if self.is_owner(player) {
            return Err(SettlementError::OwnerRequired { id: self.id });
        }
        let index = self
            .members
            .iter()
            .position(|member| member.player == player)
            .ok_or(SettlementError::NotMember {
                id: self.id,
                player,
            })?;
        let removed = self.members.remove(index);
        self.bump_revision();
        Ok(removed)
    }

    /// Change a member's role.
    ///
    /// # Errors
    ///
    /// The owner cannot be demoted ([`SettlementError::OwnerRequired`]); the
    /// owner role is only assigned by a transfer; an unknown player returns
    /// [`SettlementError::NotMember`]. A no-op role change does not bump the
    /// revision.
    pub fn change_role(
        &mut self,
        player: PlayerId,
        role: SettlementRole,
    ) -> Result<(), SettlementError> {
        if self.is_owner(player) {
            return Err(SettlementError::OwnerRequired { id: self.id });
        }
        if role.is_owner() {
            return Err(SettlementError::InvalidRecord {
                id: self.id,
                reason: "owner role requires a transfer",
            });
        }
        let member = self
            .members
            .iter_mut()
            .find(|member| member.player == player)
            .ok_or(SettlementError::NotMember {
                id: self.id,
                player,
            })?;
        if member.role == role {
            return Ok(());
        }
        member.role = role;
        self.bump_revision();
        Ok(())
    }

    /// Make `player` the owner, optionally removing the previous owner.
    ///
    /// Without removal the previous owner is demoted to
    /// [`SettlementRole::Manager`]. The new owner is inserted or promoted in
    /// `members` and the membership revision increments.
    ///
    /// # Errors
    ///
    /// Rejects a full member list when the new owner is not already a member.
    pub fn assign_owner(
        &mut self,
        player: PlayerId,
        remove_previous: bool,
    ) -> Result<(), SettlementError> {
        if self.owner_id == Some(player) {
            return Ok(());
        }
        if self.role_of(player).is_none() && self.members.len() >= MAX_SETTLEMENT_MEMBERS {
            return Err(SettlementError::TooManyMembers {
                limit: MAX_SETTLEMENT_MEMBERS,
            });
        }
        if let Some(previous) = self.owner_id {
            if remove_previous {
                self.members.retain(|member| member.player != previous);
            } else if let Some(member) = self
                .members
                .iter_mut()
                .find(|member| member.player == previous)
            {
                member.role = SettlementRole::Manager;
            }
        }
        match self
            .members
            .iter_mut()
            .find(|member| member.player == player)
        {
            Some(member) => member.role = SettlementRole::Owner,
            None => self.insert_sorted(SettlementMember {
                player,
                role: SettlementRole::Owner,
            }),
        }
        self.owner_id = Some(player);
        self.bump_revision();
        Ok(())
    }

    /// Release ownership and clear every member.
    ///
    /// # Errors
    ///
    /// Returns [`SettlementError::Unowned`] when the settlement has no owner.
    pub fn release(&mut self) -> Result<(), SettlementError> {
        if self.owner_id.is_none() {
            return Err(SettlementError::Unowned { id: self.id });
        }
        self.owner_id = None;
        self.members.clear();
        self.bump_revision();
        Ok(())
    }

    /// Fail when the expected membership revision is stale.
    ///
    /// # Errors
    ///
    /// Returns [`SettlementError::RevisionConflict`] with both revisions.
    pub fn check_revision(&self, expected: u32) -> Result<(), SettlementError> {
        if self.membership_revision == expected {
            Ok(())
        } else {
            Err(SettlementError::RevisionConflict {
                id: self.id,
                expected,
                actual: self.membership_revision,
            })
        }
    }

    /// Whether this record carries any v2 membership data.
    #[must_use]
    pub fn has_membership_data(&self) -> bool {
        self.owner_id.is_some() || !self.members.is_empty() || self.membership_revision != 0
    }

    fn insert_sorted(&mut self, member: SettlementMember) {
        let index = self
            .members
            .partition_point(|existing| existing.player < member.player);
        self.members.insert(index, member);
    }

    fn bump_revision(&mut self) {
        self.membership_revision = self.membership_revision.saturating_add(1);
    }

    /// Validate membership invariants for a loaded save version.
    fn validate_membership(&self, version: u16) -> Result<(), SettlementError> {
        if version == SETTLEMENT_FORMAT_VERSION_V1 {
            if self.has_membership_data() {
                return Err(SettlementError::InvalidRecord {
                    id: self.id,
                    reason: "v1 settlement carries membership data",
                });
            }
            return Ok(());
        }
        if self.members.len() > MAX_SETTLEMENT_MEMBERS {
            return Err(SettlementError::TooManyMembers {
                limit: MAX_SETTLEMENT_MEMBERS,
            });
        }
        let mut previous: Option<PlayerId> = None;
        let mut owners = 0_usize;
        for member in &self.members {
            if let Some(previous_id) = previous {
                if member.player == previous_id {
                    return Err(SettlementError::InvalidRecord {
                        id: self.id,
                        reason: "duplicate member",
                    });
                }
                if member.player < previous_id {
                    return Err(SettlementError::InvalidRecord {
                        id: self.id,
                        reason: "members are not sorted",
                    });
                }
            }
            if member.role.is_owner() {
                owners += 1;
            }
            previous = Some(member.player);
        }
        match self.owner_id {
            Some(owner) => {
                if self.membership_revision == 0 {
                    return Err(SettlementError::InvalidRecord {
                        id: self.id,
                        reason: "owned settlement has zero membership revision",
                    });
                }
                if self.role_of(owner) != Some(SettlementRole::Owner) {
                    return Err(SettlementError::InvalidRecord {
                        id: self.id,
                        reason: "owner is missing from members",
                    });
                }
                if owners != 1 {
                    return Err(SettlementError::InvalidRecord {
                        id: self.id,
                        reason: "more than one owner",
                    });
                }
            }
            None => {
                if owners > 0 || !self.members.is_empty() {
                    return Err(SettlementError::InvalidRecord {
                        id: self.id,
                        reason: "unowned settlement has members",
                    });
                }
            }
        }
        Ok(())
    }
}

const fn is_zero_u32(value: &u32) -> bool {
    *value == 0
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
    /// No settlement has this id.
    Unknown {
        /// Missing id.
        id: u64,
    },
    /// The settlement has no owner.
    Unowned {
        /// Settlement id.
        id: u64,
    },
    /// The actor lacks the permission for this action.
    Forbidden {
        /// Acting player.
        actor: PlayerId,
        /// Short action name.
        action: &'static str,
    },
    /// The player is not a settlement member.
    NotMember {
        /// Settlement id.
        id: u64,
        /// Player that is not a member.
        player: PlayerId,
    },
    /// The settlement already has an owner.
    AlreadyOwned {
        /// Settlement id.
        id: u64,
    },
    /// The owner cannot be removed or demoted.
    OwnerRequired {
        /// Settlement id.
        id: u64,
    },
    /// The expected membership revision is stale.
    RevisionConflict {
        /// Settlement id.
        id: u64,
        /// Revision the caller expected.
        expected: u32,
        /// Current revision.
        actual: u32,
    },
    /// The member list is full.
    TooManyMembers {
        /// Maximum members per settlement.
        limit: usize,
    },
    /// The player owns too many settlements.
    TooManyOwned {
        /// Player that would exceed the cap.
        player: PlayerId,
        /// Maximum owned settlements.
        limit: usize,
    },
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
            Self::Unknown { .. } => "settlement.unknown",
            Self::Unowned { .. } => "settlement.unowned",
            Self::Forbidden { .. } => "settlement.forbidden",
            Self::NotMember { .. } => "settlement.not_member",
            Self::AlreadyOwned { .. } => "settlement.already_owned",
            Self::OwnerRequired { .. } => "settlement.owner_required",
            Self::RevisionConflict { .. } => "settlement.revision_conflict",
            Self::TooManyMembers { .. } => "too_many_members",
            Self::TooManyOwned { .. } => "too_many_owned",
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
            Self::Unknown { id } => write!(f, "unknown settlement {id}"),
            Self::Unowned { id } => write!(f, "settlement {id} has no owner"),
            Self::Forbidden { actor, action } => {
                write!(f, "player {actor} may not {action} this settlement")
            }
            Self::NotMember { id, player } => {
                write!(f, "player {player} is not a member of settlement {id}")
            }
            Self::AlreadyOwned { id } => write!(f, "settlement {id} already has an owner"),
            Self::OwnerRequired { id } => {
                write!(f, "settlement {id} owner cannot be removed or demoted")
            }
            Self::RevisionConflict {
                id,
                expected,
                actual,
            } => write!(
                f,
                "settlement {id} membership revision is {actual}, expected {expected}"
            ),
            Self::TooManyMembers { limit } => {
                write!(f, "settlement member limit of {limit} reached")
            }
            Self::TooManyOwned { player, limit } => write!(
                f,
                "player {player} already owns {limit} settlements"
            ),
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
    /// Create an empty legacy (v1) store.
    ///
    /// The store upgrades to v2 on the first owned or membership mutation, so a
    /// save that never uses membership stays byte-identical to the legacy
    /// format.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            format_version: SETTLEMENT_FORMAT_VERSION_V1,
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

    /// Found an unowned settlement and return it.
    ///
    /// A store that only uses this method stays on the legacy v1 format.
    pub fn found(&mut self, request: &SettlementRequest) -> Result<Settlement, SettlementError> {
        let settlement = self.build(request)?;
        self.insert(settlement.clone());
        Ok(settlement)
    }

    /// Found a settlement owned by `player` and return it.
    ///
    /// The first owned settlement upgrades the store to v2. The owner is
    /// inserted as an [`SettlementRole::Owner`] member with revision 1.
    ///
    /// # Errors
    ///
    /// Adds [`SettlementError::TooManyOwned`] to the [`Self::found`] errors
    /// when the player is at [`MAX_SETTLEMENTS_OWNED_PER_PLAYER`].
    pub fn found_for_player(
        &mut self,
        request: &SettlementRequest,
        player: PlayerId,
    ) -> Result<Settlement, SettlementError> {
        if self.owned_count(player) >= MAX_SETTLEMENTS_OWNED_PER_PLAYER {
            return Err(SettlementError::TooManyOwned {
                player,
                limit: MAX_SETTLEMENTS_OWNED_PER_PLAYER,
            });
        }
        let mut settlement = self.build(request)?;
        settlement.owner_id = Some(player);
        settlement.members.push(SettlementMember {
            player,
            role: SettlementRole::Owner,
        });
        settlement.membership_revision = 1;
        self.ensure_v2();
        self.insert(settlement.clone());
        Ok(settlement)
    }

    /// Compatibility alias for [`Self::found_for_player`].
    pub fn found_owned(
        &mut self,
        request: &SettlementRequest,
        player: PlayerId,
    ) -> Result<Settlement, SettlementError> {
        self.found_for_player(request, player)
    }

    /// Invite a player as a member.
    ///
    /// The actor needs [`SettlementPermission::MEMBERS`].
    ///
    /// # Errors
    ///
    /// Unknown settlement, stale revision, forbidden actor, duplicate player,
    /// owner role, or a full member list.
    pub fn invite(
        &mut self,
        actor: PlayerId,
        settlement_id: u64,
        player: PlayerId,
        role: SettlementRole,
        expected_revision: u32,
    ) -> Result<Settlement, SettlementError> {
        self.mutate_as(
            actor,
            settlement_id,
            expected_revision,
            SettlementPermission::MEMBERS,
            "invite",
            |settlement| settlement.insert_member(player, role),
        )
    }

    /// Remove a member. The owner cannot be removed.
    ///
    /// The actor needs [`SettlementPermission::MEMBERS`].
    ///
    /// # Errors
    ///
    /// Unknown settlement, stale revision, forbidden actor, owner target, or a
    /// player that is not a member.
    pub fn remove_member(
        &mut self,
        actor: PlayerId,
        id: u64,
        player: PlayerId,
        expected_revision: u32,
    ) -> Result<Settlement, SettlementError> {
        self.mutate_as(
            actor,
            id,
            expected_revision,
            SettlementPermission::MEMBERS,
            "remove member",
            |settlement| settlement.remove_member(player).map(|_| ()),
        )
    }

    /// Change a member's role. The owner cannot be demoted.
    ///
    /// The actor needs [`SettlementPermission::MEMBERS`].
    ///
    /// # Errors
    ///
    /// Unknown settlement, stale revision, forbidden actor, owner target, the
    /// owner role as the new role, or a player that is not a member.
    pub fn set_member_role(
        &mut self,
        actor: PlayerId,
        id: u64,
        player: PlayerId,
        role: SettlementRole,
        expected_revision: u32,
    ) -> Result<Settlement, SettlementError> {
        self.mutate_as(
            actor,
            id,
            expected_revision,
            SettlementPermission::MEMBERS,
            "change member role",
            |settlement| settlement.change_role(player, role),
        )
    }

    /// Rename a settlement.
    ///
    /// The actor needs [`SettlementPermission::MANAGE`]. The name is trimmed,
    /// checked against the core name rules, and compared case-insensitively
    /// against every other settlement.
    ///
    /// # Errors
    ///
    /// Unknown settlement, stale revision, forbidden actor, empty/control/
    /// oversized name, or a duplicate name.
    pub fn rename(
        &mut self,
        actor: PlayerId,
        id: u64,
        name: &str,
        expected_revision: u32,
    ) -> Result<Settlement, SettlementError> {
        let trimmed = name.trim();
        validate_name(trimmed)?;
        if self
            .settlements
            .iter()
            .any(|settlement| settlement.id != id && settlement.name.eq_ignore_ascii_case(trimmed))
        {
            return Err(SettlementError::DuplicateName);
        }
        self.mutate_as(
            actor,
            id,
            expected_revision,
            SettlementPermission::MANAGE,
            "rename",
            |settlement| {
                settlement.name = trimmed.to_string();
                settlement.bump_revision();
                Ok(())
            },
        )
    }

    /// Transfer ownership to `recipient`.
    ///
    /// The actor needs [`SettlementPermission::MANAGE`]. The previous owner
    /// stays as [`SettlementRole::Manager`].
    ///
    /// # Errors
    ///
    /// Unknown settlement, stale revision, forbidden actor, unowned settlement,
    /// or a recipient at [`MAX_SETTLEMENTS_OWNED_PER_PLAYER`].
    pub fn transfer(
        &mut self,
        actor: PlayerId,
        id: u64,
        recipient: PlayerId,
        expected_revision: u32,
    ) -> Result<Settlement, SettlementError> {
        let recipient_owns = self.owned_count(recipient);
        let updated = {
            let settlement = self.settlement_mut(id)?;
            settlement.check_revision(expected_revision)?;
            if !settlement.is_owned() {
                return Err(SettlementError::Unowned { id });
            }
            if !settlement.has_permission(actor, SettlementPermission::MANAGE) {
                return Err(SettlementError::Forbidden {
                    actor,
                    action: "transfer",
                });
            }
            if !settlement.is_owner(recipient) && recipient_owns >= MAX_SETTLEMENTS_OWNED_PER_PLAYER
            {
                return Err(SettlementError::TooManyOwned {
                    player: recipient,
                    limit: MAX_SETTLEMENTS_OWNED_PER_PLAYER,
                });
            }
            settlement.assign_owner(recipient, false)?;
            settlement.clone()
        };
        self.ensure_v2();
        Ok(updated)
    }

    /// Claim an unowned settlement.
    ///
    /// There is no permission check: an unowned settlement is free to claim.
    ///
    /// # Errors
    ///
    /// Unknown settlement, stale revision, an owned settlement, or a player at
    /// [`MAX_SETTLEMENTS_OWNED_PER_PLAYER`].
    pub fn claim(
        &mut self,
        actor: PlayerId,
        id: u64,
        expected_revision: u32,
    ) -> Result<Settlement, SettlementError> {
        let owned = self.owned_count(actor);
        let updated = {
            let settlement = self.settlement_mut(id)?;
            settlement.check_revision(expected_revision)?;
            if settlement.is_owned() {
                return Err(SettlementError::AlreadyOwned { id });
            }
            if owned >= MAX_SETTLEMENTS_OWNED_PER_PLAYER {
                return Err(SettlementError::TooManyOwned {
                    player: actor,
                    limit: MAX_SETTLEMENTS_OWNED_PER_PLAYER,
                });
            }
            settlement.assign_owner(actor, false)?;
            settlement.clone()
        };
        self.ensure_v2();
        Ok(updated)
    }

    /// Release ownership and clear every member.
    ///
    /// Core requires the current owner. Server admin equivalents are layered on
    /// top by the server, not here.
    ///
    /// # Errors
    ///
    /// Unknown settlement, stale revision, unowned settlement, or an actor that
    /// is not the current owner.
    pub fn release_owner(
        &mut self,
        actor: PlayerId,
        id: u64,
        expected_revision: u32,
    ) -> Result<Settlement, SettlementError> {
        let updated = {
            let settlement = self.settlement_mut(id)?;
            settlement.check_revision(expected_revision)?;
            if !settlement.is_owned() {
                return Err(SettlementError::Unowned { id });
            }
            if !settlement.is_owner(actor) {
                return Err(SettlementError::Forbidden {
                    actor,
                    action: "release",
                });
            }
            settlement.release()?;
            settlement.clone()
        };
        self.ensure_v2();
        Ok(updated)
    }

    fn build(&mut self, request: &SettlementRequest) -> Result<Settlement, SettlementError> {
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
            owner_id: None,
            members: Vec::new(),
            membership_revision: 0,
        };
        self.next_id += 1;
        Ok(settlement)
    }

    fn insert(&mut self, settlement: Settlement) {
        let index = self
            .settlements
            .partition_point(|existing| existing.id < settlement.id);
        self.settlements.insert(index, settlement);
    }

    fn settlement_mut(&mut self, id: u64) -> Result<&mut Settlement, SettlementError> {
        self.settlements
            .iter_mut()
            .find(|settlement| settlement.id == id)
            .ok_or(SettlementError::Unknown { id })
    }

    fn owned_count(&self, player: PlayerId) -> usize {
        self.settlements
            .iter()
            .filter(|settlement| settlement.is_owner(player))
            .count()
    }

    fn ensure_v2(&mut self) {
        if self.format_version == SETTLEMENT_FORMAT_VERSION_V1 {
            self.format_version = SETTLEMENT_FORMAT_VERSION;
        }
    }

    fn mutate_as<F>(
        &mut self,
        actor: PlayerId,
        id: u64,
        expected_revision: u32,
        permission: SettlementPermission,
        action: &'static str,
        mutate: F,
    ) -> Result<Settlement, SettlementError>
    where
        F: FnOnce(&mut Settlement) -> Result<(), SettlementError>,
    {
        let updated = {
            let settlement = self.settlement_mut(id)?;
            settlement.check_revision(expected_revision)?;
            if !settlement.has_permission(actor, permission) {
                return Err(SettlementError::Forbidden { actor, action });
            }
            mutate(settlement)?;
            settlement.clone()
        };
        self.ensure_v2();
        Ok(updated)
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

    /// Whether any settlement carries v2 ownership or membership data.
    #[must_use]
    pub fn has_membership_data(&self) -> bool {
        self.settlements.iter().any(Settlement::has_membership_data)
    }

    /// Stable 64-bit checksum over the settlement layer.
    ///
    /// A layer without membership data hashes its normalized v1 view, so the
    /// legacy checksums stay stable even if the in-memory store was flagged v2.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let bytes = serde_json::to_vec(&self.normalized())
            .expect("settlement store serialization cannot fail");
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    /// Copy of this layer with the format version implied by its contents.
    fn normalized(&self) -> Self {
        let mut store = self.clone();
        store.format_version = if store.has_membership_data() {
            SETTLEMENT_FORMAT_VERSION
        } else {
            SETTLEMENT_FORMAT_VERSION_V1
        };
        store
    }

    /// Map legacy owner labels to player ids.
    ///
    /// Only an exactly one case-insensitive display-name match maps. Ambiguous
    /// or unmatched labels stay unowned and keep the legacy `owner` string as
    /// their display label. Already-owned settlements are skipped, so a second
    /// run maps nothing. A matched player already at
    /// [`MAX_SETTLEMENTS_OWNED_PER_PLAYER`] counts as unmatched.
    pub fn migrate_owner_labels(&mut self, registry: &PlayerRegistry) -> MigrationReport {
        let mut report = MigrationReport::default();
        for index in 0..self.settlements.len() {
            if self.settlements[index].is_owned() {
                continue;
            }
            let label = self.settlements[index].owner.clone();
            let matches = registry.find_by_display_name(&label);
            if matches.len() != 1 {
                if matches.is_empty() {
                    report.unmatched += 1;
                } else {
                    report.ambiguous += 1;
                }
                continue;
            }
            let player = matches[0].id;
            if self.owned_count(player) >= MAX_SETTLEMENTS_OWNED_PER_PLAYER {
                report.unmatched += 1;
                continue;
            }
            if self.settlements[index].assign_owner(player, false).is_err() {
                report.unmatched += 1;
            } else {
                report.mapped += 1;
            }
        }
        if report.mapped > 0 {
            self.ensure_v2();
        }
        report
    }

    /// Pretty JSON used by the world save.
    pub fn to_json_pretty(&self) -> Result<String, SettlementError> {
        serde_json::to_string_pretty(self)
            .map_err(|error| SettlementError::InvalidJson(error.to_string()))
    }

    /// Parse and validate a settlement layer.
    ///
    /// A v2 layer with no membership data normalizes back to v1.
    pub fn from_json(json: &str) -> Result<Self, SettlementError> {
        let store: Self = serde_json::from_str(json)
            .map_err(|error| SettlementError::InvalidJson(error.to_string()))?;
        store.validate_loaded()?;
        Ok(store.normalized())
    }

    /// Validate a decoded store, including ids, versions, and membership.
    ///
    /// Accepts v1 and v2. v1 records must not carry membership data; v2 records
    /// must satisfy the ownership and membership invariants.
    pub fn validate_loaded(&self) -> Result<(), SettlementError> {
        if self.format_version != SETTLEMENT_FORMAT_VERSION_V1
            && self.format_version != SETTLEMENT_FORMAT_VERSION
        {
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
            settlement.validate_membership(self.format_version)?;
            if !names.insert(settlement.name.to_lowercase()) {
                return Err(SettlementError::DuplicateName);
            }
            max_id = max_id.max(settlement.id);
        }
        let mut owned: HashMap<PlayerId, usize> = HashMap::new();
        for settlement in &self.settlements {
            if let Some(owner) = settlement.owner_id {
                let count = owned.entry(owner).or_insert(0);
                *count += 1;
                if *count > MAX_SETTLEMENTS_OWNED_PER_PLAYER {
                    return Err(SettlementError::TooManyOwned {
                        player: owner,
                        limit: MAX_SETTLEMENTS_OWNED_PER_PLAYER,
                    });
                }
            }
        }
        if self.next_id == 0 || self.next_id <= max_id || self.next_id >= MAX_SETTLEMENT_ID {
            return Err(SettlementError::InvalidNextId {
                next_id: self.next_id,
            });
        }
        Ok(())
    }
}

/// Counts from [`SettlementStore::migrate_owner_labels`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MigrationReport {
    /// Settlements whose legacy label matched exactly one player.
    pub mapped: usize,
    /// Settlements whose legacy label matched more than one player.
    pub ambiguous: usize,
    /// Settlements with no match, or a matched player at the ownership cap.
    pub unmatched: usize,
}

fn validate_name(name: &str) -> Result<(), SettlementError> {
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
    Ok(())
}

fn validate_request(request: &SettlementRequest) -> Result<(), SettlementError> {
    validate_name(request.name.trim())?;
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

    fn player(value: u64) -> PlayerId {
        PlayerId::new(value).expect("player id")
    }

    fn owned_store() -> (SettlementStore, Settlement) {
        let mut store = SettlementStore::new(2026);
        let settlement = store
            .found_for_player(&request("Riverbend", 0, 0, 64), player(1))
            .expect("found");
        (store, settlement)
    }

    fn owned_request(name: &str, owner: &str) -> SettlementRequest {
        SettlementRequest {
            owner: owner.to_string(),
            ..request(name, 0, 0, 32)
        }
    }

    #[test]
    fn role_presets_and_effective_permissions_match_the_matrix() {
        assert_eq!(
            SettlementRole::Owner.permissions(),
            SettlementPermission::ALL
        );
        let manager = SettlementRole::Manager.permissions();
        for permission in [
            SettlementPermission::BUILD,
            SettlementPermission::MANAGE,
            SettlementPermission::TAX,
            SettlementPermission::MEMBERS,
        ] {
            assert!(manager.contains(permission));
        }
        assert_eq!(
            SettlementRole::Builder.permissions(),
            SettlementPermission::BUILD
        );
        assert_eq!(
            SettlementRole::Member.permissions(),
            SettlementPermission::NONE
        );
        assert_eq!(
            SettlementPermission::from_bits(0b1111),
            Some(SettlementPermission::ALL)
        );
        assert_eq!(SettlementPermission::from_bits(0b1_0000), None);
        assert!(SettlementPermission::ALL.contains(SettlementPermission::TAX));
        assert!(!SettlementPermission::BUILD.contains(SettlementPermission::MANAGE));

        let mut settlement = Settlement {
            id: 1,
            name: "A".to_string(),
            owner: "a".to_string(),
            center: [0, 0],
            radius_m: 32,
            owner_id: Some(player(1)),
            members: vec![
                SettlementMember {
                    player: player(2),
                    role: SettlementRole::Manager,
                },
                SettlementMember {
                    player: player(3),
                    role: SettlementRole::Builder,
                },
                SettlementMember {
                    player: player(4),
                    role: SettlementRole::Member,
                },
            ],
            membership_revision: 1,
        };
        // The owner keeps every bit even when a malformed record omits the
        // owner from `members`.
        assert_eq!(
            settlement.effective_permissions(player(1)),
            SettlementPermission::ALL
        );
        assert!(settlement.has_permission(player(2), SettlementPermission::MEMBERS));
        assert_eq!(
            settlement.effective_permissions(player(2)),
            SettlementPermission::ALL
        );
        assert!(settlement.has_permission(player(3), SettlementPermission::BUILD));
        assert!(!settlement.has_permission(player(3), SettlementPermission::ALL));
        assert_eq!(
            settlement.effective_permissions(player(4)),
            SettlementPermission::NONE
        );
        assert_eq!(
            settlement.effective_permissions(player(5)),
            SettlementPermission::NONE
        );
        assert_eq!(settlement.owner_permissions(), SettlementPermission::ALL);
        settlement.owner_id = None;
        assert_eq!(settlement.owner_permissions(), SettlementPermission::NONE);
    }

    #[test]
    fn owner_cannot_be_removed_demoted_or_invited() {
        let (mut store, settlement) = owned_store();
        let id = settlement.id;
        assert_eq!(
            store.remove_member(player(1), id, player(1), 1),
            Err(SettlementError::OwnerRequired { id })
        );
        assert_eq!(
            store.set_member_role(player(1), id, player(1), SettlementRole::Manager, 1),
            Err(SettlementError::OwnerRequired { id })
        );
        assert_eq!(
            store.invite(player(1), id, player(1), SettlementRole::Builder, 1),
            Err(SettlementError::InvalidRecord {
                id,
                reason: "player is already a member"
            })
        );
        assert_eq!(
            store.invite(player(1), id, player(2), SettlementRole::Owner, 1),
            Err(SettlementError::InvalidRecord {
                id,
                reason: "owner role requires a transfer"
            })
        );
        let unchanged = store.get(id).expect("settlement");
        assert_eq!(unchanged.owner_id, Some(player(1)));
        assert_eq!(unchanged.membership_revision, 1);
        assert_eq!(unchanged.members.len(), 1);
    }

    #[test]
    fn actor_permissions_gate_the_membership_wrappers() {
        let (mut store, settlement) = owned_store();
        let id = settlement.id;
        let mut revision = settlement.membership_revision;
        for (value, role) in [
            (2, SettlementRole::Manager),
            (3, SettlementRole::Builder),
            (4, SettlementRole::Member),
        ] {
            revision = store
                .invite(player(1), id, player(value), role, revision)
                .expect("invite")
                .membership_revision;
        }

        assert_eq!(
            store.invite(player(3), id, player(5), SettlementRole::Member, revision),
            Err(SettlementError::Forbidden {
                actor: player(3),
                action: "invite"
            })
        );
        assert_eq!(
            store.invite(player(4), id, player(5), SettlementRole::Member, revision),
            Err(SettlementError::Forbidden {
                actor: player(4),
                action: "invite"
            })
        );
        assert_eq!(
            store.remove_member(player(3), id, player(4), revision),
            Err(SettlementError::Forbidden {
                actor: player(3),
                action: "remove member"
            })
        );

        // A manager may invite and change roles.
        let updated = store
            .invite(player(2), id, player(5), SettlementRole::Member, revision)
            .expect("manager invite");
        assert_eq!(updated.role_of(player(5)), Some(SettlementRole::Member));
        let updated = store
            .set_member_role(
                player(2),
                id,
                player(5),
                SettlementRole::Builder,
                updated.membership_revision,
            )
            .expect("manager role change");
        assert_eq!(updated.role_of(player(5)), Some(SettlementRole::Builder));
        let updated = store
            .remove_member(player(2), id, player(5), updated.membership_revision)
            .expect("manager remove");
        assert_eq!(updated.role_of(player(5)), None);
    }

    #[test]
    fn stale_revisions_are_rejected_without_mutation() {
        let (mut store, settlement) = owned_store();
        let id = settlement.id;
        let error = store.claim(player(2), id, 7).expect_err("stale claim");
        assert_eq!(
            error,
            SettlementError::RevisionConflict {
                id,
                expected: 7,
                actual: 1
            }
        );
        assert_eq!(error.code(), "settlement.revision_conflict");
        assert_eq!(
            store.invite(player(1), id, player(2), SettlementRole::Member, 6),
            Err(SettlementError::RevisionConflict {
                id,
                expected: 6,
                actual: 1
            })
        );
        assert_eq!(store.get(id).expect("settlement").membership_revision, 1);
    }

    #[test]
    fn duplicate_and_unknown_members_are_rejected() {
        let (mut store, settlement) = owned_store();
        let id = settlement.id;
        let revision = store
            .invite(player(1), id, player(2), SettlementRole::Builder, 1)
            .expect("invite")
            .membership_revision;
        assert_eq!(
            store.invite(player(1), id, player(2), SettlementRole::Member, revision),
            Err(SettlementError::InvalidRecord {
                id,
                reason: "player is already a member"
            })
        );
        assert_eq!(
            store.remove_member(player(1), id, player(3), revision),
            Err(SettlementError::NotMember {
                id,
                player: player(3)
            })
        );
        assert_eq!(
            store.set_member_role(player(1), id, player(3), SettlementRole::Builder, revision),
            Err(SettlementError::NotMember {
                id,
                player: player(3)
            })
        );
        assert_eq!(
            store.claim(player(9), 999, 0),
            Err(SettlementError::Unknown { id: 999 })
        );
    }

    #[test]
    fn claim_transfer_and_release_follow_owner_invariants() {
        let mut store = SettlementStore::new(2026);
        let settlement = store.found(&request("Riverbend", 0, 0, 32)).expect("found");
        assert_eq!(store.format_version, SETTLEMENT_FORMAT_VERSION_V1);

        let claimed = store.claim(player(2), settlement.id, 0).expect("claim");
        assert_eq!(store.format_version, SETTLEMENT_FORMAT_VERSION);
        assert_eq!(claimed.owner_id, Some(player(2)));
        assert_eq!(claimed.membership_revision, 1);
        assert_eq!(
            store.claim(player(3), settlement.id, 1),
            Err(SettlementError::AlreadyOwned { id: settlement.id })
        );

        let transferred = store
            .transfer(player(2), settlement.id, player(3), 1)
            .expect("transfer");
        assert_eq!(transferred.owner_id, Some(player(3)));
        assert_eq!(
            transferred.role_of(player(2)),
            Some(SettlementRole::Manager)
        );
        assert_eq!(transferred.membership_revision, 2);

        assert_eq!(
            store.release_owner(player(2), settlement.id, 2),
            Err(SettlementError::Forbidden {
                actor: player(2),
                action: "release"
            })
        );
        let released = store
            .release_owner(player(3), settlement.id, 2)
            .expect("release");
        assert_eq!(released.owner_id, None);
        assert!(released.members.is_empty());
        assert_eq!(released.membership_revision, 3);
        assert_eq!(
            store.release_owner(player(3), settlement.id, 3),
            Err(SettlementError::Unowned { id: settlement.id })
        );
        assert_eq!(
            store.transfer(player(3), settlement.id, player(4), 3),
            Err(SettlementError::Unowned { id: settlement.id })
        );

        // Claiming after a release starts a fresh ownership term.
        let reclaimed = store.claim(player(4), settlement.id, 3).expect("reclaim");
        assert_eq!(reclaimed.owner_id, Some(player(4)));
        assert_eq!(reclaimed.membership_revision, 4);
    }

    #[test]
    fn rename_requires_manage_and_a_fresh_unique_name() {
        let (mut store, settlement) = owned_store();
        let revision = settlement.membership_revision;
        store
            .found_for_player(&request("Docks", 200, 0, 32), player(2))
            .expect("second settlement");

        let renamed = store
            .rename(player(1), settlement.id, "  Harbour  ", revision)
            .expect("rename");
        assert_eq!(renamed.name, "Harbour");
        assert_eq!(renamed.membership_revision, revision + 1);

        assert_eq!(
            store.rename(player(1), settlement.id, "docks", revision + 1),
            Err(SettlementError::DuplicateName)
        );
        assert_eq!(
            store.rename(player(1), settlement.id, "Harbour", 0),
            Err(SettlementError::RevisionConflict {
                id: settlement.id,
                expected: 0,
                actual: revision + 1
            })
        );
        assert_eq!(
            store.rename(player(1), settlement.id, "  ", revision + 1),
            Err(SettlementError::EmptyName)
        );

        let invited = store
            .invite(
                player(1),
                settlement.id,
                player(3),
                SettlementRole::Builder,
                revision + 1,
            )
            .expect("invite");
        assert_eq!(
            store.rename(
                player(3),
                settlement.id,
                "Harbour",
                invited.membership_revision
            ),
            Err(SettlementError::Forbidden {
                actor: player(3),
                action: "rename"
            })
        );
    }

    #[test]
    fn member_and_ownership_limits_are_enforced() {
        let (mut store, settlement) = owned_store();
        let mut revision = settlement.membership_revision;
        for value in 2..=MAX_SETTLEMENT_MEMBERS as u64 {
            revision = store
                .invite(
                    player(1),
                    settlement.id,
                    player(value),
                    SettlementRole::Member,
                    revision,
                )
                .expect("invite")
                .membership_revision;
        }
        assert_eq!(
            store.get(settlement.id).expect("settlement").members.len(),
            MAX_SETTLEMENT_MEMBERS
        );
        assert_eq!(
            store.invite(
                player(1),
                settlement.id,
                player(100),
                SettlementRole::Member,
                revision
            ),
            Err(SettlementError::TooManyMembers {
                limit: MAX_SETTLEMENT_MEMBERS
            })
        );

        let mut owner = SettlementStore::new(2026);
        for index in 0..MAX_SETTLEMENTS_OWNED_PER_PLAYER {
            owner
                .found_for_player(
                    &request(
                        &format!("S{index}"),
                        i32::try_from(index).expect("coord"),
                        0,
                        16,
                    ),
                    player(1),
                )
                .expect("found");
        }
        assert_eq!(
            owner.found_for_player(&request("OneTooMany", 0, 0, 16), player(1)),
            Err(SettlementError::TooManyOwned {
                player: player(1),
                limit: MAX_SETTLEMENTS_OWNED_PER_PLAYER
            })
        );
        owner
            .found_for_player(&request("Neighbour", 0, 0, 16), player(2))
            .expect("second player may still found");
    }

    #[test]
    fn v1_and_v2_round_trip_through_json() {
        let mut legacy = SettlementStore::new(2026);
        legacy
            .found(&request("Riverbend", 0, 0, 32))
            .expect("found");
        let json = legacy.to_json_pretty().expect("json");
        assert_eq!(legacy.format_version, SETTLEMENT_FORMAT_VERSION_V1);
        assert!(!json.contains("owner_id"));
        assert!(!json.contains("members"));
        assert!(!json.contains("membership_revision"));
        let decoded = SettlementStore::from_json(&json).expect("decode");
        assert_eq!(decoded, legacy);

        let (mut owned, settlement) = owned_store();
        owned
            .invite(
                player(1),
                settlement.id,
                player(2),
                SettlementRole::Builder,
                1,
            )
            .expect("invite");
        let json = owned.to_json_pretty().expect("json");
        assert_eq!(owned.format_version, SETTLEMENT_FORMAT_VERSION);
        assert!(json.contains("owner_id"));
        assert!(json.contains("membership_revision"));
        let decoded = SettlementStore::from_json(&json).expect("decode");
        assert_eq!(decoded.format_version, SETTLEMENT_FORMAT_VERSION);
        assert_eq!(decoded, owned);
        assert_eq!(decoded.checksum(), owned.checksum());
    }

    #[test]
    fn malformed_v2_membership_is_rejected() {
        let (_, settlement) = owned_store();

        let mut missing_owner = SettlementStore::new(2026);
        let mut record = settlement.clone();
        record.members.clear();
        missing_owner.format_version = SETTLEMENT_FORMAT_VERSION;
        missing_owner.next_id = 2;
        missing_owner.settlements.push(record);
        assert_eq!(
            missing_owner.validate_loaded(),
            Err(SettlementError::InvalidRecord {
                id: 1,
                reason: "owner is missing from members"
            })
        );

        let mut duplicate = missing_owner.clone();
        duplicate.settlements[0].members = vec![
            SettlementMember {
                player: player(1),
                role: SettlementRole::Owner,
            },
            SettlementMember {
                player: player(1),
                role: SettlementRole::Owner,
            },
        ];
        assert_eq!(
            duplicate.validate_loaded(),
            Err(SettlementError::InvalidRecord {
                id: 1,
                reason: "duplicate member"
            })
        );

        let mut unowned = missing_owner.clone();
        unowned.settlements[0].owner_id = None;
        unowned.settlements[0].members = vec![SettlementMember {
            player: player(1),
            role: SettlementRole::Owner,
        }];
        unowned.settlements[0].membership_revision = 1;
        assert_eq!(
            unowned.validate_loaded(),
            Err(SettlementError::InvalidRecord {
                id: 1,
                reason: "unowned settlement has members"
            })
        );

        let mut legacy_with_membership = missing_owner.clone();
        legacy_with_membership.format_version = SETTLEMENT_FORMAT_VERSION_V1;
        assert_eq!(
            legacy_with_membership.validate_loaded(),
            Err(SettlementError::InvalidRecord {
                id: 1,
                reason: "v1 settlement carries membership data"
            })
        );

        let json = r#"{"format_version":2,"seed":1,"next_id":2,"settlements":[{"id":1,"name":"A","owner":"a","center":[0,0],"radius_m":16,"owner_id":1}]}"#;
        assert_eq!(
            SettlementStore::from_json(json),
            Err(SettlementError::InvalidRecord {
                id: 1,
                reason: "owned settlement has zero membership revision"
            })
        );
    }

    #[test]
    fn legacy_checksums_are_pinned_and_the_upgrade_is_lazy() {
        let mut store = SettlementStore::new(2026);
        assert_eq!(store.format_version, SETTLEMENT_FORMAT_VERSION_V1);
        assert_eq!(store.checksum(), 0xf8cb_9141_3b66_c82b);
        store
            .found(&request("Riverbend", 100, 100, 64))
            .expect("found");
        assert_eq!(store.checksum(), 0x8b1a_7dca_a78b_f583);
        assert_eq!(store.format_version, SETTLEMENT_FORMAT_VERSION_V1);

        // A stray v2 flag without membership data still hashes the v1 view.
        let legacy_checksum = store.checksum();
        store.format_version = SETTLEMENT_FORMAT_VERSION;
        assert_eq!(store.checksum(), legacy_checksum);
        store.format_version = SETTLEMENT_FORMAT_VERSION_V1;

        let settlement = store
            .found_for_player(&request("Owned", 0, 0, 32), player(1))
            .expect("found");
        assert_eq!(store.format_version, SETTLEMENT_FORMAT_VERSION);
        assert_ne!(store.checksum(), legacy_checksum);
        assert_eq!(
            store.get(settlement.id).map(|s| s.membership_revision),
            Some(1)
        );
        let decoded =
            SettlementStore::from_json(&store.to_json_pretty().expect("json")).expect("decode");
        assert_eq!(decoded, store);

        // A v2 layer without membership data normalizes back to v1 on load.
        store.settlements.clear();
        store.next_id = 1;
        store.format_version = SETTLEMENT_FORMAT_VERSION;
        assert!(!store.has_membership_data());
        let decoded =
            SettlementStore::from_json(&store.to_json_pretty().expect("json")).expect("decode");
        assert_eq!(decoded.format_version, SETTLEMENT_FORMAT_VERSION_V1);
        assert_eq!(decoded.checksum(), 0xf8cb_9141_3b66_c82b);
    }

    #[test]
    fn migration_maps_only_exactly_one_case_insensitive_match() {
        use crate::player::{Player, PlayerRegistry, PLAYER_FORMAT_VERSION};

        let registry = PlayerRegistry {
            format_version: PLAYER_FORMAT_VERSION,
            seed: 2026,
            next_id: 5,
            players: vec![
                Player {
                    id: player(1),
                    display_name: "Ada".to_string(),
                },
                Player {
                    id: player(2),
                    display_name: "Grace".to_string(),
                },
                // A malformed registry can hold two case-insensitive matches;
                // the migration must treat the label as ambiguous.
                Player {
                    id: player(3),
                    display_name: "Twin".to_string(),
                },
                Player {
                    id: player(4),
                    display_name: "TWIN".to_string(),
                },
            ],
        };

        let mut store = SettlementStore::new(2026);
        let mapped = store.found(&owned_request("Mapped", "Ada")).expect("found");
        let ambiguous = store.found(&owned_request("Twin", "twin")).expect("found");
        let unmatched = store
            .found(&owned_request("Lonely", "Nobody"))
            .expect("found");

        let report = store.migrate_owner_labels(&registry);
        assert_eq!(
            report,
            MigrationReport {
                mapped: 1,
                ambiguous: 1,
                unmatched: 1
            }
        );
        assert_eq!(store.format_version, SETTLEMENT_FORMAT_VERSION);
        assert_eq!(
            store.get(mapped.id).expect("mapped").owner_id,
            Some(player(1))
        );
        assert_eq!(
            store.get(mapped.id).expect("mapped").owner,
            "Ada".to_string()
        );
        assert_eq!(store.get(ambiguous.id).expect("ambiguous").owner_id, None);
        assert_eq!(
            store.get(ambiguous.id).expect("ambiguous").owner,
            "twin".to_string()
        );
        assert_eq!(store.get(unmatched.id).expect("unmatched").owner_id, None);

        let json = store.to_json_pretty().expect("json");
        let decoded = SettlementStore::from_json(&json).expect("decode");
        assert_eq!(decoded, store);

        let again = store.migrate_owner_labels(&registry);
        assert_eq!(again.mapped, 0);
        assert_eq!(
            store.get(mapped.id).expect("mapped").owner_id,
            Some(player(1))
        );
    }

    #[test]
    fn membership_errors_expose_stable_codes_and_messages() {
        let cases = [
            (SettlementError::Unknown { id: 1 }, "settlement.unknown"),
            (SettlementError::Unowned { id: 1 }, "settlement.unowned"),
            (
                SettlementError::Forbidden {
                    actor: player(1),
                    action: "invite",
                },
                "settlement.forbidden",
            ),
            (
                SettlementError::NotMember {
                    id: 1,
                    player: player(2),
                },
                "settlement.not_member",
            ),
            (
                SettlementError::AlreadyOwned { id: 1 },
                "settlement.already_owned",
            ),
            (
                SettlementError::OwnerRequired { id: 1 },
                "settlement.owner_required",
            ),
            (
                SettlementError::RevisionConflict {
                    id: 1,
                    expected: 1,
                    actual: 2,
                },
                "settlement.revision_conflict",
            ),
            (
                SettlementError::TooManyMembers { limit: 64 },
                "too_many_members",
            ),
            (
                SettlementError::TooManyOwned {
                    player: player(1),
                    limit: 16,
                },
                "too_many_owned",
            ),
        ];
        for (error, code) in cases {
            assert_eq!(error.code(), code);
            assert!(!error.to_string().is_empty());
        }
    }
}
