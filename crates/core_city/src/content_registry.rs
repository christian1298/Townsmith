//! D-042 content-registry manifest parser (proposed).
//!
//! [[docs/84 - Content Registry Modes|Content Registry Modes]] proposes an
//! operator-supplied manifest that gives every active content ID a kind
//! (`mod` or `resource_pack`), an optional matching namespace, and a canonical
//! `blake3:<64 lowercase hex>` checksum. Base content is implicit and is never
//! listed.
//!
//! This module is the pure parser and validation contract. It does not read a
//! file, bind a listener, change `--content-ids`, or alter placement behavior;
//! the server wiring described by D-042 is deliberately absent while the ADR is
//! **Proposed**. See [`parse_content_registry_manifest`].
//!
//! # Shape
//!
//! ```json
//! {
//!   "format_version": 1,
//!   "ids": [
//!     {
//!       "id": "mymod",
//!       "kind": "mod",
//!       "namespace": "mymod",
//!       "checksum": "blake3:0000000000000000000000000000000000000000000000000000000000000000"
//!     }
//!   ]
//! }
//! ```
//!
//! Unknown fields, duplicate IDs or namespaces, explicit `base`, unsupported
//! versions, malformed IDs/namespaces, and non-canonical checksums all fail
//! closed. The parser accepts at most [`MAX_CONTENT_REGISTRY_ENTRIES`] entries
//! and [`MAX_CONTENT_REGISTRY_BYTES`] input bytes.
//!
//! # IDs
//!
//! An ID is `namespace` or `namespace:pack` using lowercase ASCII letters,
//! digits, `_`, and `-`, at most [`MAX_CONTENT_ID_BYTES`] bytes. Control
//! characters, path separators, dots, and repeated colons are rejected, so an
//! ID can never name a path or traverse one.
//!
//! # Example
//!
//! ```
//! use core_city::content_registry::{parse_content_registry_manifest, ContentRegistryKind};
//!
//! let manifest = parse_content_registry_manifest(
//!     r#"{
//!         "format_version": 1,
//!         "ids": [{
//!             "id": "example",
//!             "kind": "mod",
//!             "checksum": "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
//!         }]
//!     }"#,
//! )
//! .expect("valid manifest");
//!
//! assert_eq!(manifest.ids[0].kind, ContentRegistryKind::Mod);
//! assert_eq!(manifest.implicit_ids(), ["base", "example"]);
//! assert!(manifest.provides("example"));
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Manifest format version understood by this build.
pub const CONTENT_REGISTRY_FORMAT_VERSION: u16 = 1;

/// Maximum entries accepted in one registry manifest (docs/84 §8).
pub const MAX_CONTENT_REGISTRY_ENTRIES: usize = 256;

/// Maximum manifest size in bytes (1 MiB, docs/84 §8).
pub const MAX_CONTENT_REGISTRY_BYTES: usize = 1024 * 1024;

/// Maximum content-ID length in bytes (docs/84 §8).
pub const MAX_CONTENT_ID_BYTES: usize = 64;

/// Base game content ID, always present and never listed in a manifest.
pub const BASE_CONTENT_ID: &str = "base";

/// Canonical checksum prefix for content hashes.
pub const CONTENT_CHECKSUM_PREFIX: &str = "blake3:";

/// Length of the lowercase hex body of a canonical content checksum.
pub const CONTENT_CHECKSUM_HEX_LEN: usize = 64;

/// The kind of content a registry ID names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentRegistryKind {
    /// A mod: content documents, rules, or code loaded into the game.
    Mod,
    /// A resource pack: assets and data that do not add new content rules.
    ResourcePack,
}

impl ContentRegistryKind {
    /// Stable lowercase label used by manifests and status lines.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mod => "mod",
            Self::ResourcePack => "resource_pack",
        }
    }
}

impl fmt::Display for ContentRegistryKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One content-registry entry from a D-042 manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentRegistryEntry {
    /// Canonical registry key: `namespace` or `namespace:pack`.
    pub id: String,
    /// Whether the ID names a mod or a resource pack.
    pub kind: ContentRegistryKind,
    /// Optional matching alias; may equal [`Self::id`].
    pub namespace: Option<String>,
    /// Canonical `blake3:<64 lowercase hex>` content hash.
    ///
    /// The manifest form requires it; the field stays optional so a caller can
    /// represent an entry before validation or the `--content-ids` short form.
    pub checksum: Option<String>,
}

