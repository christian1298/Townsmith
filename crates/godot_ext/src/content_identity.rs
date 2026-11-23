//! Pure content-identity contract for the bridge world save (D-034 v1).
//!
//! A base-only world save stays outer `format_version: 1` and has no
//! `content` member. A save written while one runtime pack is active is outer
//! `format_version: 2` and carries one top-level `content` object:
//!
//! ```json
//! "content": {
//!   "format_version": 1,
//!   "pack_id": "example:copper_age",
//!   "pack_checksum": "ad4c645869a411ed",
//!   "registry_checksum": "330a538d07805b50"
//! }
//! ```
//!
//! The functions here are pure: they parse or patch JSON strings and never
//! touch a filesystem, the live registry, `core_city::PlacementStore`, or a
//! save path. The bridge calls [`stamp_world_content`] before its durable
//! write and [`verify_world_content`] immediately after reading the JSON,
//! before any store or blueprint package is parsed. Committing the verified
//! identity and keeping it on failure is the caller's responsibility.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Outer world-save version for a base-only world.
pub(crate) const WORLD_SAVE_FORMAT_VERSION_V1: u32 = 1;
/// Outer world-save version for a content-bound world.
pub(crate) const WORLD_SAVE_FORMAT_VERSION_V2: u32 = 2;
/// Supported `content.format_version`.
pub(crate) const CONTENT_IDENTITY_FORMAT_VERSION: u32 = 1;

/// Summary of the live runtime content registry for the save decision.
///
/// `packs` is `0` for the bundled base content and `1` after one
/// `content_load_dir` pack; the runtime loader never stacks packs. The pack
/// fields are empty/zero for a base-only registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveContentIdentity {
    /// Number of packs merged after base (`0` or `1`).
    pub packs: usize,
    /// Manifest ID of the live pack, empty when base only.
    pub pack_id: String,
    /// Stable [`ContentPack::checksum`] of the live pack, zero when base only.
    ///
    /// [`ContentPack::checksum`]: core_city::ContentPack::checksum
    pub pack_checksum: u64,
    /// Stable merged registry checksum of base + pack.
    pub registry_checksum: u64,
    /// Stable checksum of the bundled base registry.
    pub base_checksum: u64,
}

impl ActiveContentIdentity {
    /// Base-only runtime identity.
    #[must_use]
    pub(crate) fn base(registry_checksum: u64, base_checksum: u64) -> Self {
        Self {
            packs: 0,
            pack_id: String::new(),
            pack_checksum: 0,
            registry_checksum,
            base_checksum,
        }
    }

    /// One-pack runtime identity.
    #[must_use]
    pub(crate) fn pack(
        pack_id: impl Into<String>,
        pack_checksum: u64,
        registry_checksum: u64,
        base_checksum: u64,
    ) -> Self {
        Self {
            packs: 1,
            pack_id: pack_id.into(),
            pack_checksum,
            registry_checksum,
            base_checksum,
        }
    }
}

/// The optional top-level `content` object of a content-bound world save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorldContentIdentity {
    /// Identity format version; `1` is the only supported value.
    pub format_version: u32,
    /// Manifest ID of the pack that produced the world, never `base`.
    pub pack_id: String,
    /// Stable pack checksum, serialized as 16 lowercase hex digits.
    #[serde(with = "hex_u64")]
    pub pack_checksum: u64,
    /// Stable merged base + pack registry checksum, 16 lowercase hex digits.
    #[serde(with = "hex_u64")]
    pub registry_checksum: u64,
}

/// Result of matching the recorded identity against the live registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContentIdentityStatus {
    /// A base-only save with no recorded identity.
    Absent,
    /// A v1 save with no identity loaded while a pack is active.
    LegacyUnbound,
    /// A v2 save whose pack ID and both checksums match.
    Match,
}

impl ContentIdentityStatus {
    /// Stable result-field spelling.
    #[must_use]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::LegacyUnbound => "legacy_unbound",
            Self::Match => "match",
        }
    }
}

/// Stable content-identity failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContentIdentityError {
    /// Machine-readable code for logs, tests, and UI mapping.
    pub code: &'static str,
    /// Human-readable detail; names the pack where useful.
    pub message: String,
}

fn identity_error(code: &'static str, message: impl Into<String>) -> ContentIdentityError {
    ContentIdentityError {
        code,
        message: message.into(),
    }
}

