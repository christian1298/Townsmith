//! Persisted district-policy registry v0.
//!
//! A district policy is a named circle on the world XZ plane with a tax
//! multiplier, a service-funding multiplier, a construction switch, and a
//! blueprint allowlist. This module is the **data layer** plus one pure
//! placement rule:
//!
//! - [`policy_allows_placement`] and [`policy_placement_denial`] decide whether
//!   a blueprint id may be placed under one policy. The bridge applies the
//!   result to `world_place_blueprint`; the treasury also reads `tax_multiplier`
//!   (D-030) and `service_funding` (D-031);
//! - no UI displays or edits a policy except the standalone developer scene;
//! - [`DistrictPolicyStore`] can be composed into the world-save envelope as an
//!   optional `policies` object; saving and loading persist the records, but
//!   saving does not apply any tax or funding effect.
//!
//! The registry is additive and owns its own JSON representation. Composing it
//! into the world save must not change the policy record fields, the existing
//! world-save checksum, or a save written without a registry. The deterministic
//! lookup rule in this slice is the smallest policy id whose inclusive circle
//! contains a point; overlap resolution, priority, and the actual tax/funding
//! effects remain open design work.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::placement::MAX_WORLD_COORD_M;

/// Current district-policy save format.
pub const DISTRICT_POLICY_FORMAT_VERSION: u16 = 1;

/// Hard cap for one registry.
pub const MAX_DISTRICT_POLICIES: usize = 4_096;

/// Largest id the registry may assign.
///
/// Ids start at 1 and stop before this value, so the largest assigned id is
/// `i64::MAX - 1`. That keeps every id representable by the GDScript bridge.
pub const MAX_DISTRICT_POLICY_ID: u64 = i64::MAX as u64;

/// Smallest accepted district radius in metres.
pub const MIN_DISTRICT_POLICY_RADIUS_M: f32 = 16.0;

/// Largest accepted district radius in metres.
pub const MAX_DISTRICT_POLICY_RADIUS_M: f32 = 4_096.0;

/// Smallest accepted tax multiplier.
pub const MIN_DISTRICT_TAX_MULTIPLIER: f32 = 0.25;

/// Largest accepted tax multiplier.
pub const MAX_DISTRICT_TAX_MULTIPLIER: f32 = 4.0;

/// Largest accepted service-funding multiplier.
pub const MAX_DISTRICT_SERVICE_FUNDING: f32 = 2.0;

/// Longest accepted district name, in trimmed characters.
pub const MAX_DISTRICT_POLICY_NAME_CHARS: usize = 64;

/// Largest accepted blueprint allowlist.
pub const MAX_DISTRICT_POLICY_ALLOWLIST: usize = 256;

/// One saved district policy.
///
/// [`construction_allowed`](Self::construction_allowed) and
/// [`blueprint_allowlist`](Self::blueprint_allowlist) are applied to blueprint
/// placement by the bridge through [`policy_placement_denial`]. The treasury
/// applies `tax_multiplier` to city income (D-030) and `service_funding` to
/// civic service upkeep (D-031).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DistrictPolicy {
    /// Stable id within the world. Assigned by the store and immutable on
    /// update.
    pub id: u64,
    /// Player-facing name, stored trimmed.
    pub name: String,
    /// Circle centre `[x, z]` in world metres.
    pub center: [f32; 2],
    /// Inclusive circle radius in metres, `16..=4096`.
    pub radius_m: f32,
    /// Tax multiplier, `0.25..=4.0`. Applied to city tax income by D-030.
    pub tax_multiplier: f32,
    /// Service-funding multiplier, `0.0..=2.0`. Applied to civic service
    /// upkeep by D-031.
    pub service_funding: f32,
    /// Whether construction is allowed inside the circle. Enforced for
    /// blueprint placement by the bridge.
    pub construction_allowed: bool,
    /// Namespaced blueprint ids allowed in the district. An empty set is
    /// unrestricted; a non-empty set denies every other id at blueprint
    /// placement. See [`policy_allows_placement`].
    pub blueprint_allowlist: BTreeSet<String>,
}

impl DistrictPolicy {
    /// Whether a world point lies inside the policy circle.
    ///
    /// The circle is inclusive: a point exactly on the radius is contained.
    /// A non-finite point is never contained. Only the XZ plane is considered;
    /// terrain height is irrelevant.
    #[must_use]
    pub fn contains(&self, point: [f32; 2]) -> bool {
        if !point.iter().all(|value| value.is_finite()) {
            return false;
        }
        let dx = f64::from(point[0]) - f64::from(self.center[0]);
        let dz = f64::from(point[1]) - f64::from(self.center[1]);
        let radius = f64::from(self.radius_m);
        dx * dx + dz * dz <= radius * radius
    }

    /// Whether two policy circles intersect or touch.
    ///
    /// Touching circles (`distance == radius_a + radius_b`) count as
    /// overlapping. This is a geometric helper only; the registry does not
    /// reject overlaps.
    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        let dx = f64::from(self.center[0]) - f64::from(other.center[0]);
        let dz = f64::from(self.center[1]) - f64::from(other.center[1]);
        let radii = f64::from(self.radius_m) + f64::from(other.radius_m);
        dx * dx + dz * dz <= radii * radii
    }

    /// Whether this record is complete enough to enforce a placement rule.
    ///
    /// A stored policy is always complete: [`DistrictPolicyStore`] validates
    /// every record on create and load. This guard makes a hand-built record
    /// neutral instead of allowing its missing or non-finite geometry to
    /// produce a denial. The rule only uses the fields that decide
    /// containment and identity, so a malformed name or allowlist entry does
    /// not disable an otherwise usable circle.
    #[must_use]
    pub fn is_enforceable(&self) -> bool {
        self.id != 0
            && self.center.iter().all(|value| value.is_finite())
            && self.radius_m.is_finite()
            && self.radius_m > 0.0
    }
}

/// Why a district policy denies a blueprint placement.
///
/// Both kinds are stable data: the bridge returns [`Self::code`] through its
/// usual `{ ok: false, error_code, error }` result and a smoke can pin it
/// without parsing prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyPlacementDenial {
    /// The policy's `construction_allowed` flag is `false`.
    ConstructionDisabled,
    /// The policy's non-empty allowlist does not contain the blueprint id.
    BlueprintNotAllowed,
}

impl PolicyPlacementDenial {
    /// Stable machine-readable code for the bridge error shape.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ConstructionDisabled => "policy_construction_disabled",
            Self::BlueprintNotAllowed => "policy_blueprint_not_allowed",
        }
    }
}

