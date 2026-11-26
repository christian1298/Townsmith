//! Read-only D-042 content-registry manifest reader for the local editor.
//!
//! The live registry is managed by `content_load_dir`; this module only reads
//! one explicit manifest file and reports what it says. It never loads,
//! selects, merges, or activates content, never touches the live registry
//! slot, and never reads a pack directory. The file is bounded to
//! [`MAX_CONTENT_REGISTRY_BYTES`] before parsing with the pure core parser
//! [`parse_content_registry_manifest`].
//!
//! Errors carry a stable `content.*` code or `content.registry_io`, and never
//! echo the requested path, the raw file contents, or a JSON parser snippet
//! that could contain either.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use core_city::{
    parse_content_registry_manifest, ContentRegistryManifest, MAX_CONTENT_REGISTRY_BYTES,
};
use godot::prelude::*;

use super::fnv1a_push;

/// Stable code for a manifest file that cannot be opened, read, or decoded.
const CONTENT_REGISTRY_IO: &str = "content.registry_io";

/// One validated manifest entry in Rust form, so tests need no Godot engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestEntryView {
    /// Canonical registry key.
    pub id: String,
    /// `mod` or `resource_pack`.
    pub kind: &'static str,
    /// Optional matching namespace.
    pub namespace: Option<String>,
    /// Canonical `blake3:<64 lowercase hex>` checksum.
    pub checksum: Option<String>,
}

/// Parsed manifest payload plus its stable fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestView {
    /// Manifest schema version.
    pub format_version: u16,
    /// Entries sorted by ID.
    pub ids: Vec<ManifestEntryView>,
    /// `base` plus every manifest ID, sorted.
    pub implicit_ids: Vec<String>,
    /// FNV-1a fingerprint over the sorted entries.
    pub checksum: u64,
}

impl ManifestView {
    /// Project a validated core manifest into the read-only view.
    fn from_manifest(manifest: &ContentRegistryManifest) -> Self {
        let mut ids: Vec<ManifestEntryView> = manifest
            .ids
            .iter()
            .map(|entry| ManifestEntryView {
                id: entry.id.clone(),
                kind: entry.kind.as_str(),
                namespace: entry.namespace.clone(),
                checksum: entry.checksum.clone(),
            })
            .collect();
        ids.sort_by(|left, right| left.id.cmp(&right.id));
        let checksum = manifest_checksum(manifest.format_version, &ids);
        Self {
            format_version: manifest.format_version,
            ids,
            implicit_ids: manifest.implicit_ids(),
            checksum,
        }
    }
}

/// Stable 16-hex fingerprint over version, ID, kind, namespace, and checksum.
///
/// The caller passes entries sorted by ID, so manifest order does not affect
/// the result. Absent fields use a tag instead of an empty string to keep
/// `None` distinct from `Some("")`.
fn manifest_checksum(format_version: u16, entries: &[ManifestEntryView]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    fnv1a_push(&mut hash, b"content-registry-manifest");
    fnv1a_push(&mut hash, &format_version.to_le_bytes());
    for entry in entries {
        fnv1a_push(&mut hash, entry.id.as_bytes());
        fnv1a_push(&mut hash, &[0x1f]);
        fnv1a_push(&mut hash, entry.kind.as_bytes());
        fnv1a_push(&mut hash, &[0x1f]);
        match &entry.namespace {
            Some(namespace) => {
                fnv1a_push(&mut hash, b"namespace");
                fnv1a_push(&mut hash, namespace.as_bytes());
            }
            None => fnv1a_push(&mut hash, b"-"),
        }
        fnv1a_push(&mut hash, &[0x1f]);
        match &entry.checksum {
            Some(checksum) => {
                fnv1a_push(&mut hash, b"checksum");
                fnv1a_push(&mut hash, checksum.as_bytes());
            }
            None => fnv1a_push(&mut hash, b"-"),
        }
        fnv1a_push(&mut hash, &[0x1e]);
    }
    hash
}

/// Parse one manifest string into the read-only view.
///
/// Exposed to the tests so every validation case can run without a temporary
/// file; the Godot entry point reads a file and calls this.
pub(crate) fn manifest_view_from_str(json: &str) -> Result<ManifestView, (&'static str, String)> {
    parse_content_registry_manifest(json)
        .map(|manifest| ManifestView::from_manifest(&manifest))
        .map_err(|error| (error.code(), error.to_string()))
}