impl ContentRegistryEntry {
    /// Validate this entry's ID, namespace, and checksum.
    fn validate(&self) -> Result<(), ContentRegistryError> {
        if self.id == BASE_CONTENT_ID {
            return Err(ContentRegistryError::invalid(
                "`base` is implicit and must not be listed",
            ));
        }
        if !is_valid_content_id(&self.id) {
            return Err(ContentRegistryError::invalid(format!(
                "invalid content id `{}`",
                self.id
            )));
        }
        if let Some(namespace) = &self.namespace {
            if namespace == BASE_CONTENT_ID {
                return Err(ContentRegistryError::invalid(
                    "`base` is implicit and must not be listed",
                ));
            }
            if !is_valid_content_id(namespace) {
                return Err(ContentRegistryError::invalid(format!(
                    "invalid content namespace `{namespace}`"
                )));
            }
        }
        match &self.checksum {
            None => Err(ContentRegistryError::invalid(format!(
                "content entry `{}` is missing a checksum",
                self.id
            ))),
            Some(checksum) if !is_canonical_content_checksum(checksum) => {
                Err(ContentRegistryError::invalid(format!(
                    "content entry `{}` has a malformed checksum",
                    self.id
                )))
            }
            Some(_) => Ok(()),
        }
    }
}

/// A D-042 content-registry manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentRegistryManifest {
    /// Manifest schema version; only [`CONTENT_REGISTRY_FORMAT_VERSION`] is valid.
    pub format_version: u16,
    /// Registry entries. Order is not significant.
    pub ids: Vec<ContentRegistryEntry>,
}