/// Whether `policy` permits placing `blueprint_id`.
///
/// This is the pure placement rule shared by the bridge and tests:
///
/// - a policy that is not [`DistrictPolicy::is_enforceable`] is neutral and
///   permits every blueprint;
/// - `construction_allowed == false` denies every blueprint;
/// - an empty `blueprint_allowlist` permits every blueprint;
/// - a non-empty allowlist permits only its exact, case-sensitive ids.
///
/// Circle containment is not part of this helper; callers resolve the policy
/// with [`DistrictPolicyStore::policy_at`] (smallest containing id) first.
#[must_use]
pub fn policy_allows_placement(policy: &DistrictPolicy, blueprint_id: &str) -> bool {
    policy_placement_denial(policy, blueprint_id).is_none()
}

/// The reason `policy` denies `blueprint_id`, or `None` when it permits it.
///
/// `ConstructionDisabled` takes priority over the allowlist, so a policy with
/// both fields denying a placement reports the construction switch. See
/// [`policy_allows_placement`] for the full rule.
#[must_use]
pub fn policy_placement_denial(
    policy: &DistrictPolicy,
    blueprint_id: &str,
) -> Option<PolicyPlacementDenial> {
    if !policy.is_enforceable() {
        return None;
    }
    if !policy.construction_allowed {
        return Some(PolicyPlacementDenial::ConstructionDisabled);
    }
    if policy.blueprint_allowlist.is_empty() || policy.blueprint_allowlist.contains(blueprint_id) {
        None
    } else {
        Some(PolicyPlacementDenial::BlueprintNotAllowed)
    }
}

/// One create or update request.
///
/// The id is assigned by [`DistrictPolicyStore::create`] and cannot be changed
/// by [`DistrictPolicyStore::update`].
#[derive(Debug, Clone, PartialEq)]
pub struct DistrictPolicyRequest {
    /// Player-facing name; trimmed on create and update.
    pub name: String,
    /// Circle centre `[x, z]` in world metres.
    pub center: [f32; 2],
    /// Inclusive circle radius in metres.
    pub radius_m: f32,
    /// Tax multiplier.
    pub tax_multiplier: f32,
    /// Service-funding multiplier.
    pub service_funding: f32,
    /// Whether construction is nominally allowed.
    pub construction_allowed: bool,
    /// Namespaced blueprint ids allowed in the district.
    pub blueprint_allowlist: BTreeSet<String>,
}

/// A district-policy validation, persistence, or lookup failure.
#[derive(Debug, Clone, PartialEq)]
pub enum DistrictPolicyError {
    /// A centre coordinate is NaN or infinite.
    InvalidCenter {
        /// Rejected centre.
        center: [f32; 2],
    },
    /// A centre coordinate is outside the supported world range.
    CoordinateOutOfRange {
        /// Axis name.
        axis: &'static str,
        /// Rejected value.
        value: f32,
    },
    /// The radius is non-finite or outside `16..=4096` m.
    InvalidRadius {
        /// Rejected radius.
        radius_m: f32,
    },
    /// The tax multiplier is non-finite or outside `0.25..=4.0`.
    InvalidTaxMultiplier {
        /// Rejected multiplier.
        value: f32,
    },
    /// The service funding is non-finite or outside `0..=2.0`.
    InvalidServiceFunding {
        /// Rejected funding.
        value: f32,
    },
    /// The trimmed name is empty.
    EmptyName,
    /// The name is longer than [`MAX_DISTRICT_POLICY_NAME_CHARS`].
    NameTooLong {
        /// Maximum accepted length.
        limit: usize,
    },
    /// The name contains a control character or was not stored trimmed.
    InvalidName,
    /// An allowlist entry does not match `^[a-z0-9_]+:[a-z0-9_./-]+$`.
    InvalidAllowlistEntry {
        /// Rejected entry.
        entry: String,
    },
    /// The allowlist has more than [`MAX_DISTRICT_POLICY_ALLOWLIST`] entries.
    TooManyAllowlistEntries {
        /// Rejected entry count.
        count: usize,
    },
    /// The registry already holds [`MAX_DISTRICT_POLICIES`] records.
    Full,
    /// No policy has the requested id.
    NotFound {
        /// Requested id.
        id: u64,
    },
    /// No more ids can be assigned.
    IdExhausted,
    /// A loaded record repeats an id.
    ///
    /// The policy map is keyed by id, so a normal load cannot produce this;
    /// the variant is kept for the bridge error taxonomy and future import
    /// paths.
    DuplicateId {
        /// Duplicated id.
        id: u64,
    },
    /// A loaded record is malformed or internally inconsistent.
    InvalidRecord {
        /// Policy id.
        id: u64,
        /// Short reason.
        reason: &'static str,
    },
    /// `next_id` is zero or does not leave room for the stored records.
    InvalidNextId {
        /// Rejected counter.
        next_id: u64,
    },
    /// The JSON could not be parsed.
    InvalidJson(String),
    /// The registry uses an unknown format version.
    UnsupportedVersion {
        /// Rejected version.
        version: u16,
    },
    /// The loaded registry belongs to a different world seed.
    SeedMismatch {
        /// Seed the caller expects.
        expected: u64,
        /// Seed found in the JSON.
        found: u64,
    },
}

impl DistrictPolicyError {
    /// Stable machine-readable code for the shell and smoke output.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidCenter { .. } => "invalid_center",
            Self::CoordinateOutOfRange { .. } => "coordinate_out_of_range",
            Self::InvalidRadius { .. } => "invalid_radius",
            Self::InvalidTaxMultiplier { .. } => "invalid_tax_multiplier",
            Self::InvalidServiceFunding { .. } => "invalid_service_funding",
            Self::EmptyName => "empty_name",
            Self::NameTooLong { .. } => "name_too_long",
            Self::InvalidName => "invalid_name",
            Self::InvalidAllowlistEntry { .. } => "invalid_allowlist_entry",
            Self::TooManyAllowlistEntries { .. } => "too_many_allowlist_entries",
            Self::Full => "full",
            Self::NotFound { .. } => "not_found",
            Self::IdExhausted => "id_exhausted",
            Self::DuplicateId { .. } => "duplicate_id",
            Self::InvalidRecord { .. } => "invalid_record",
            Self::InvalidNextId { .. } => "invalid_next_id",
            Self::InvalidJson(_) => "invalid_json",
            Self::UnsupportedVersion { .. } => "unsupported_version",
            Self::SeedMismatch { .. } => "seed_mismatch",
        }
    }
}