/// Successful save-side identity decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StampedWorldContent {
    /// JSON to write; unchanged for a base-only save, outer v2 otherwise.
    pub json: String,
    /// `Absent` or `Match` (`LegacyUnbound` is a load-only result).
    pub status: ContentIdentityStatus,
    /// Identity to commit after the durable write; `None` for base only.
    pub identity: Option<WorldContentIdentity>,
    /// Whether this save recorded a new identity (no prior binding).
    pub new: bool,
}

impl StampedWorldContent {
    /// Whether the written JSON is content-bound.
    #[must_use]
    pub(crate) fn content_bound(&self) -> bool {
        self.identity.is_some()
    }

    /// Save result spelling: `absent`, `new`, or `match`.
    #[must_use]
    pub(crate) fn status_str(&self) -> &'static str {
        match (self.status, self.new) {
            (ContentIdentityStatus::Absent, _) => "absent",
            (ContentIdentityStatus::Match, true) => "new",
            (ContentIdentityStatus::Match, false) => "match",
            (ContentIdentityStatus::LegacyUnbound, _) => "legacy_unbound",
        }
    }

    /// Recorded identity format version, or `0` when the save is base only.
    #[must_use]
    pub(crate) fn format_version(&self) -> u32 {
        self.identity
            .as_ref()
            .map_or(0, |identity| identity.format_version)
    }
}

/// Successful load-side identity verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedWorldContent {
    /// `Absent`, `LegacyUnbound`, or `Match`.
    pub status: ContentIdentityStatus,
    /// Verified identity to commit after the whole load succeeds.
    pub identity: Option<WorldContentIdentity>,
    /// A v1 copy of the save for the existing store parsers.
    ///
    /// `None` means the input was already v1 and must be used as-is (this
    /// keeps the base-only read byte-identical). `Some` is the v2 JSON with
    /// `content` removed and `format_version` set back to `1`.
    pub normalized_json: Option<String>,
    /// Recorded identity format version, or `0` when none is recorded.
    pub format_version: u32,
}

impl VerifiedWorldContent {
    /// Load result spelling: `absent`, `legacy_unbound`, or `match`.
    #[must_use]
    pub(crate) fn status_str(&self) -> &'static str {
        self.status.as_str()
    }
}

/// Decide what a save writes and which identity the bridge may commit.
///
/// # Rules
///
/// - base only, no recorded identity: return the input JSON untouched;
/// - base only, recorded identity: `content_save_requires_pack`;
/// - one pack, recorded identity: the pack ID and both checksums must match;
/// - one pack, no recorded identity: write a new identity;
/// - more than one pack: `content_multiple_packs_unsupported`.
///
/// The content-bound output is deterministic pretty JSON with only
/// `format_version` and the identity fields added; no path is recorded.
///
/// # Errors
///
/// Returns [`ContentIdentityError`] with one of the stable `content_*` codes.
/// On failure the input string, the live registry, and the recorded identity
/// are untouched.
pub(crate) fn stamp_world_content(
    json: &str,
    active: &ActiveContentIdentity,
    recorded: Option<&WorldContentIdentity>,
) -> Result<StampedWorldContent, ContentIdentityError> {
    if active.packs > 1 {
        return Err(identity_error(
            "content_multiple_packs_unsupported",
            format!(
                "the runtime registry has {} active packs; one pack is supported",
                active.packs
            ),
        ));
    }
    if active.packs == 0 {
        if recorded.is_some() {
            return Err(identity_error(
                "content_save_requires_pack",
                "this world is bound to a content pack; load that exact pack before saving",
            ));
        }
        return Ok(StampedWorldContent {
            json: json.to_owned(),
            status: ContentIdentityStatus::Absent,
            identity: None,
            new: false,
        });
    }
    validate_active_pack(active)?;
    let (identity, new) = match recorded {
        Some(recorded) => {
            if recorded.format_version != CONTENT_IDENTITY_FORMAT_VERSION {
                return Err(identity_error(
                    "content_identity_unsupported",
                    format!(
                        "recorded content identity format_version {} is not supported",
                        recorded.format_version
                    ),
                ));
            }
            validate_pack_id(&recorded.pack_id)?;
            if recorded.pack_id != active.pack_id {
                return Err(identity_error(
                    "content_save_pack_mismatch",
                    format!(
                        "this world is bound to pack {} but {} is active",
                        recorded.pack_id, active.pack_id
                    ),
                ));
            }
            if recorded.pack_checksum != active.pack_checksum {
                return Err(identity_error(
                    "content_pack_checksum_mismatch",
                    format!(
                        "pack {} was saved with checksum {:016x} but the active pack is {:016x}",
                        active.pack_id, recorded.pack_checksum, active.pack_checksum
                    ),
                ));
            }
            if recorded.registry_checksum != active.registry_checksum {
                return Err(identity_error(
                    "content_registry_checksum_mismatch",
                    format!(
                        "pack {} was saved with merged registry checksum {:016x} but the live registry is {:016x}",
                        active.pack_id, recorded.registry_checksum, active.registry_checksum
                    ),
                ));
            }
            (recorded.clone(), false)
        }
        None => (
            WorldContentIdentity {
                format_version: CONTENT_IDENTITY_FORMAT_VERSION,
                pack_id: active.pack_id.clone(),
                pack_checksum: active.pack_checksum,
                registry_checksum: active.registry_checksum,
            },
            true,
        ),
    };
    let mut value: Value = serde_json::from_str(json).map_err(|error| {
        identity_error(
            "content_identity_invalid",
            format!("invalid save JSON: {error}"),
        )
    })?;
    let Some(object) = value.as_object_mut() else {
        return Err(identity_error(
            "content_identity_invalid",
            "the world save is not a JSON object",
        ));
    };
    object.insert(
        "format_version".to_string(),
        Value::from(WORLD_SAVE_FORMAT_VERSION_V2),
    );
    object.insert(
        "content".to_string(),
        serde_json::to_value(&identity).map_err(|error| {
            identity_error(
                "content_identity_invalid",
                format!("content identity serialization failed: {error}"),
            )
        })?,
    );
    let json = serde_json::to_string_pretty(&value).map_err(|error| {
        identity_error(
            "content_identity_invalid",
            format!("content-bound save serialization failed: {error}"),
        )
    })?;
    Ok(StampedWorldContent {
        json,
        status: ContentIdentityStatus::Match,
        identity: Some(identity),
        new,
    })
}