impl ContentRegistryManifest {
    /// Parse and validate a content-registry manifest.
    ///
    /// # Errors
    ///
    /// Returns [`ContentRegistryError::InvalidRegistry`] for malformed JSON,
    /// unknown fields, entry validation failures, or duplicate claims, and
    /// [`ContentRegistryError::UnsupportedVersion`] for any `format_version`
    /// other than 1.
    pub fn parse(json: &str) -> Result<Self, ContentRegistryError> {
        if json.len() > MAX_CONTENT_REGISTRY_BYTES {
            return Err(ContentRegistryError::invalid(format!(
                "manifest exceeds {MAX_CONTENT_REGISTRY_BYTES} bytes"
            )));
        }
        let manifest: Self = serde_json::from_str(json).map_err(|error| {
            ContentRegistryError::invalid(format!("malformed manifest JSON: {error}"))
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validate an already-deserialized manifest.
    ///
    /// # Errors
    ///
    /// Returns [`ContentRegistryError::UnsupportedVersion`] for an unsupported
    /// version, and [`ContentRegistryError::InvalidRegistry`] or
    /// [`ContentRegistryError::KindMismatch`] for an invalid entry or a claim
    /// shared across kinds.
    pub fn validate(&self) -> Result<(), ContentRegistryError> {
        if self.format_version != CONTENT_REGISTRY_FORMAT_VERSION {
            return Err(ContentRegistryError::UnsupportedVersion {
                version: self.format_version,
            });
        }
        if self.ids.len() > MAX_CONTENT_REGISTRY_ENTRIES {
            return Err(ContentRegistryError::invalid(format!(
                "manifest must contain at most {MAX_CONTENT_REGISTRY_ENTRIES} entries"
            )));
        }

        let mut claims: BTreeMap<&str, ContentRegistryKind> = BTreeMap::new();
        for entry in &self.ids {
            entry.validate()?;
            let mut own_claims = vec![entry.id.as_str()];
            if let Some(namespace) = entry.namespace.as_deref() {
                if namespace != entry.id {
                    own_claims.push(namespace);
                }
            }
            for claim in own_claims {
                if let Some(previous) = claims.get(claim) {
                    if *previous != entry.kind {
                        return Err(ContentRegistryError::KindMismatch {
                            id: claim.to_string(),
                            expected: *previous,
                            found: entry.kind,
                        });
                    }
                    return Err(ContentRegistryError::invalid(format!(
                        "duplicate content id or namespace `{claim}`"
                    )));
                }
                claims.insert(claim, entry.kind);
            }
        }
        Ok(())
    }

    /// Base plus every manifest ID, sorted and deduplicated.
    #[must_use]
    pub fn implicit_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .ids
            .iter()
            .filter(|entry| entry.id != BASE_CONTENT_ID)
            .map(|entry| entry.id.clone())
            .collect();
        ids.sort();
        ids.dedup();
        ids.insert(0, BASE_CONTENT_ID.to_string());
        ids
    }

    /// Resolve a requested ID or namespace to base plus canonical registry IDs.
    ///
    /// `base` is always present and may be requested explicitly. Each token is
    /// resolved by exact ID/namespace first, then by the shared namespace rule
    /// (`mymod` matches `mymod:pack`). The result is sorted and deduplicated,
    /// so manifest order and request order do not affect it.
    ///
    /// # Errors
    ///
    /// Returns [`ContentRegistryError::UnknownId`] when a token matches no
    /// entry and [`ContentRegistryError::KindMismatch`] when one token matches
    /// entries of more than one kind. A same-kind ambiguity on a hand-built,
    /// unvalidated manifest is [`ContentRegistryError::InvalidRegistry`].
    pub fn select(&self, requested: &[&str]) -> Result<Vec<String>, ContentRegistryError> {
        let mut selected = BTreeSet::new();
        selected.insert(BASE_CONTENT_ID.to_string());
        for raw in requested {
            let token = raw.trim();
            if token.is_empty() || token == BASE_CONTENT_ID {
                if token == BASE_CONTENT_ID {
                    continue;
                }
                return Err(ContentRegistryError::UnknownId {
                    id: token.to_string(),
                });
            }
            selected.insert(self.resolve(token)?);
        }
        Ok(selected.into_iter().collect())
    }

    /// Whether the registry provides `required`.
    ///
    /// Matching is exact ID, exact explicit namespace, or shared namespace:
    /// `mymod` and `mymod:pack` match each other, mirroring the existing
    /// `.tsbp` dependency rule. `base` is always provided.
    #[must_use]
    pub fn provides(&self, required: &str) -> bool {
        let required = required.trim();
        if required.is_empty() {
            return false;
        }
        if required == BASE_CONTENT_ID {
            return true;
        }
        let required_namespace = namespace_of(required);
        self.ids.iter().any(|entry| {
            entry.id == required
                || entry.namespace.as_deref() == Some(required)
                || namespace_of(&entry.id) == required_namespace
                || entry
                    .namespace
                    .as_deref()
                    .is_some_and(|namespace| namespace_of(namespace) == required_namespace)
        })
    }

    /// Resolve one selector token to a canonical entry ID.
    fn resolve(&self, token: &str) -> Result<String, ContentRegistryError> {
        let exact: Vec<&ContentRegistryEntry> = self
            .ids
            .iter()
            .filter(|entry| entry.id == token || entry.namespace.as_deref() == Some(token))
            .collect();
        match exact.as_slice() {
            [] => {}
            [entry] => return Ok(entry.id.clone()),
            entries => return Err(ambiguous_selector(token, entries)),
        }

        let token_namespace = namespace_of(token);
        let matched: Vec<&ContentRegistryEntry> = self
            .ids
            .iter()
            .filter(|entry| {
                namespace_of(&entry.id) == token_namespace
                    || entry
                        .namespace
                        .as_deref()
                        .is_some_and(|namespace| namespace_of(namespace) == token_namespace)
            })
            .collect();
        match matched.as_slice() {
            [] => Err(ContentRegistryError::UnknownId {
                id: token.to_string(),
            }),
            [entry] => Ok(entry.id.clone()),
            entries => Err(ambiguous_selector(token, entries)),
        }
    }
}

/// Parse and validate a D-042 content-registry manifest.
///
/// This is the free-function entry point; [`ContentRegistryManifest::parse`]
/// is equivalent.
///
/// # Errors
///
/// Returns [`ContentRegistryError::InvalidRegistry`] for malformed JSON,
/// unknown fields, entry validation failures, or duplicate claims, and
/// [`ContentRegistryError::UnsupportedVersion`] for any `format_version` other
/// than 1.
pub fn parse_content_registry_manifest(
    json: &str,
) -> Result<ContentRegistryManifest, ContentRegistryError> {
    ContentRegistryManifest::parse(json)
}

/// Whether `id` is a valid registry ID or namespace.
///
/// The form is `namespace` or `namespace:pack`: lowercase ASCII letters,
/// digits, `_`, and `-`, 1–64 bytes, with at most one non-empty colon.
#[must_use]
pub fn is_valid_content_id(id: &str) -> bool {
    if id.is_empty() || id.len() > MAX_CONTENT_ID_BYTES {
        return false;
    }
    let mut parts = id.split(':');
    let Some(namespace) = parts.next() else {
        return false;
    };
    if namespace.is_empty() || !namespace.bytes().all(is_id_byte) {
        return false;
    }
    match parts.next() {
        None => true,
        Some(pack) => !pack.is_empty() && pack.bytes().all(is_id_byte) && parts.next().is_none(),
    }
}

/// Whether `value` is a canonical `blake3:<64 lowercase hex>` checksum.
#[must_use]
pub fn is_canonical_content_checksum(value: &str) -> bool {
    let Some(hex) = value.strip_prefix(CONTENT_CHECKSUM_PREFIX) else {
        return false;
    };
    hex.len() == CONTENT_CHECKSUM_HEX_LEN
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// Namespace of an ID, split at the first `:`.
fn namespace_of(id: &str) -> &str {
    id.split_once(':').map_or(id, |(namespace, _)| namespace)
}

fn is_id_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
}

/// Build the deterministic kind/ambiguity error for one selector token.
fn ambiguous_selector(token: &str, entries: &[&ContentRegistryEntry]) -> ContentRegistryError {
    let mut kinds = entries.iter().map(|entry| entry.kind);
    let Some(first) = kinds.next() else {
        return ContentRegistryError::UnknownId {
            id: token.to_string(),
        };
    };
    if let Some(found) = kinds.find(|kind| *kind != first) {
        return ContentRegistryError::KindMismatch {
            id: token.to_string(),
            expected: first,
            found,
        };
    }
    ContentRegistryError::invalid(format!(
        "content selector `{token}` matches more than one registry entry"
    ))
}

/// Why a content-registry manifest or selection was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentRegistryError {
    /// Malformed JSON/schema or an invalid entry, claim, or selector.
    InvalidRegistry {
        /// Human-readable detail; the stable code is [`Self::code`].
        reason: String,
    },
    /// `format_version` is not [`CONTENT_REGISTRY_FORMAT_VERSION`].
    UnsupportedVersion {
        /// Version found in the manifest.
        version: u16,
    },
    /// A selected ID or namespace is not in the manifest.
    UnknownId {
        /// Requested token.
        id: String,
    },
    /// One ID or namespace was claimed by different kinds, or a selector is
    /// ambiguous across kinds.
    KindMismatch {
        /// ID, namespace, or selector.
        id: String,
        /// Kind already registered.
        expected: ContentRegistryKind,
        /// Conflicting kind.
        found: ContentRegistryKind,
    },
}

impl ContentRegistryError {
    /// Stable API code from docs/84 §6.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidRegistry { .. } => "content.invalid_registry",
            Self::UnsupportedVersion { .. } => "content.unsupported_registry_version",
            Self::UnknownId { .. } => "content.unknown_id",
            Self::KindMismatch { .. } => "content.kind_mismatch",
        }
    }

    fn invalid(reason: impl Into<String>) -> Self {
        Self::InvalidRegistry {
            reason: reason.into(),
        }
    }
}