/// Read and validate one bounded manifest file.
///
/// At most [`MAX_CONTENT_REGISTRY_BYTES`] plus one byte is read, so an
/// oversized file is rejected without loading it. Error messages never
/// include the path, the raw file contents, or a parse snippet.
pub(crate) fn read_manifest_view(path: &Path) -> Result<ManifestView, (&'static str, String)> {
    let file = File::open(path).map_err(|error| {
        (
            CONTENT_REGISTRY_IO,
            format!("content registry read failed: {error}"),
        )
    })?;
    let limit = u64::try_from(MAX_CONTENT_REGISTRY_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes).map_err(|error| {
        (
            CONTENT_REGISTRY_IO,
            format!("content registry read failed: {error}"),
        )
    })?;
    if bytes.len() > MAX_CONTENT_REGISTRY_BYTES {
        return Err((
            CONTENT_REGISTRY_IO,
            format!("content registry manifest exceeds {MAX_CONTENT_REGISTRY_BYTES} bytes"),
        ));
    }
    let json = String::from_utf8(bytes).map_err(|_| {
        (
            CONTENT_REGISTRY_IO,
            "content registry manifest is not valid UTF-8".to_owned(),
        )
    })?;
    manifest_view_from_str(&json)
}

/// Optional string field as a Godot `null` when absent.
fn optional_string(value: Option<&str>) -> Variant {
    value.map_or_else(Variant::nil, |value| GString::from(value).to_variant())
}

/// Build the GDScript dictionary for one parsed manifest.
pub(crate) fn manifest_view_dictionary(view: &ManifestView) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("ok", true);
    result.set("format_version", i64::from(view.format_version));

    let mut ids = VarArray::new();
    for entry in &view.ids {
        let mut item = VarDictionary::new();
        item.set("id", &GString::from(entry.id.as_str()));
        item.set("kind", &GString::from(entry.kind));
        let namespace = optional_string(entry.namespace.as_deref());
        item.set("namespace", &namespace);
        let checksum = optional_string(entry.checksum.as_deref());
        item.set("checksum", &checksum);
        ids.push(&item);
    }
    result.set("ids", &ids);

    let mut implicit = VarArray::new();
    for id in &view.implicit_ids {
        implicit.push(&GString::from(id.as_str()));
    }
    result.set("implicit_ids", &implicit);
    result.set(
        "checksum",
        &GString::from(&format!("{:016x}", view.checksum)),
    );
    result
}