/// Verify a loaded world save before any store or package is parsed.
///
/// # Rules
///
/// - v1 without `content`, base only: `absent`;
/// - v1 without `content`, one pack: `legacy_unbound` (allowed);
/// - v2 with a matching identity: `match` and a v1 `normalized_json`;
/// - v2 without `content`: `content_identity_required`;
/// - v1 with `content`: `content_identity_unexpected`;
/// - an outer version other than 1/2, or content format > 1:
///   `content_identity_unsupported`;
/// - malformed JSON: `corrupt_save`;
/// - malformed, empty, `base`, or unknown-field identity:
///   `content_identity_invalid`;
/// - recorded identity with a base-only registry: `content_pack_missing`;
/// - different pack ID: `content_pack_mismatch`;
/// - same ID, different pack checksum: `content_pack_checksum_mismatch`;
/// - same pack, different merged checksum: `content_registry_checksum_mismatch`;
/// - more than one pack: `content_multiple_packs_unsupported`.
///
/// # Errors
///
/// Returns [`ContentIdentityError`]. The caller must not replace any store,
/// package map, or recorded identity when this fails.
pub(crate) fn verify_world_content(
    json: &str,
    active: &ActiveContentIdentity,
) -> Result<VerifiedWorldContent, ContentIdentityError> {
    let value: Value = serde_json::from_str(json)
        .map_err(|error| identity_error("corrupt_save", format!("invalid save JSON: {error}")))?;
    let Some(object) = value.as_object() else {
        return Err(identity_error(
            "corrupt_save",
            "the world save is not a JSON object",
        ));
    };
    if active.packs > 1 {
        return Err(identity_error(
            "content_multiple_packs_unsupported",
            format!(
                "the runtime registry has {} active packs; one pack is supported",
                active.packs
            ),
        ));
    }
    let version = object
        .get("format_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            identity_error(
                "content_identity_invalid",
                "the world save format_version is missing or not an integer",
            )
        })?;
    let content = object.get("content");
    match version {
        version if version == u64::from(WORLD_SAVE_FORMAT_VERSION_V1) => {
            if content.is_some() {
                return Err(identity_error(
                    "content_identity_unexpected",
                    "a format_version 1 world save must not carry a content identity",
                ));
            }
            if active.packs == 0 {
                Ok(VerifiedWorldContent {
                    status: ContentIdentityStatus::Absent,
                    identity: None,
                    normalized_json: None,
                    format_version: 0,
                })
            } else {
                Ok(VerifiedWorldContent {
                    status: ContentIdentityStatus::LegacyUnbound,
                    identity: None,
                    normalized_json: None,
                    format_version: 0,
                })
            }
        }
        version if version == u64::from(WORLD_SAVE_FORMAT_VERSION_V2) => {
            let Some(content) = content else {
                return Err(identity_error(
                    "content_identity_required",
                    "a format_version 2 world save requires a content identity",
                ));
            };
            let identity = parse_content_identity(content)?;
            if active.packs == 0 {
                return Err(identity_error(
                    "content_pack_missing",
                    format!(
                        "this world requires pack {} ({:016x}) but the runtime registry is base only",
                        identity.pack_id, identity.pack_checksum
                    ),
                ));
            }
            if identity.pack_id != active.pack_id {
                return Err(identity_error(
                    "content_pack_mismatch",
                    format!(
                        "this world requires pack {} but {} is active",
                        identity.pack_id, active.pack_id
                    ),
                ));
            }
            if identity.pack_checksum != active.pack_checksum {
                return Err(identity_error(
                    "content_pack_checksum_mismatch",
                    format!(
                        "pack {} was saved with checksum {:016x} but the active pack is {:016x}",
                        identity.pack_id, identity.pack_checksum, active.pack_checksum
                    ),
                ));
            }
            if identity.registry_checksum != active.registry_checksum {
                return Err(identity_error(
                    "content_registry_checksum_mismatch",
                    format!(
                        "pack {} was saved with merged registry checksum {:016x} but the live registry is {:016x}",
                        identity.pack_id, identity.registry_checksum, active.registry_checksum
                    ),
                ));
            }
            let mut normalized = value;
            if let Some(object) = normalized.as_object_mut() {
                object.remove("content");
                object.insert(
                    "format_version".to_string(),
                    Value::from(WORLD_SAVE_FORMAT_VERSION_V1),
                );
            }
            let normalized_json = serde_json::to_string_pretty(&normalized).map_err(|error| {
                identity_error(
                    "content_identity_invalid",
                    format!("content-bound save normalization failed: {error}"),
                )
            })?;
            Ok(VerifiedWorldContent {
                status: ContentIdentityStatus::Match,
                format_version: identity.format_version,
                identity: Some(identity),
                normalized_json: Some(normalized_json),
            })
        }
        other => Err(identity_error(
            "content_identity_unsupported",
            format!("outer world save format_version {other} is not supported"),
        )),
    }
}