impl fmt::Display for DistrictPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCenter { center } => {
                write!(formatter, "district centre {center:?} is not finite")
            }
            Self::CoordinateOutOfRange { axis, value } => write!(
                formatter,
                "district {axis} coordinate {value} is outside ±{MAX_WORLD_COORD_M} m"
            ),
            Self::InvalidRadius { radius_m } => write!(
                formatter,
                "district radius {radius_m} m is outside {MIN_DISTRICT_POLICY_RADIUS_M}..={MAX_DISTRICT_POLICY_RADIUS_M} m"
            ),
            Self::InvalidTaxMultiplier { value } => write!(
                formatter,
                "district tax multiplier {value} is outside {MIN_DISTRICT_TAX_MULTIPLIER}..={MAX_DISTRICT_TAX_MULTIPLIER}"
            ),
            Self::InvalidServiceFunding { value } => write!(
                formatter,
                "district service funding {value} is outside 0..={MAX_DISTRICT_SERVICE_FUNDING}"
            ),
            Self::EmptyName => formatter.write_str("district name is empty"),
            Self::NameTooLong { limit } => {
                write!(formatter, "district name exceeds {limit} characters")
            }
            Self::InvalidName => formatter.write_str("district name is invalid"),
            Self::InvalidAllowlistEntry { entry } => write!(
                formatter,
                "blueprint allowlist entry {entry:?} must match ^[a-z0-9_]+:[a-z0-9_./-]+$"
            ),
            Self::TooManyAllowlistEntries { count } => write!(
                formatter,
                "blueprint allowlist has {count} entries; the limit is {MAX_DISTRICT_POLICY_ALLOWLIST}"
            ),
            Self::Full => write!(
                formatter,
                "district policy limit of {MAX_DISTRICT_POLICIES} reached"
            ),
            Self::NotFound { id } => write!(formatter, "district policy {id} does not exist"),
            Self::IdExhausted => formatter.write_str("district policy id space is exhausted"),
            Self::DuplicateId { id } => write!(formatter, "duplicate district policy id {id}"),
            Self::InvalidRecord { id, reason } => {
                write!(formatter, "district policy {id} is invalid: {reason}")
            }
            Self::InvalidNextId { next_id } => {
                write!(formatter, "district policy next_id {next_id} is invalid")
            }
            Self::InvalidJson(message) => {
                write!(formatter, "invalid district policy JSON: {message}")
            }
            Self::UnsupportedVersion { version } => {
                write!(formatter, "unsupported district policy version {version}")
            }
            Self::SeedMismatch { expected, found } => write!(
                formatter,
                "district policy registry seed {found} does not match the world seed {expected}"
            ),
        }
    }
}

impl Error for DistrictPolicyError {}

/// Persistent district-policy registry.
///
/// The store is additive: when no registry exists the world save omits the
/// optional `policies` object, so old saves and their checksums do not move.
/// It owns a `seed` so the save/load path can reject a registry that belongs
/// to another world.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DistrictPolicyStore {
    /// Save format version.
    pub format_version: u16,
    /// World seed this registry belongs to.
    pub seed: u64,
    /// Next id to assign.
    pub next_id: u64,
    /// Policies keyed by id, iterated in ascending id order.
    pub policies: BTreeMap<u64, DistrictPolicy>,
}

impl DistrictPolicyStore {
    /// Create an empty registry for one seed.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self {
            format_version: DISTRICT_POLICY_FORMAT_VERSION,
            seed,
            next_id: 1,
            policies: BTreeMap::new(),
        }
    }

    /// Number of stored policies.
    #[must_use]
    pub fn count(&self) -> usize {
        self.policies.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.policies.is_empty()
    }

    /// Look up one policy by id.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&DistrictPolicy> {
        self.policies.get(&id)
    }

    /// Policies in ascending id order.
    pub fn list(&self) -> impl Iterator<Item = &DistrictPolicy> {
        self.policies.values()
    }

    /// Validate and insert a new policy.
    ///
    /// The store assigns the id. Validation happens before the insert, so a
    /// failed create leaves the registry unchanged.
    ///
    /// # Errors
    ///
    /// Returns an invalid-input error, [`DistrictPolicyError::Full`] when the
    /// cap is reached, or [`DistrictPolicyError::IdExhausted`] when no id
    /// remains.
    pub fn create(
        &mut self,
        request: &DistrictPolicyRequest,
    ) -> Result<DistrictPolicy, DistrictPolicyError> {
        let policy = policy_from_request(0, request)?;
        if self.policies.len() >= MAX_DISTRICT_POLICIES {
            return Err(DistrictPolicyError::Full);
        }
        if self.next_id >= MAX_DISTRICT_POLICY_ID {
            return Err(DistrictPolicyError::IdExhausted);
        }
        let id = self.next_id;
        let mut policy = policy;
        policy.id = id;
        self.next_id += 1;
        self.policies.insert(id, policy.clone());
        Ok(policy)
    }

    /// Replace the fields of an existing policy without changing its id.
    ///
    /// # Errors
    ///
    /// Returns [`DistrictPolicyError::NotFound`] when `id` is absent, or an
    /// invalid-input error without mutating the registry.
    pub fn update(
        &mut self,
        id: u64,
        request: &DistrictPolicyRequest,
    ) -> Result<DistrictPolicy, DistrictPolicyError> {
        if !self.policies.contains_key(&id) {
            return Err(DistrictPolicyError::NotFound { id });
        }
        let policy = policy_from_request(id, request)?;
        self.policies.insert(id, policy.clone());
        Ok(policy)
    }

    /// Remove one policy by id. Returns `false` when the id is absent.
    pub fn remove(&mut self, id: u64) -> bool {
        self.policies.remove(&id).is_some()
    }

    /// Smallest-id policy whose inclusive circle contains `point`.
    ///
    /// `BTreeMap` iteration is ascending, so the first match is the smallest
    /// id. Overlapping policies are allowed; this helper prefers the older id
    /// rather than the smallest or nearest circle. A non-finite point returns
    /// `None`.
    #[must_use]
    pub fn policy_at(&self, point: [f32; 2]) -> Option<&DistrictPolicy> {
        if !point.iter().all(|value| value.is_finite()) {
            return None;
        }
        self.policies.values().find(|policy| policy.contains(point))
    }

    /// Ids of stored policies whose circles intersect `policy`, ascending.
    ///
    /// The policy itself is excluded by id. Touching circles count as
    /// overlapping. This is a query helper; the registry does not reject
    /// overlaps on create or update.
    #[must_use]
    pub fn overlaps(&self, policy: &DistrictPolicy) -> Vec<u64> {
        self.policies
            .values()
            .filter(|other| other.id != policy.id && other.overlaps(policy))
            .map(|other| other.id)
            .collect()
    }

    /// Validate the format version, counter, record ids, and every field.
    ///
    /// # Errors
    ///
    /// Returns the first invalid record or an unsupported format version.
    pub fn validate(&self) -> Result<(), DistrictPolicyError> {
        if self.format_version != DISTRICT_POLICY_FORMAT_VERSION {
            return Err(DistrictPolicyError::UnsupportedVersion {
                version: self.format_version,
            });
        }
        if self.policies.len() > MAX_DISTRICT_POLICIES {
            return Err(DistrictPolicyError::Full);
        }
        let mut max_id = 0;
        for (&id, policy) in &self.policies {
            if id == 0 || id >= MAX_DISTRICT_POLICY_ID {
                return Err(DistrictPolicyError::InvalidRecord {
                    id,
                    reason: "id is out of range",
                });
            }
            if policy.id != id {
                return Err(DistrictPolicyError::InvalidRecord {
                    id,
                    reason: "record id does not match its map key",
                });
            }
            validate_policy(policy).map_err(|error| DistrictPolicyError::InvalidRecord {
                id,
                reason: record_reason(&error),
            })?;
            max_id = max_id.max(id);
        }
        if self.next_id == 0 || self.next_id <= max_id || self.next_id > MAX_DISTRICT_POLICY_ID {
            return Err(DistrictPolicyError::InvalidNextId {
                next_id: self.next_id,
            });
        }
        Ok(())
    }

    /// Stable FNV-1a checksum over the compact JSON representation.
    ///
    /// The checksum covers the format version, seed, next id, and every policy
    /// field. It is intended for deterministic bridge payloads and regression
    /// comparisons within one build, not for cryptographic use.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let bytes =
            serde_json::to_vec(self).expect("district policy store serialization cannot fail");
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    /// Serialize the registry as pretty JSON.
    ///
    /// # Errors
    ///
    /// Returns [`DistrictPolicyError::InvalidJson`] if serialization fails.
    pub fn to_json_pretty(&self) -> Result<String, DistrictPolicyError> {
        serde_json::to_string_pretty(self)
            .map_err(|error| DistrictPolicyError::InvalidJson(error.to_string()))
    }

    /// Parse and validate a registry.
    ///
    /// # Errors
    ///
    /// Returns [`DistrictPolicyError::InvalidJson`] for malformed JSON, or the
    /// first validation error.
    pub fn from_json(json: &str) -> Result<Self, DistrictPolicyError> {
        let store: Self = serde_json::from_str(json)
            .map_err(|error| DistrictPolicyError::InvalidJson(error.to_string()))?;
        store.validate()?;
        Ok(store)
    }

    /// Parse a registry and require it to belong to `seed`.
    ///
    /// # Errors
    ///
    /// Returns every [`Self::from_json`] error plus
    /// [`DistrictPolicyError::SeedMismatch`] when the JSON seed differs.
    pub fn from_json_for_seed(json: &str, seed: u64) -> Result<Self, DistrictPolicyError> {
        let store = Self::from_json(json)?;
        if store.seed != seed {
            return Err(DistrictPolicyError::SeedMismatch {
                expected: seed,
                found: store.seed,
            });
        }
        Ok(store)
    }
}