/// Read one manifest file and build the Godot payload.
pub(crate) fn content_registry_manifest_from_arg(
    raw_path: &str,
) -> Result<VarDictionary, (&'static str, String)> {
    let view = read_manifest_view(Path::new(raw_path.trim()))?;
    Ok(manifest_view_dictionary(&view))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "townsmith-content-registry-ui-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }

        fn write(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, contents).expect("write manifest");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn checksum(byte: &str) -> String {
        format!("blake3:{}", byte.repeat(64))
    }

    fn manifest(entries: &str) -> String {
        format!(r#"{{"format_version":1,"ids":[{entries}]}}"#)
    }

    #[test]
    fn reads_mod_and_resource_pack_preserving_kind_namespace_and_order() {
        let temp = TempDir::new();
        let path = temp.write(
            "registry.json",
            &manifest(&format!(
                r#"{{"id":"textures","kind":"resource_pack","namespace":"textures","checksum":"{}"}},
                   {{"id":"mymod","kind":"mod","checksum":"{}"}}"#,
                checksum("b"),
                checksum("a")
            )),
        );

        let view = read_manifest_view(&path).expect("valid manifest");
        assert_eq!(view.format_version, 1);
        assert_eq!(view.ids.len(), 2);
        assert_eq!(view.ids[0].id, "mymod");
        assert_eq!(view.ids[0].kind, "mod");
        assert_eq!(view.ids[0].namespace, None);
        assert_eq!(
            view.ids[0].checksum.as_deref(),
            Some(checksum("a").as_str())
        );
        assert_eq!(view.ids[1].id, "textures");
        assert_eq!(view.ids[1].kind, "resource_pack");
        assert_eq!(view.ids[1].namespace.as_deref(), Some("textures"));
        assert_eq!(view.implicit_ids, ["base", "mymod", "textures"]);
    }

    #[test]
    fn base_is_implicit_for_an_empty_manifest() {
        let view = manifest_view_from_str(&manifest("")).expect("empty manifest");
        assert!(view.ids.is_empty());
        assert_eq!(view.implicit_ids, ["base"]);
    }

    #[test]
    fn explicit_base_is_rejected() {
        let json = manifest(&format!(
            r#"{{"id":"base","kind":"mod","checksum":"{}"}}"#,
            checksum("a")
        ));
        let (code, _) = manifest_view_from_str(&json).expect_err("base is implicit");
        assert_eq!(code, "content.invalid_registry");
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let json = manifest(&format!(
            r#"{{"id":"mymod","kind":"mod","checksum":"{}"}},
               {{"id":"mymod","kind":"mod","checksum":"{}"}}"#,
            checksum("a"),
            checksum("b")
        ));
        let (code, _) = manifest_view_from_str(&json).expect_err("duplicate id");
        assert_eq!(code, "content.invalid_registry");
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let json = format!(
            r#"{{"format_version":2,"ids":[{{"id":"mymod","kind":"mod","checksum":"{}"}}]}}"#,
            checksum("a")
        );
        let (code, _) = manifest_view_from_str(&json).expect_err("unsupported version");
        assert_eq!(code, "content.unsupported_registry_version");
    }

    #[test]
    fn missing_file_reports_io_without_leaking_the_path() {
        let temp = TempDir::new();
        let secret = temp.0.join("secret-registry-name.json");
        let (code, message) = read_manifest_view(&secret).expect_err("missing file");
        assert_eq!(code, "content.registry_io");
        assert!(!message.contains("secret-registry-name"));
        assert!(!message.contains(temp.0.to_string_lossy().as_ref()));
    }

    #[test]
    fn parse_error_does_not_leak_the_path_or_contents() {
        let temp = TempDir::new();
        let path = temp.write("secret-registry-name.json", "not json at all");
        let (code, message) = read_manifest_view(&path).expect_err("invalid manifest");
        assert_eq!(code, "content.invalid_registry");
        assert!(!message.contains("secret-registry-name"));
        assert!(!message.contains("not json at all"));
    }

    #[test]
    fn oversized_file_is_rejected_as_io() {
        let temp = TempDir::new();
        let path = temp.0.join("oversized.json");
        let oversized = "x".repeat(MAX_CONTENT_REGISTRY_BYTES + 1);
        fs::write(&path, oversized).expect("write oversized");
        let (code, message) = read_manifest_view(&path).expect_err("oversized");
        assert_eq!(code, "content.registry_io");
        assert!(message.contains("exceeds"));
    }

    #[test]
    fn checksum_is_stable_and_order_independent() {
        let first = format!(
            r#"{{"id":"mymod","kind":"mod","checksum":"{}"}},
               {{"id":"textures","kind":"resource_pack","namespace":"textures","checksum":"{}"}}"#,
            checksum("a"),
            checksum("b")
        );
        let second = format!(
            r#"{{"id":"textures","kind":"resource_pack","namespace":"textures","checksum":"{}"}},
               {{"id":"mymod","kind":"mod","checksum":"{}"}}"#,
            checksum("b"),
            checksum("a")
        );
        let one = manifest_view_from_str(&manifest(&first)).expect("first");
        let two = manifest_view_from_str(&manifest(&second)).expect("second");
        assert_eq!(one.checksum, two.checksum);

        let different = manifest_view_from_str(&manifest(&format!(
            r#"{{"id":"mymod","kind":"mod","checksum":"{}"}}"#,
            checksum("c")
        )))
        .expect("different checksum");
        assert_ne!(one.checksum, different.checksum);
    }

    #[test]
    fn namespace_and_checksum_are_distinct_from_empty_strings() {
        let with_fields = manifest_view_from_str(&manifest(&format!(
            r#"{{"id":"mymod","kind":"mod","namespace":"mymod","checksum":"{}"}}"#,
            checksum("a")
        )))
        .expect("with fields");
        let without_namespace = manifest_view_from_str(&manifest(&format!(
            r#"{{"id":"mymod","kind":"mod","checksum":"{}"}}"#,
            checksum("a")
        )))
        .expect("without namespace");
        assert_ne!(with_fields.checksum, without_namespace.checksum);
    }
}