/// Parse and validate the `content` object of a v2 save.
fn parse_content_identity(value: &Value) -> Result<WorldContentIdentity, ContentIdentityError> {
    let Some(object) = value.as_object() else {
        return Err(identity_error(
            "content_identity_invalid",
            "the content identity is not a JSON object",
        ));
    };
    let Some(version) = object.get("format_version").and_then(Value::as_u64) else {
        return Err(identity_error(
            "content_identity_invalid",
            "the content identity format_version is missing or not an integer",
        ));
    };
    if version > u64::from(CONTENT_IDENTITY_FORMAT_VERSION) {
        return Err(identity_error(
            "content_identity_unsupported",
            format!("content identity format_version {version} is not supported"),
        ));
    }
    if version != u64::from(CONTENT_IDENTITY_FORMAT_VERSION) {
        return Err(identity_error(
            "content_identity_invalid",
            format!("content identity format_version {version} is invalid"),
        ));
    }
    let identity: WorldContentIdentity =
        serde_json::from_value(value.clone()).map_err(|error| {
            identity_error(
                "content_identity_invalid",
                format!("invalid content identity: {error}"),
            )
        })?;
    validate_pack_id(&identity.pack_id)?;
    Ok(identity)
}

/// Reject an empty, base, or malformed manifest ID.
///
/// A manifest ID is `mymod` or `mymod:pack` (the forms accepted by
/// `ModManifest`); both are valid pack identities. `base` is never a pack.
fn validate_pack_id(pack_id: &str) -> Result<(), ContentIdentityError> {
    if pack_id == "base" {
        return Err(identity_error(
            "content_identity_invalid",
            "the base registry is never recorded as a pack",
        ));
    }
    if !is_manifest_id(pack_id) {
        return Err(identity_error(
            "content_identity_invalid",
            format!("pack ID {pack_id:?} is not a valid manifest ID"),
        ));
    }
    Ok(())
}