/// Build a canonical policy from a request. The name is trimmed here.
fn policy_from_request(
    id: u64,
    request: &DistrictPolicyRequest,
) -> Result<DistrictPolicy, DistrictPolicyError> {
    let policy = DistrictPolicy {
        id,
        name: request.name.trim().to_string(),
        center: request.center,
        radius_m: request.radius_m,
        tax_multiplier: request.tax_multiplier,
        service_funding: request.service_funding,
        construction_allowed: request.construction_allowed,
        blueprint_allowlist: request.blueprint_allowlist.clone(),
    };
    validate_policy(&policy)?;
    Ok(policy)
}

/// Validate every policy field. The name must already be trimmed.
fn validate_policy(policy: &DistrictPolicy) -> Result<(), DistrictPolicyError> {
    if policy.center.iter().any(|value| !value.is_finite()) {
        return Err(DistrictPolicyError::InvalidCenter {
            center: policy.center,
        });
    }
    for (axis, value) in [("x", policy.center[0]), ("z", policy.center[1])] {
        if value.abs() > MAX_WORLD_COORD_M {
            return Err(DistrictPolicyError::CoordinateOutOfRange { axis, value });
        }
    }
    if !policy.radius_m.is_finite()
        || !(MIN_DISTRICT_POLICY_RADIUS_M..=MAX_DISTRICT_POLICY_RADIUS_M).contains(&policy.radius_m)
    {
        return Err(DistrictPolicyError::InvalidRadius {
            radius_m: policy.radius_m,
        });
    }
    if !policy.tax_multiplier.is_finite()
        || !(MIN_DISTRICT_TAX_MULTIPLIER..=MAX_DISTRICT_TAX_MULTIPLIER)
            .contains(&policy.tax_multiplier)
    {
        return Err(DistrictPolicyError::InvalidTaxMultiplier {
            value: policy.tax_multiplier,
        });
    }
    if !policy.service_funding.is_finite()
        || !(0.0..=MAX_DISTRICT_SERVICE_FUNDING).contains(&policy.service_funding)
    {
        return Err(DistrictPolicyError::InvalidServiceFunding {
            value: policy.service_funding,
        });
    }

    if policy.name != policy.name.trim() {
        return Err(DistrictPolicyError::InvalidName);
    }
    if policy.name.is_empty() {
        return Err(DistrictPolicyError::EmptyName);
    }
    if policy.name.chars().any(char::is_control) {
        return Err(DistrictPolicyError::InvalidName);
    }
    if policy.name.chars().count() > MAX_DISTRICT_POLICY_NAME_CHARS {
        return Err(DistrictPolicyError::NameTooLong {
            limit: MAX_DISTRICT_POLICY_NAME_CHARS,
        });
    }

    if policy.blueprint_allowlist.len() > MAX_DISTRICT_POLICY_ALLOWLIST {
        return Err(DistrictPolicyError::TooManyAllowlistEntries {
            count: policy.blueprint_allowlist.len(),
        });
    }
    for entry in &policy.blueprint_allowlist {
        if !is_valid_content_id(entry) {
            return Err(DistrictPolicyError::InvalidAllowlistEntry {
                entry: entry.clone(),
            });
        }
    }
    Ok(())
}