impl fmt::Display for ContentRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRegistry { reason } => {
                write!(formatter, "invalid content registry: {reason}")
            }
            Self::UnsupportedVersion { version } => write!(
                formatter,
                "unsupported content registry format_version {version}; expected {CONTENT_REGISTRY_FORMAT_VERSION}"
            ),
            Self::UnknownId { id } => write!(formatter, "unknown content id `{id}`"),
            Self::KindMismatch {
                id,
                expected,
                found,
            } => write!(
                formatter,
                "content kind mismatch for `{id}`: expected {expected}, found {found}"
            ),
        }
    }
}

impl Error for ContentRegistryError {}

#[cfg(test)]
mod tests {
    use super::{
        is_canonical_content_checksum, is_valid_content_id, parse_content_registry_manifest,
        ContentRegistryError, ContentRegistryKind, ContentRegistryManifest,
        MAX_CONTENT_REGISTRY_ENTRIES,
    };
    use serde_json::json;

    fn checksum(byte: &str) -> String {
        format!("blake3:{}", byte.repeat(64))
    }

    fn entry(
        id: &str,
        kind: &str,
        namespace: Option<&str>,
        checksum: Option<&str>,
    ) -> serde_json::Value {
        let mut entry = json!({
            "id": id,
            "kind": kind,
        });
        if let Some(namespace) = namespace {
            entry["namespace"] = json!(namespace);
        }
        if let Some(checksum) = checksum {
            entry["checksum"] = json!(checksum);
        }
        entry
    }