/// Validate the live pack fields before they are written.
fn validate_active_pack(active: &ActiveContentIdentity) -> Result<(), ContentIdentityError> {
    if active.packs != 1 {
        return Err(identity_error(
            "content_multiple_packs_unsupported",
            format!("{} active packs; one pack is supported", active.packs),
        ));
    }
    validate_pack_id(&active.pack_id)
}

/// Whether `id` matches the manifest ID forms `mymod` or `mymod:pack`.
fn is_manifest_id(id: &str) -> bool {
    let mut parts = id.split(':');
    let Some(namespace) = parts.next() else {
        return false;
    };
    if namespace.is_empty() || !namespace.bytes().all(is_manifest_id_byte) {
        return false;
    }
    match parts.next() {
        None => true,
        Some(pack) => {
            !pack.is_empty() && pack.bytes().all(is_manifest_id_byte) && parts.next().is_none()
        }
    }
}

fn is_manifest_id_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
}

/// 16 lowercase hex digits on the wire, `u64` in memory.
mod hex_u64 {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{value:016x}"))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.len() != 16 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(D::Error::custom(
                "checksum must be 16 lowercase hexadecimal digits",
            ));
        }
        if text.bytes().any(|byte| byte.is_ascii_uppercase()) {
            return Err(D::Error::custom(
                "checksum must be 16 lowercase hexadecimal digits",
            ));
        }
        u64::from_str_radix(&text, 16)
            .map_err(|error| D::Error::custom(format!("invalid checksum: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_city::{PlacementRequest, PlacementStore};
    use serde_json::json;

    const PACK_CHECKSUM: u64 = 0xad4c_6458_69a4_11ed;
    const REGISTRY_CHECKSUM: u64 = 0x330a_538d_0780_5b50;
    const BASE_CHECKSUM: u64 = 0xc2df_0f24_2f5e_78d2;

    fn base_active() -> ActiveContentIdentity {
        ActiveContentIdentity::base(BASE_CHECKSUM, BASE_CHECKSUM)
    }

    fn pack_active() -> ActiveContentIdentity {
        ActiveContentIdentity::pack(
            "example:copper_age",
            PACK_CHECKSUM,
            REGISTRY_CHECKSUM,
            BASE_CHECKSUM,
        )
    }

    fn recorded_identity() -> WorldContentIdentity {
        WorldContentIdentity {
            format_version: CONTENT_IDENTITY_FORMAT_VERSION,
            pack_id: "example:copper_age".to_string(),
            pack_checksum: PACK_CHECKSUM,
            registry_checksum: REGISTRY_CHECKSUM,
        }
    }

    fn base_world_json() -> String {
        let mut store = PlacementStore::new(2026);
        let request = PlacementRequest {
            blueprint_id: "local:content_house".to_string(),
            blueprint_hash: "blake3:content".to_string(),
            blueprint_path: "/townsmith/content_house.tsbp".to_string(),
            name: "Content House".to_string(),
            bounds_m: [8.0, 6.0, 6.0],
            center_x: 128.0,
            center_z: 128.0,
            rotation_deg: 0,
            material_cost: 100.0,
            resident_capacity: 2,
            job_slots: 0,
            solid_voxels: 100,
            structure: None,
        };
        store.place(&request, |_, _| 32).expect("placement");
        store.to_json_pretty().expect("world json")
    }

    #[test]
    fn world_content_identity_round_trips_and_rejects_unknown_fields() {
        let identity = recorded_identity();
        let value = serde_json::to_value(&identity).expect("identity json");
        assert_eq!(value["format_version"], 1);
        assert_eq!(value["pack_id"], "example:copper_age");
        assert_eq!(value["pack_checksum"], "ad4c645869a411ed");
        assert_eq!(value["registry_checksum"], "330a538d07805b50");
        let parsed: WorldContentIdentity = serde_json::from_value(value).expect("roundtrip");
        assert_eq!(parsed, identity);

        for (label, content) in [
            (
                "unknown-field",
                json!({"format_version": 1, "pack_id": "example:copper_age",
                       "pack_checksum": "ad4c645869a411ed",
                       "registry_checksum": "330a538d07805b50", "extra": true}),
            ),
            (
                "uppercase",
                json!({"format_version": 1, "pack_id": "example:copper_age",
                       "pack_checksum": "AD4C645869A411ED",
                       "registry_checksum": "330a538d07805b50"}),
            ),
            (
                "short",
                json!({"format_version": 1, "pack_id": "example:copper_age",
                       "pack_checksum": "ad4c", "registry_checksum": "330a538d07805b50"}),
            ),
            (
                "base-id",
                json!({"format_version": 1, "pack_id": "base",
                       "pack_checksum": "ad4c645869a411ed",
                       "registry_checksum": "330a538d07805b50"}),
            ),
        ] {
            assert!(
                parse_content_identity(&content).is_err(),
                "{label} content must be rejected"
            );
        }
    }

    #[test]
    fn bare_manifest_ids_are_valid_pack_bindings() {
        let active =
            ActiveContentIdentity::pack("example", PACK_CHECKSUM, REGISTRY_CHECKSUM, BASE_CHECKSUM);
        let saved = stamp_world_content(&base_world_json(), &active, None).expect("bare pack save");
        assert_eq!(
            saved.identity.as_ref().expect("identity").pack_id,
            "example"
        );
        let verified = verify_world_content(&saved.json, &active).expect("bare pack load");
        assert_eq!(verified.status, ContentIdentityStatus::Match);
        assert_eq!(verified.identity.expect("identity").pack_id, "example");
    }

    #[test]
    fn stamp_base_only_leaves_the_json_byte_identical_and_absent() {
        let json = base_world_json();
        let stamped = stamp_world_content(&json, &base_active(), None).expect("base save");
        assert_eq!(stamped.json, json);
        assert_eq!(stamped.status, ContentIdentityStatus::Absent);
        assert_eq!(stamped.status_str(), "absent");
        assert!(!stamped.content_bound());
        assert!(stamped.identity.is_none());
        assert_eq!(stamped.format_version(), 0);
        assert!(!stamped.json.contains("\"content\""));
        assert_eq!(
            serde_json::from_str::<Value>(&stamped.json).expect("object")["format_version"],
            1
        );
    }

    #[test]
    fn stamp_base_only_with_a_recorded_identity_requires_the_pack() {
        let error = stamp_world_content(
            &base_world_json(),
            &base_active(),
            Some(&recorded_identity()),
        )
        .expect_err("base save must be refused");
        assert_eq!(error.code, "content_save_requires_pack");
    }

    #[test]
    fn stamp_new_identity_writes_outer_v2_deterministically() {
        let json = base_world_json();
        let first = stamp_world_content(&json, &pack_active(), None).expect("new identity");
        let second = stamp_world_content(&json, &pack_active(), None).expect("repeat");
        assert_eq!(first.json, second.json);
        assert_eq!(first.status_str(), "new");
        assert!(first.content_bound());
        assert_eq!(first.format_version(), 1);
        let value: Value = serde_json::from_str(&first.json).expect("v2 json");
        assert_eq!(value["format_version"], 2);
        assert_eq!(value["content"]["format_version"], 1);
        assert_eq!(value["content"]["pack_id"], "example:copper_age");
        assert_eq!(value["content"]["pack_checksum"], "ad4c645869a411ed");
        assert_eq!(value["content"]["registry_checksum"], "330a538d07805b50");
        let content = value["content"].as_object().expect("content object");
        assert_eq!(content.len(), 4);
        assert!(!content.contains_key("path"));
        let identity = first.identity.as_ref().expect("identity");
        assert_eq!(identity, &recorded_identity());
        // Content-bound fixture pin: this exact one-house v2 fixture with the
        // example pack identity. Base-only output keeps its own pin.
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in first.json.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        assert_eq!(hash, 0x1c74_187c_a20c_e289);
    }

    #[test]
    fn stamp_a_recorded_identity_matches_and_keeps_it() {
        let json = base_world_json();
        let recorded = recorded_identity();
        let stamped =
            stamp_world_content(&json, &pack_active(), Some(&recorded)).expect("matching save");
        assert_eq!(stamped.status_str(), "match");
        assert_eq!(stamped.identity.as_ref(), Some(&recorded));
        assert!(stamped.content_bound());
        let value: Value = serde_json::from_str(&stamped.json).expect("v2 json");
        assert_eq!(value["format_version"], 2);
        assert_eq!(value["content"]["pack_id"], "example:copper_age");
    }

    #[test]
    fn stamp_refuses_a_different_pack_id() {
        let mut recorded = recorded_identity();
        recorded.pack_id = "example:other_pack".to_string();
        let error = stamp_world_content(&base_world_json(), &pack_active(), Some(&recorded))
            .expect_err("different pack");
        assert_eq!(error.code, "content_save_pack_mismatch");
    }

    #[test]
    fn stamp_refuses_a_different_pack_checksum() {
        let mut recorded = recorded_identity();
        recorded.pack_checksum = PACK_CHECKSUM ^ 1;
        let error = stamp_world_content(&base_world_json(), &pack_active(), Some(&recorded))
            .expect_err("different checksum");
        assert_eq!(error.code, "content_pack_checksum_mismatch");
    }

    #[test]
    fn stamp_refuses_a_different_registry_checksum() {
        let mut recorded = recorded_identity();
        recorded.registry_checksum = REGISTRY_CHECKSUM ^ 1;
        let error = stamp_world_content(&base_world_json(), &pack_active(), Some(&recorded))
            .expect_err("different registry");
        assert_eq!(error.code, "content_registry_checksum_mismatch");
    }

    #[test]
    fn stamp_refuses_more_than_one_pack() {
        let mut active = pack_active();
        active.packs = 2;
        let error =
            stamp_world_content(&base_world_json(), &active, None).expect_err("multiple packs");
        assert_eq!(error.code, "content_multiple_packs_unsupported");
    }

    #[test]
    fn verify_v1_without_content_is_absent_for_base() {
        let json = base_world_json();
        let verified = verify_world_content(&json, &base_active()).expect("base load");
        assert_eq!(verified.status, ContentIdentityStatus::Absent);
        assert_eq!(verified.status_str(), "absent");
        assert!(verified.identity.is_none());
        assert!(verified.normalized_json.is_none());
        assert_eq!(verified.format_version, 0);
    }

    #[test]
    fn verify_v1_without_content_is_legacy_unbound_for_a_pack() {
        let json = base_world_json();
        let verified = verify_world_content(&json, &pack_active()).expect("legacy load");
        assert_eq!(verified.status, ContentIdentityStatus::LegacyUnbound);
        assert_eq!(verified.status_str(), "legacy_unbound");
        assert!(verified.identity.is_none());
        assert!(verified.normalized_json.is_none());

        let stamped = stamp_world_content(&json, &pack_active(), verified.identity.as_ref())
            .expect("next save binds the pack");
        assert_eq!(stamped.status_str(), "new");
        assert_eq!(
            serde_json::from_str::<Value>(&stamped.json).expect("v2")["format_version"],
            2
        );
    }

    #[test]
    fn verify_v2_match_strips_content_and_restores_v1_records() {
        let json = base_world_json();
        let before = PlacementStore::from_json(&json).expect("v1 store");
        let stamped = stamp_world_content(&json, &pack_active(), None).expect("v2 save");
        let verified = verify_world_content(&stamped.json, &pack_active()).expect("v2 load");
        assert_eq!(verified.status, ContentIdentityStatus::Match);
        assert_eq!(verified.status_str(), "match");
        assert_eq!(verified.format_version, 1);
        assert_eq!(verified.identity.as_ref(), stamped.identity.as_ref());
        let normalized = verified.normalized_json.as_deref().expect("v1 json");
        let value: Value = serde_json::from_str(normalized).expect("normalized json");
        assert_eq!(value["format_version"], 1);
        assert!(value.get("content").is_none());
        let after = PlacementStore::from_json(normalized).expect("normalized store");
        assert_eq!(after, before);
        assert_eq!(after.checksum(), before.checksum());
        assert_eq!(after.placements, before.placements);

        let resaved = stamp_world_content(normalized, &pack_active(), verified.identity.as_ref())
            .expect("re-save");
        assert_eq!(resaved.status_str(), "match");
        assert_eq!(resaved.json, stamped.json);
    }

    #[test]
    fn verify_v2_without_content_is_required() {
        let json = base_world_json().replace("\"format_version\": 1", "\"format_version\": 2");
        let error = verify_world_content(&json, &base_active()).expect_err("missing identity");
        assert_eq!(error.code, "content_identity_required");
        let error = verify_world_content(&json, &pack_active()).expect_err("missing identity");
        assert_eq!(error.code, "content_identity_required");
    }

    #[test]
    fn verify_v1_with_content_is_unexpected() {
        let stamped =
            stamp_world_content(&base_world_json(), &pack_active(), None).expect("content save");
        let json = stamped
            .json
            .replace("\"format_version\": 2", "\"format_version\": 1");
        let error = verify_world_content(&json, &pack_active()).expect_err("unexpected identity");
        assert_eq!(error.code, "content_identity_unexpected");
    }

    #[test]
    fn verify_an_unsupported_outer_or_content_version_is_rejected() {
        let stamped =
            stamp_world_content(&base_world_json(), &pack_active(), None).expect("content save");
        let outer = stamped
            .json
            .replace("\"format_version\": 2", "\"format_version\": 3");
        assert_eq!(
            verify_world_content(&outer, &pack_active())
                .expect_err("outer v3")
                .code,
            "content_identity_unsupported"
        );
        let inner = stamped
            .json
            .replace("\"format_version\": 1", "\"format_version\": 2");
        assert_eq!(
            verify_world_content(&inner, &pack_active())
                .expect_err("content v2")
                .code,
            "content_identity_unsupported"
        );
    }

    #[test]
    fn verify_malformed_json_or_content_is_rejected() {
        let error = verify_world_content("not json", &base_active()).expect_err("malformed");
        assert_eq!(error.code, "corrupt_save");
        let error = verify_world_content("[]", &base_active()).expect_err("not an object");
        assert_eq!(error.code, "corrupt_save");

        let stamped =
            stamp_world_content(&base_world_json(), &pack_active(), None).expect("content save");
        for (label, json) in [
            (
                "content-not-object",
                stamped
                    .json
                    .replace("\"content\": {", "\"content\": \"bad\", \"old\": {"),
            ),
            (
                "unknown-field",
                stamped.json.replace(
                    "\"registry_checksum\": \"330a538d07805b50\"",
                    "\"registry_checksum\": \"330a538d07805b50\", \"extra\": 1",
                ),
            ),
            (
                "uppercase-checksum",
                stamped
                    .json
                    .replace("\"ad4c645869a411ed\"", "\"AD4C645869A411ED\""),
            ),
            (
                "base-pack",
                stamped.json.replace("\"example:copper_age\"", "\"base\""),
            ),
        ] {
            let error = match verify_world_content(&json, &pack_active()) {
                Ok(_) => panic!("{label} must be rejected"),
                Err(error) => error,
            };
            assert_eq!(error.code, "content_identity_invalid", "{label}: {error:?}");
        }
    }

    #[test]
    fn verify_a_recorded_identity_with_base_content_is_missing() {
        let stamped =
            stamp_world_content(&base_world_json(), &pack_active(), None).expect("content save");
        let error = verify_world_content(&stamped.json, &base_active()).expect_err("base only");
        assert_eq!(error.code, "content_pack_missing");
    }

    #[test]
    fn verify_refuses_a_different_pack_id_or_checksums() {
        let stamped =
            stamp_world_content(&base_world_json(), &pack_active(), None).expect("content save");
        let mut other = pack_active();
        other.pack_id = "example:other_pack".to_string();
        assert_eq!(
            verify_world_content(&stamped.json, &other)
                .expect_err("pack id")
                .code,
            "content_pack_mismatch"
        );
        let mut other = pack_active();
        other.pack_checksum ^= 1;
        assert_eq!(
            verify_world_content(&stamped.json, &other)
                .expect_err("pack checksum")
                .code,
            "content_pack_checksum_mismatch"
        );
        let mut other = pack_active();
        other.registry_checksum ^= 1;
        assert_eq!(
            verify_world_content(&stamped.json, &other)
                .expect_err("registry checksum")
                .code,
            "content_registry_checksum_mismatch"
        );
    }

    #[test]
    fn verify_refuses_more_than_one_pack() {
        let mut active = pack_active();
        active.packs = 2;
        assert_eq!(
            verify_world_content(&base_world_json(), &active)
                .expect_err("multiple packs")
                .code,
            "content_multiple_packs_unsupported"
        );
    }

    #[test]
    fn a_failed_decision_yields_no_identity_to_commit() {
        // The bridge commits only the identity returned by a successful
        // decision, so an error cannot replace the recorded identity.
        let mut recorded = Some(recorded_identity());
        let json = stamp_world_content(&base_world_json(), &pack_active(), None)
            .expect("content save")
            .json;
        let mut other = pack_active();
        other.pack_checksum ^= 1;
        assert!(verify_world_content(&json, &other).is_err());
        assert_eq!(recorded, Some(recorded_identity()));

        let error = stamp_world_content(&base_world_json(), &base_active(), recorded.as_ref())
            .expect_err("missing pack on save");
        assert_eq!(error.code, "content_save_requires_pack");
        assert_eq!(recorded, Some(recorded_identity()));
        recorded = None;
        assert!(recorded.is_none());
    }
}