/// Short stable reason for a loaded record.
fn record_reason(error: &DistrictPolicyError) -> &'static str {
    match error {
        DistrictPolicyError::InvalidCenter { .. } => "invalid centre",
        DistrictPolicyError::CoordinateOutOfRange { .. } => "centre out of range",
        DistrictPolicyError::InvalidRadius { .. } => "invalid radius",
        DistrictPolicyError::InvalidTaxMultiplier { .. } => "invalid tax multiplier",
        DistrictPolicyError::InvalidServiceFunding { .. } => "invalid service funding",
        DistrictPolicyError::EmptyName => "empty name",
        DistrictPolicyError::NameTooLong { .. } => "name too long",
        DistrictPolicyError::InvalidName => "invalid name",
        DistrictPolicyError::InvalidAllowlistEntry { .. } => "invalid allowlist entry",
        DistrictPolicyError::TooManyAllowlistEntries { .. } => "too many allowlist entries",
        _ => "invalid record",
    }
}

/// Whether `id` matches `^[a-z0-9_]+:[a-z0-9_./-]+$`.
///
/// This is the shared content-ID pattern used by the registry and manifests.
fn is_valid_content_id(id: &str) -> bool {
    let Some((namespace, path)) = id.split_once(':') else {
        return false;
    };
    if namespace.is_empty() || path.is_empty() || path.contains(':') {
        return false;
    }
    namespace
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        && path.bytes().all(is_path_byte)
}