    fn manifest(entries: Vec<serde_json::Value>) -> String {
        json!({
            "format_version": 1,
            "ids": entries,
        })
        .to_string()
    }

    fn hash_a() -> String {
        checksum("a")
    }

    fn hash_b() -> String {
        checksum("b")
    }

    #[test]
    fn parses_valid_one_mod_manifest() {
        let parsed = parse_content_registry_manifest(&manifest(vec![entry(
            "mymod",
            "mod",
            Some("mymod"),
            Some(&hash_a()),
        )]))
        .expect("valid manifest");

        assert_eq!(parsed.format_version, 1);
        assert_eq!(parsed.ids.len(), 1);
        assert_eq!(parsed.ids[0].kind, ContentRegistryKind::Mod);
        assert_eq!(parsed.ids[0].namespace.as_deref(), Some("mymod"));
        assert_eq!(parsed.implicit_ids(), ["base", "mymod"]);
        assert!(parsed.provides("mymod"));
    }

    #[test]
    fn base_is_implicit_and_empty_registry_is_valid() {
        let parsed = parse_content_registry_manifest(r#"{"format_version":1,"ids":[]}"#)
            .expect("base-only manifest");

        assert_eq!(parsed.implicit_ids(), ["base"]);
        assert_eq!(parsed.select(&[]).expect("selection"), ["base"]);
        assert_eq!(parsed.select(&["base"]).expect("selection"), ["base"]);
        assert!(parsed.provides("base"));
        assert!(!parsed.provides("mymod"));
    }

    #[test]
    fn parses_resource_pack_and_namespace_alias() {
        let parsed = parse_content_registry_manifest(&manifest(vec![entry(
            "mymod:pack",
            "resource_pack",
            Some("mymod:pack"),
            Some(&hash_a()),
        )]))
        .expect("valid manifest");

        assert_eq!(parsed.ids[0].kind, ContentRegistryKind::ResourcePack);
        assert_eq!(
            parsed.select(&["mymod:pack"]).expect("selection"),
            ["base", "mymod:pack"]
        );
        assert_eq!(
            parsed.select(&["mymod"]).expect("namespace selection"),
            ["base", "mymod:pack"]
        );
        assert!(parsed.provides("mymod:pack"));
        assert!(parsed.provides("mymod"));
        assert!(!parsed.provides("other"));
    }

    #[test]
    fn namespace_may_equal_id() {
        let parsed = parse_content_registry_manifest(&manifest(vec![entry(
            "mymod",
            "mod",
            Some("mymod"),
            Some(&hash_a()),
        )]))
        .expect("namespace may equal id");

        assert_eq!(parsed.ids[0].namespace.as_deref(), Some("mymod"));
    }

    #[test]
    fn rejects_duplicate_id() {
        let error = parse_content_registry_manifest(&manifest(vec![
            entry("mymod", "mod", None, Some(&hash_a())),
            entry("mymod", "mod", None, Some(&hash_b())),
        ]))
        .expect_err("duplicate id");

        assert_eq!(error.code(), "content.invalid_registry");
        assert!(error.to_string().contains("duplicate"));
    }

    #[test]
    fn rejects_duplicate_namespace() {
        let error = parse_content_registry_manifest(&manifest(vec![
            entry("mymod", "mod", Some("shared"), Some(&hash_a())),
            entry("other", "mod", Some("shared"), Some(&hash_b())),
        ]))
        .expect_err("duplicate namespace");

        assert_eq!(error.code(), "content.invalid_registry");
    }

    #[test]
    fn rejects_explicit_base() {
        let as_id = parse_content_registry_manifest(&manifest(vec![entry(
            "base",
            "mod",
            None,
            Some(&hash_a()),
        )]))
        .expect_err("base id");
        assert_eq!(as_id.code(), "content.invalid_registry");
        assert!(as_id.to_string().contains("implicit"));

        let as_namespace = parse_content_registry_manifest(&manifest(vec![entry(
            "mymod",
            "mod",
            Some("base"),
            Some(&hash_a()),
        )]))
        .expect_err("base namespace");
        assert_eq!(as_namespace.code(), "content.invalid_registry");
    }

    #[test]
    fn rejects_empty_id() {
        let error = parse_content_registry_manifest(&manifest(vec![entry(
            "",
            "mod",
            None,
            Some(&hash_a()),
        )]))
        .expect_err("empty id");

        assert_eq!(error.code(), "content.invalid_registry");
        assert!(error.to_string().contains("invalid content id"));
    }

    #[test]
    fn rejects_uppercase_id() {
        let error = parse_content_registry_manifest(&manifest(vec![entry(
            "MyMod",
            "mod",
            None,
            Some(&hash_a()),
        )]))
        .expect_err("uppercase id");

        assert_eq!(error.code(), "content.invalid_registry");
    }

    #[test]
    fn rejects_bad_namespace() {
        for namespace in [
            "MyMod",
            "mymod/../other",
            "mymod:",
            ":mymod",
            "mymod:bad:extra",
        ] {
            let error = parse_content_registry_manifest(&manifest(vec![entry(
                "mymod",
                "mod",
                Some(namespace),
                Some(&hash_a()),
            )]))
            .expect_err("bad namespace");
            assert_eq!(error.code(), "content.invalid_registry", "{namespace}");
        }
    }

    #[test]
    fn rejects_bad_checksums() {
        for checksum in [
            "sha256:".to_string() + &"a".repeat(64),
            "blake3:".to_string() + &"a".repeat(63),
            "blake3:".to_string() + &"A".repeat(64),
            "blake3:".to_string() + &"g".repeat(64),
            "a".repeat(64),
        ] {
            let error = parse_content_registry_manifest(&manifest(vec![entry(
                "mymod",
                "mod",
                None,
                Some(&checksum),
            )]))
            .expect_err("bad checksum");
            assert_eq!(error.code(), "content.invalid_registry", "{checksum}");
        }

        let missing =
            parse_content_registry_manifest(&manifest(vec![entry("mymod", "mod", None, None)]))
                .expect_err("missing checksum");
        assert_eq!(missing.code(), "content.invalid_registry");
        assert!(missing.to_string().contains("missing a checksum"));
    }

    #[test]
    fn rejects_unsupported_versions() {
        for version in [0, 2] {
            let json = json!({
                "format_version": version,
                "ids": [],
            })
            .to_string();
            let error = parse_content_registry_manifest(&json).expect_err("bad version");

            assert_eq!(error, ContentRegistryError::UnsupportedVersion { version });
            assert_eq!(error.code(), "content.unsupported_registry_version");
        }
    }

    #[test]
    fn rejects_unknown_fields_and_malformed_json() {
        let unknown =
            parse_content_registry_manifest(r#"{"format_version":1,"ids":[],"extra":true}"#)
                .expect_err("unknown field");
        assert_eq!(unknown.code(), "content.invalid_registry");

        let malformed = parse_content_registry_manifest("{").expect_err("malformed JSON");
        assert_eq!(malformed.code(), "content.invalid_registry");
    }

    #[test]
    fn rejects_more_than_the_entry_cap() {
        let entries = (0..=MAX_CONTENT_REGISTRY_ENTRIES)
            .map(|index| entry(&format!("mod{index}"), "mod", None, Some(&hash_a())))
            .collect();
        let error = parse_content_registry_manifest(&manifest(entries)).expect_err("entry cap");

        assert_eq!(error.code(), "content.invalid_registry");
    }

    #[test]
    fn select_rejects_unknown_id() {
        let parsed = parse_content_registry_manifest(&manifest(vec![entry(
            "mymod",
            "mod",
            None,
            Some(&hash_a()),
        )]))
        .expect("valid manifest");

        let error = parsed.select(&["missing"]).expect_err("unknown id");
        assert_eq!(
            error,
            ContentRegistryError::UnknownId {
                id: "missing".to_string()
            }
        );
        assert_eq!(error.code(), "content.unknown_id");
    }

    #[test]
    fn select_is_sorted_and_deterministic() {
        let parsed = parse_content_registry_manifest(&manifest(vec![
            entry("zmod", "mod", None, Some(&hash_a())),
            entry("mymod", "mod", None, Some(&hash_b())),
        ]))
        .expect("valid manifest");

        let first = parsed
            .select(&["zmod", "mymod", "zmod", "base"])
            .expect("selection");
        let second = parsed
            .select(&["base", "mymod", "zmod"])
            .expect("selection");

        assert_eq!(first, ["base", "mymod", "zmod"]);
        assert_eq!(first, second);
        assert_eq!(parsed.implicit_ids(), first);
    }

    #[test]
    fn select_rejects_cross_kind_namespace_ambiguity() {
        let parsed = parse_content_registry_manifest(&manifest(vec![
            entry("shared:one", "mod", None, Some(&hash_a())),
            entry("shared:two", "resource_pack", None, Some(&hash_b())),
        ]))
        .expect("valid manifest");

        let error = parsed
            .select(&["shared"])
            .expect_err("ambiguous across kinds");
        assert_eq!(error.code(), "content.kind_mismatch");
    }

    #[test]
    fn rejects_cross_kind_id_or_namespace_claim() {
        let as_id = parse_content_registry_manifest(&manifest(vec![
            entry("mymod", "mod", None, Some(&hash_a())),
            entry("mymod", "resource_pack", None, Some(&hash_b())),
        ]))
        .expect_err("cross-kind id");
        assert_eq!(as_id.code(), "content.kind_mismatch");

        let as_namespace = parse_content_registry_manifest(&manifest(vec![
            entry("mymod", "mod", Some("shared"), Some(&hash_a())),
            entry("other", "resource_pack", Some("shared"), Some(&hash_b())),
        ]))
        .expect_err("cross-kind namespace");
        assert_eq!(as_namespace.code(), "content.kind_mismatch");
    }

    #[test]
    fn provides_matches_exact_and_shared_namespace() {
        let parsed = parse_content_registry_manifest(&manifest(vec![
            entry("mymod", "mod", None, Some(&hash_a())),
            entry("mymod:pack", "resource_pack", None, Some(&hash_b())),
        ]))
        .expect("valid manifest");

        assert!(parsed.provides("mymod"));
        assert!(parsed.provides("mymod:pack"));
        assert!(parsed.provides("mymod:other"));
        assert!(!parsed.provides("other"));
        assert!(!parsed.provides(""));
        assert!(parsed.provides(" base "));
    }

    #[test]
    fn serde_round_trip_preserves_manifest() {
        let parsed = parse_content_registry_manifest(&manifest(vec![
            entry("mymod", "mod", Some("mymod"), Some(&hash_a())),
            entry(
                "mymod:pack",
                "resource_pack",
                Some("mymod:pack"),
                Some(&hash_b()),
            ),
        ]))
        .expect("valid manifest");

        let json = serde_json::to_string(&parsed).expect("serialize");
        assert!(json.contains("\"kind\":\"resource_pack\""));
        let reparsed = parse_content_registry_manifest(&json).expect("round trip");
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn id_and_checksum_helpers_follow_the_documented_rules() {
        assert!(is_valid_content_id("mymod"));
        assert!(is_valid_content_id("mymod:pack"));
        assert!(is_valid_content_id("my-mod_2:pack_1"));
        assert!(!is_valid_content_id(""));
        assert!(!is_valid_content_id("MyMod"));
        assert!(!is_valid_content_id("mymod:"));
        assert!(!is_valid_content_id(":mymod"));
        assert!(!is_valid_content_id("mymod:a:b"));
        assert!(!is_valid_content_id("mymod/../pack"));
        assert!(!is_valid_content_id(&"a".repeat(65)));

        assert!(is_canonical_content_checksum(&hash_a()));
        assert!(!is_canonical_content_checksum("blake3:"));
        assert!(!is_canonical_content_checksum(&"a".repeat(64)));
        assert!(!is_canonical_content_checksum(&format!(
            "blake3:{}",
            "A".repeat(64)
        )));
    }

    #[test]
    fn validated_manifest_can_be_constructed_and_revalidated() {
        let manifest = ContentRegistryManifest {
            format_version: 1,
            ids: Vec::new(),
        };
        manifest.validate().expect("valid empty manifest");

        let invalid = ContentRegistryManifest {
            format_version: 2,
            ids: Vec::new(),
        };
        assert_eq!(
            invalid.validate(),
            Err(ContentRegistryError::UnsupportedVersion { version: 2 })
        );
    }
}