fn is_path_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'.' | b'/' | b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(name: &str, x: f32, z: f32, radius_m: f32) -> DistrictPolicyRequest {
        DistrictPolicyRequest {
            name: name.to_string(),
            center: [x, z],
            radius_m,
            tax_multiplier: 1.0,
            service_funding: 1.0,
            construction_allowed: true,
            blueprint_allowlist: BTreeSet::new(),
        }
    }

    fn store_with_one() -> (DistrictPolicyStore, DistrictPolicy) {
        let mut store = DistrictPolicyStore::new(2026);
        let policy = store
            .create(&request("Riverside", 100.0, 100.0, 64.0))
            .expect("create");
        (store, policy)
    }

    #[test]
    fn create_assigns_ids_and_lists_in_order() {
        let mut store = DistrictPolicyStore::new(2026);
        assert_eq!(store.count(), 0);
        assert!(store.is_empty());

        let first = store
            .create(&request("Riverside", 100.0, 100.0, 64.0))
            .expect("first");
        let second = store
            .create(&request("Old Town", -100.0, 50.0, 32.0))
            .expect("second");
        assert_eq!(first.id, 1);
        assert_eq!(second.id, 2);
        assert_eq!(store.next_id, 3);
        assert_eq!(store.count(), 2);
        assert!(!store.is_empty());
        assert_eq!(store.get(1), Some(&first));
        assert_eq!(store.get(2), Some(&second));
        assert!(store.get(3).is_none());
        assert_eq!(
            store.list().map(|policy| policy.id).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(
            store.list().next().map(|policy| policy.name.as_str()),
            Some("Riverside")
        );
    }

    #[test]
    fn update_keeps_the_id_and_replaces_fields() {
        let (mut store, original) = store_with_one();
        let mut changed = request("North Industrial", -200.0, -100.0, 512.0);
        changed.tax_multiplier = 2.5;
        changed.service_funding = 0.5;
        changed.construction_allowed = false;
        changed
            .blueprint_allowlist
            .insert("base:workshop".to_string());

        let updated = store.update(original.id, &changed).expect("update");
        assert_eq!(updated.id, original.id);
        assert_eq!(updated.name, "North Industrial");
        assert_eq!(updated.center, [-200.0, -100.0]);
        assert_eq!(updated.radius_m, 512.0);
        assert_eq!(updated.tax_multiplier, 2.5);
        assert_eq!(updated.service_funding, 0.5);
        assert!(!updated.construction_allowed);
        assert_eq!(store.get(original.id), Some(&updated));
        assert_eq!(store.count(), 1);
        assert_eq!(store.next_id, 2);
    }

    #[test]
    fn update_and_remove_missing_ids_return_not_found() {
        let (mut store, _) = store_with_one();
        let mut missing = store.update(99, &request("Missing", 0.0, 0.0, 16.0));
        assert_eq!(missing, Err(DistrictPolicyError::NotFound { id: 99 }));
        // A failed update did not create a record.
        assert_eq!(store.count(), 1);

        // The recovered value proves the error is not sticky.
        missing = store.update(1, &request("Renamed", 0.0, 0.0, 16.0));
        assert!(missing.is_ok());

        assert!(!store.remove(99));
        assert!(store.remove(1));
        assert!(!store.remove(1));
        assert!(store.is_empty());
    }

    #[test]
    fn names_are_trimmed_on_create_and_update() {
        let mut store = DistrictPolicyStore::new(2026);
        let created = store
            .create(&request("  Riverside  ", 0.0, 0.0, 16.0))
            .expect("create");
        assert_eq!(created.name, "Riverside");
        let updated = store
            .update(created.id, &request("\tOld Town\n", 0.0, 0.0, 16.0))
            .expect("update");
        assert_eq!(updated.name, "Old Town");
    }

    #[test]
    fn id_counter_stops_before_i64_max() {
        let mut store = DistrictPolicyStore::new(2026);
        store.next_id = MAX_DISTRICT_POLICY_ID - 1;
        let last = store
            .create(&request("Last", 0.0, 0.0, 16.0))
            .expect("last id");
        assert_eq!(last.id, i64::MAX as u64 - 1);
        assert_eq!(store.next_id, MAX_DISTRICT_POLICY_ID);
        store.validate().expect("exhausted registry is valid");

        assert_eq!(
            store.create(&request("None left", 0.0, 0.0, 16.0)),
            Err(DistrictPolicyError::IdExhausted)
        );
        assert_eq!(store.next_id, MAX_DISTRICT_POLICY_ID);
        assert_eq!(store.count(), 1);
    }

    #[test]
    fn validation_rejects_every_out_of_range_field() {
        let mut cases: Vec<(DistrictPolicyRequest, &str)> = Vec::new();

        for radius in [
            MIN_DISTRICT_POLICY_RADIUS_M - 0.01,
            MAX_DISTRICT_POLICY_RADIUS_M + 0.01,
            f32::NAN,
            f32::INFINITY,
        ] {
            cases.push((request("Radius", 0.0, 0.0, radius), "invalid_radius"));
        }
        for tax in [
            MIN_DISTRICT_TAX_MULTIPLIER - 0.01,
            MAX_DISTRICT_TAX_MULTIPLIER + 0.01,
            f32::NAN,
            f32::INFINITY,
        ] {
            let mut request = request("Tax", 0.0, 0.0, 16.0);
            request.tax_multiplier = tax;
            cases.push((request, "invalid_tax_multiplier"));
        }
        for funding in [
            -0.01,
            MAX_DISTRICT_SERVICE_FUNDING + 0.01,
            f32::NAN,
            f32::INFINITY,
        ] {
            let mut request = request("Funding", 0.0, 0.0, 16.0);
            request.service_funding = funding;
            cases.push((request, "invalid_service_funding"));
        }
        for center in [
            [f32::NAN, 0.0],
            [0.0, f32::INFINITY],
            [MAX_WORLD_COORD_M + 1.0, 0.0],
            [0.0, -MAX_WORLD_COORD_M - 1.0],
        ] {
            let mut request = request("Center", 0.0, 0.0, 16.0);
            request.center = center;
            let code = if center.iter().any(|value| !value.is_finite()) {
                "invalid_center"
            } else {
                "coordinate_out_of_range"
            };
            cases.push((request, code));
        }

        let mut store = DistrictPolicyStore::new(2026);
        for (request, expected) in cases {
            let error = store.create(&request).expect_err("invalid request");
            assert_eq!(error.code(), expected, "request {request:?}");
            assert!(store.is_empty(), "a failed create inserted a record");
            assert_eq!(store.next_id, 1, "a failed create advanced the counter");
        }
    }

    #[test]
    fn name_and_allowlist_limits_are_enforced() {
        let mut store = DistrictPolicyStore::new(2026);

        assert_eq!(
            store.create(&request("   ", 0.0, 0.0, 16.0)),
            Err(DistrictPolicyError::EmptyName)
        );
        assert_eq!(
            store.create(&request("", 0.0, 0.0, 16.0)),
            Err(DistrictPolicyError::EmptyName)
        );
        let long = "a".repeat(MAX_DISTRICT_POLICY_NAME_CHARS + 1);
        assert_eq!(
            store.create(&request(&long, 0.0, 0.0, 16.0)),
            Err(DistrictPolicyError::NameTooLong {
                limit: MAX_DISTRICT_POLICY_NAME_CHARS
            })
        );
        assert_eq!(
            store.create(&request("line\nbreak", 0.0, 0.0, 16.0)),
            Err(DistrictPolicyError::InvalidName)
        );

        for entry in [
            "NoColon",
            "Base:house",
            "base:",
            ":house",
            "base:House",
            "base:house upper",
            "base:two:colons",
            "base/house",
        ] {
            let mut invalid = request("Allowlist", 0.0, 0.0, 16.0);
            invalid.blueprint_allowlist.insert(entry.to_string());
            assert_eq!(
                store.create(&invalid),
                Err(DistrictPolicyError::InvalidAllowlistEntry {
                    entry: entry.to_string()
                }),
                "entry {entry:?}"
            );
        }

        let mut too_many = request("Allowlist", 0.0, 0.0, 16.0);
        for index in 0..=MAX_DISTRICT_POLICY_ALLOWLIST {
            too_many
                .blueprint_allowlist
                .insert(format!("base:house_{index}"));
        }
        assert_eq!(
            store.create(&too_many),
            Err(DistrictPolicyError::TooManyAllowlistEntries {
                count: MAX_DISTRICT_POLICY_ALLOWLIST + 1
            })
        );
    }

    #[test]
    fn boundary_values_are_accepted_and_then_round_trip() {
        let mut store = DistrictPolicyStore::new(2026);
        let long_name = "n".repeat(MAX_DISTRICT_POLICY_NAME_CHARS);
        let mut boundary = request(&long_name, 0.0, 0.0, 16.0);
        boundary.center = [-MAX_WORLD_COORD_M, MAX_WORLD_COORD_M];
        boundary.radius_m = MAX_DISTRICT_POLICY_RADIUS_M;
        boundary.tax_multiplier = MIN_DISTRICT_TAX_MULTIPLIER;
        boundary.service_funding = MAX_DISTRICT_SERVICE_FUNDING;
        boundary
            .blueprint_allowlist
            .insert("base:oak_plank".to_string());
        boundary
            .blueprint_allowlist
            .insert("example:pack/v2-house_a".to_string());
        let policy = store.create(&boundary).expect("boundary values");
        assert_eq!(policy.center, [-MAX_WORLD_COORD_M, MAX_WORLD_COORD_M]);
        assert_eq!(policy.radius_m, MAX_DISTRICT_POLICY_RADIUS_M);
        assert_eq!(policy.tax_multiplier, MIN_DISTRICT_TAX_MULTIPLIER);
        assert_eq!(policy.service_funding, MAX_DISTRICT_SERVICE_FUNDING);
        store.validate().expect("boundary registry is valid");

        // The other end of each range.
        let mut ok = request("Minimum", 0.0, 0.0, MIN_DISTRICT_POLICY_RADIUS_M);
        ok.tax_multiplier = MAX_DISTRICT_TAX_MULTIPLIER;
        ok.service_funding = 0.0;
        store.create(&ok).expect("minimum values");
    }

    #[test]
    fn exact_allowlist_cap_is_accepted() {
        let mut store = DistrictPolicyStore::new(2026);
        let mut request = request("Allowlist", 0.0, 0.0, 16.0);
        for index in 0..MAX_DISTRICT_POLICY_ALLOWLIST {
            request
                .blueprint_allowlist
                .insert(format!("base:house_{index}"));
        }
        let policy = store.create(&request).expect("256 entries");
        assert_eq!(
            policy.blueprint_allowlist.len(),
            MAX_DISTRICT_POLICY_ALLOWLIST
        );
    }

    #[test]
    fn policy_at_returns_the_smallest_containing_id() {
        let mut store = DistrictPolicyStore::new(2026);
        let outer = store
            .create(&request("Outer", 0.0, 0.0, 100.0))
            .expect("outer");
        let inner = store
            .create(&request("Inner", 0.0, 0.0, 50.0))
            .expect("inner");
        assert_eq!(outer.id, 1);
        assert_eq!(inner.id, 2);

        // Both contain the origin; the older, larger outer policy wins.
        assert_eq!(store.policy_at([0.0, 0.0]).map(|policy| policy.id), Some(1));
        // Only the outer circle contains (60, 0).
        assert_eq!(
            store.policy_at([60.0, 0.0]).map(|policy| policy.id),
            Some(1)
        );
        // Only the inner circle contains (40, 0), and the outer also does;
        // the smallest id still wins.
        assert_eq!(
            store.policy_at([40.0, 0.0]).map(|policy| policy.id),
            Some(1)
        );
        assert!(store.policy_at([200.0, 0.0]).is_none());
        assert!(store.policy_at([f32::NAN, 0.0]).is_none());
        assert!(store.policy_at([0.0, f32::INFINITY]).is_none());

        // Removing the older policy promotes the inner one.
        assert!(store.remove(1));
        assert_eq!(store.policy_at([0.0, 0.0]).map(|policy| policy.id), Some(2));
    }

    #[test]
    fn containment_is_inclusive_on_the_radius() {
        let (_, policy) = store_with_one();
        let mut circle = policy;
        circle.center = [0.0, 0.0];
        circle.radius_m = 100.0;

        assert!(circle.contains([0.0, 0.0]));
        assert!(circle.contains([100.0, 0.0]));
        assert!(circle.contains([-100.0, 0.0]));
        assert!(circle.contains([0.0, 100.0]));
        // A 3-4-5 triangle scaled to a 100 m radius.
        assert!(circle.contains([60.0, 80.0]));
        assert!(!circle.contains([100.01, 0.0]));
        assert!(!circle.contains([60.0, 80.01]));
        assert!(!circle.contains([f32::NAN, 0.0]));
    }

    #[test]
    fn overlaps_helper_finds_touching_and_intersecting_circles() {
        let mut store = DistrictPolicyStore::new(2026);
        let first = store
            .create(&request("First", 0.0, 0.0, 20.0))
            .expect("first");
        let touching = store
            .create(&request("Touching", 40.0, 0.0, 20.0))
            .expect("touching");
        let apart = store
            .create(&request("Apart", 100.0, 0.0, 20.0))
            .expect("apart");

        assert!(first.overlaps(&touching));
        assert!(touching.overlaps(&first));
        assert!(!first.overlaps(&apart));
        assert_eq!(store.overlaps(&first), vec![touching.id]);
        assert_eq!(store.overlaps(&touching), vec![first.id]);
        assert!(store.overlaps(&apart).is_empty());
        // The helper excludes the queried policy even when it overlaps itself.
        assert_eq!(
            store.overlaps(store.get(first.id).expect("first")),
            vec![touching.id]
        );
    }

    #[test]
    fn checksum_is_deterministic_and_data_sensitive() {
        let (store, _) = store_with_one();
        let checksum = store.checksum();
        assert_eq!(checksum, store.clone().checksum());
        assert_ne!(checksum, 0);

        let mut changed_tax = store.clone();
        changed_tax
            .policies
            .get_mut(&1)
            .expect("policy")
            .tax_multiplier = 2.0;
        assert_ne!(checksum, changed_tax.checksum());

        let mut changed_next = store.clone();
        changed_next.next_id = 5;
        assert_ne!(checksum, changed_next.checksum());

        let mut changed_seed = store;
        changed_seed.seed = 2027;
        assert_ne!(checksum, changed_seed.checksum());
    }

    #[test]
    fn json_round_trip_preserves_records_and_checksum() {
        let mut store = DistrictPolicyStore::new(2026);
        let first = store
            .create(&request("Riverside", 100.0, 100.0, 64.0))
            .expect("first");
        let mut second = request("Old Town", -100.0, 50.0, 32.0);
        second.service_funding = 1.75;
        second.construction_allowed = false;
        second.blueprint_allowlist.insert("base:house".to_string());
        second.blueprint_allowlist.insert("base:shop".to_string());
        let second = store.create(&second).expect("second");
        let checksum = store.checksum();

        let json = store.to_json_pretty().expect("json");
        let loaded = DistrictPolicyStore::from_json(&json).expect("load");
        assert_eq!(loaded, store);
        assert_eq!(loaded.checksum(), checksum);
        assert_eq!(loaded.get(1), Some(&first));
        assert_eq!(loaded.get(2), Some(&second));
        assert_eq!(
            loaded
                .get(2)
                .expect("second")
                .blueprint_allowlist
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["base:house", "base:shop"]
        );
    }

    #[test]
    fn json_load_checks_the_seed_and_version() {
        let (store, _) = store_with_one();
        let json = store.to_json_pretty().expect("json");
        assert_eq!(
            DistrictPolicyStore::from_json_for_seed(&json, 2027),
            Err(DistrictPolicyError::SeedMismatch {
                expected: 2027,
                found: 2026
            })
        );
        assert!(DistrictPolicyStore::from_json_for_seed(&json, 2026).is_ok());

        let newer = json.replace("\"format_version\": 1", "\"format_version\": 2");
        assert_eq!(
            DistrictPolicyStore::from_json(&newer)
                .expect_err("version")
                .code(),
            "unsupported_version"
        );
    }

    #[test]
    fn load_rejects_untrimmed_and_inconsistent_records() {
        let (store, _) = store_with_one();

        let mut padded = store.clone();
        padded.policies.get_mut(&1).expect("policy").name = "Riverside ".to_string();
        assert_eq!(
            padded.validate(),
            Err(DistrictPolicyError::InvalidRecord {
                id: 1,
                reason: "invalid name"
            })
        );

        let mut mismatched = store.clone();
        mismatched.policies.get_mut(&1).expect("policy").id = 2;
        assert_eq!(
            mismatched.validate(),
            Err(DistrictPolicyError::InvalidRecord {
                id: 1,
                reason: "record id does not match its map key"
            })
        );

        let mut max = store.clone();
        max.policies
            .insert(MAX_DISTRICT_POLICY_ID, max.policies[&1].clone());
        max.policies
            .get_mut(&MAX_DISTRICT_POLICY_ID)
            .expect("policy")
            .id = MAX_DISTRICT_POLICY_ID;
        assert_eq!(
            max.validate(),
            Err(DistrictPolicyError::InvalidRecord {
                id: MAX_DISTRICT_POLICY_ID,
                reason: "id is out of range"
            })
        );

        let mut next = store.clone();
        next.next_id = 1;
        assert_eq!(
            next.validate(),
            Err(DistrictPolicyError::InvalidNextId { next_id: 1 })
        );

        let mut zero_next = store;
        zero_next.next_id = 0;
        assert_eq!(
            zero_next.validate(),
            Err(DistrictPolicyError::InvalidNextId { next_id: 0 })
        );
    }

    #[test]
    fn load_rejects_invalid_fields_with_a_record_reason() {
        type Mutation = fn(&mut DistrictPolicy);
        let mutations: [(Mutation, &'static str); 4] = [
            (|policy| policy.radius_m = 1.0, "invalid radius"),
            (
                |policy| policy.tax_multiplier = 5.0,
                "invalid tax multiplier",
            ),
            (
                |policy| policy.service_funding = -1.0,
                "invalid service funding",
            ),
            (
                |policy| {
                    policy.blueprint_allowlist.insert("bad".to_string());
                },
                "invalid allowlist entry",
            ),
        ];
        for (mutate, reason) in mutations {
            let (mut store, _) = store_with_one();
            mutate(store.policies.get_mut(&1).expect("policy"));
            assert_eq!(
                store.validate(),
                Err(DistrictPolicyError::InvalidRecord { id: 1, reason })
            );
        }
    }

    #[test]
    fn empty_registry_is_valid_and_its_checksum_is_stable() {
        let store = DistrictPolicyStore::new(2026);
        store.validate().expect("empty registry");
        assert_eq!(store.count(), 0);
        assert!(store.list().next().is_none());
        assert_eq!(store.checksum(), DistrictPolicyStore::new(2026).checksum());
        assert_ne!(store.checksum(), DistrictPolicyStore::new(2027).checksum());
    }

    #[test]
    fn capacity_is_limited_to_4096() {
        let mut store = DistrictPolicyStore::new(2026);
        for index in 0..MAX_DISTRICT_POLICIES {
            store
                .create(&request(&format!("District {index}"), 0.0, 0.0, 16.0))
                .expect("create under the cap");
        }
        assert_eq!(store.count(), MAX_DISTRICT_POLICIES);
        store.validate().expect("full registry is valid");
        assert_eq!(
            store.create(&request("Overflow", 0.0, 0.0, 16.0)),
            Err(DistrictPolicyError::Full)
        );
        assert_eq!(store.count(), MAX_DISTRICT_POLICIES);
        assert_eq!(store.next_id, MAX_DISTRICT_POLICIES as u64 + 1);
    }

    #[test]
    fn failed_update_does_not_mutate_the_record() {
        let (mut store, original) = store_with_one();
        let invalid = request("Changed", 0.0, 0.0, 1.0);
        assert_eq!(
            store.update(original.id, &invalid),
            Err(DistrictPolicyError::InvalidRadius { radius_m: 1.0 })
        );
        assert_eq!(store.get(original.id), Some(&original));
    }

    fn placement_policy(id: u64, construction_allowed: bool, allowlist: &[&str]) -> DistrictPolicy {
        DistrictPolicy {
            id,
            name: format!("District {id}"),
            center: [0.0, 0.0],
            radius_m: 64.0,
            tax_multiplier: 1.0,
            service_funding: 1.0,
            construction_allowed,
            blueprint_allowlist: allowlist.iter().map(|id| (*id).to_string()).collect(),
        }
    }

    #[test]
    fn placement_denial_codes_are_pinned() {
        assert_eq!(
            PolicyPlacementDenial::ConstructionDisabled.code(),
            "policy_construction_disabled"
        );
        assert_eq!(
            PolicyPlacementDenial::BlueprintNotAllowed.code(),
            "policy_blueprint_not_allowed"
        );
    }

    #[test]
    fn construction_disabled_denies_every_blueprint() {
        let empty = placement_policy(1, false, &[]);
        assert_eq!(
            policy_placement_denial(&empty, "townsmith:hut"),
            Some(PolicyPlacementDenial::ConstructionDisabled)
        );
        assert!(!policy_allows_placement(&empty, "townsmith:hut"));

        // The construction switch wins over the allowlist, including for a
        // blueprint the policy explicitly lists.
        let allowlisted = placement_policy(2, false, &["townsmith:hut"]);
        assert_eq!(
            policy_placement_denial(&allowlisted, "townsmith:hut"),
            Some(PolicyPlacementDenial::ConstructionDisabled)
        );
        assert!(!policy_allows_placement(&allowlisted, "townsmith:hut"));
    }

    #[test]
    fn non_empty_allowlist_permits_only_listed_ids() {
        let policy = placement_policy(1, true, &["townsmith:hut", "townsmith:shop"]);
        assert!(policy_allows_placement(&policy, "townsmith:hut"));
        assert!(policy_allows_placement(&policy, "townsmith:shop"));

        // Exact and case-sensitive: no prefix, case, or namespace slack.
        for denied in [
            "townsmith:house",
            "townsmith:hut.2",
            "Townsmith:Hut",
            "townsmith:h",
            "",
        ] {
            assert_eq!(
                policy_placement_denial(&policy, denied),
                Some(PolicyPlacementDenial::BlueprintNotAllowed),
                "{denied:?} must be denied"
            );
            assert!(!policy_allows_placement(&policy, denied));
        }
    }

    #[test]
    fn empty_allowlist_permits_every_blueprint() {
        let policy = placement_policy(1, true, &[]);
        for allowed in [
            "townsmith:hut",
            "townsmith:shop",
            "core:anvil",
            "not-a-namespaced-id",
        ] {
            assert_eq!(policy_placement_denial(&policy, allowed), None);
            assert!(policy_allows_placement(&policy, allowed));
        }
    }

    #[test]
    fn outside_the_circle_is_neutral_through_policy_at() {
        let mut store = DistrictPolicyStore::new(2026);
        let mut deny = request("Riverside", 100.0, 100.0, 64.0);
        deny.construction_allowed = false;
        store.create(&deny).expect("create");

        let inside = store.policy_at([100.0, 100.0]).expect("contained");
        assert_eq!(
            policy_placement_denial(inside, "townsmith:hut"),
            Some(PolicyPlacementDenial::ConstructionDisabled)
        );

        // No containing policy means no rule to apply; the caller permits.
        assert!(store.policy_at([500.0, 500.0]).is_none());
        assert!(store.policy_at([100.0 + 64.0 + 0.5, 100.0]).is_none());
    }

    #[test]
    fn non_enforceable_policy_is_neutral() {
        let mut zero_id = placement_policy(0, false, &["townsmith:hut"]);
        assert!(!zero_id.is_enforceable());

        let mut nan_center = placement_policy(1, false, &[]);
        nan_center.center = [f32::NAN, 0.0];

        let mut infinite_radius = placement_policy(2, false, &[]);
        infinite_radius.radius_m = f32::INFINITY;

        let mut zero_radius = placement_policy(3, false, &[]);
        zero_radius.radius_m = 0.0;

        let mut negative_radius = placement_policy(4, false, &[]);
        negative_radius.radius_m = -64.0;

        for policy in [
            &mut zero_id,
            &mut nan_center,
            &mut infinite_radius,
            &mut zero_radius,
            &mut negative_radius,
        ] {
            assert!(!policy.is_enforceable(), "{policy:?}");
            assert_eq!(policy_placement_denial(policy, "townsmith:hut"), None);
            assert_eq!(policy_placement_denial(policy, "townsmith:other"), None);
            assert!(policy_allows_placement(policy, "townsmith:other"));
        }
    }
}
