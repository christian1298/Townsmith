//! Deterministic fault/security matrix for the bridge content-registry reader.
//!
//! The reader is pure with respect to the engine: `read_manifest_view` and
//! `manifest_view_from_str` never touch `RuntimeContent`, bind a listener, or
//! create files. These tests keep every case on that pure layer so they run
//! without a Godot interface table.

use super::content_registry_ui::{manifest_view_from_str, read_manifest_view};
use core_city::content_registry::MAX_CONTENT_REGISTRY_BYTES;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "townsmith-content-manifest-faults-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, bytes).expect("write temp file");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn checksum(byte: char) -> String {
    format!("blake3:{}", byte.to_string().repeat(64))
}

fn entry(id: &str, kind: &str, namespace: Option<&str>, checksum: &str) -> String {
    let namespace =
        namespace.map_or_else(String::new, |value| format!(r#","namespace":"{value}""#));
    format!(r#"{{"id":"{id}","kind":"{kind}"{namespace},"checksum":"{checksum}"}}"#)
}

fn manifest(version: u16, entries: &[String]) -> String {
    format!(
        r#"{{"format_version":{version},"ids":[{}]}}"#,
        entries.join(",")
    )
}

fn parse_error(json: &str) -> (&'static str, String) {
    manifest_view_from_str(json).expect_err("manifest must be rejected")
}

#[test]
fn accepts_a_canonical_manifest_and_sorts_ids() {
    let json = manifest(
        1,
        &[
            entry("textures", "resource_pack", None, &checksum('b')),
            entry("mymod", "mod", Some("mymod"), &checksum('a')),
            entry("mymod:extra", "mod", None, &checksum('c')),
        ],
    );
    let view = manifest_view_from_str(&json).expect("valid manifest");
    assert_eq!(view.format_version, 1);
    assert_eq!(
        view.ids
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        ["mymod", "mymod:extra", "textures"]
    );
    assert_eq!(view.implicit_ids.first().map(String::as_str), Some("base"));
    assert!(view.implicit_ids.contains(&"mymod".to_owned()));
    assert!(view.implicit_ids.contains(&"textures".to_owned()));
    assert!(view.checksum != 0);
}

#[test]
fn manifest_checksum_is_order_independent() {
    let a = entry("alpha", "mod", None, &checksum('a'));
    let b = entry("beta", "resource_pack", Some("beta"), &checksum('b'));
    let forward = manifest_view_from_str(&manifest(1, &[a.clone(), b.clone()])).expect("forward");
    let reverse = manifest_view_from_str(&manifest(1, &[b, a])).expect("reverse");
    assert_eq!(forward, reverse);
}

#[test]
fn rejects_malformed_documents_with_stable_codes() {
    for json in [
        "",
        " ",
        "{",
        "[]",
        "null",
        "{}",
        r#"{"format_version":1,"ids":[]} trailing"#,
        r#"{"format_version":1,"ids":[],"unknown":true}"#,
        r#"{"format_version":1,"ids":[{"id":"a","kind":"mod","checksum":"blake3:00","extra":1}]}"#,
        r#"{"format_version":1,"format_version":1,"ids":[]}"#,
    ] {
        let (code, message) = parse_error(json);
        assert!(
            code == "content.invalid_registry" || code == "content.unsupported_registry_version",
            "unexpected code {code} for {json}"
        );
        assert!(!message.contains("townsmith-content-manifest-faults"));
    }
}

#[test]
fn rejects_unsupported_versions() {
    for version in [0, 2, u16::MAX] {
        let (code, _) = parse_error(&manifest(version, &[]));
        assert_eq!(code, "content.unsupported_registry_version");
    }
}

#[test]
fn rejects_invalid_entry_fields() {
    let good = checksum('a');
    let cases = [
        entry("base", "mod", None, &good),
        entry("", "mod", None, &good),
        entry("Upper", "mod", None, &good),
        entry("bad/id", "mod", None, &good),
        entry("bad..id", "mod", None, &good),
        entry("bad::id", "mod", None, &good),
        entry("mymod", "Mod", None, &good),
        entry("mymod", "unknown", None, &good),
        entry("mymod", "mod", Some("base"), &good),
        entry("mymod", "mod", Some("Bad"), &good),
        entry("mymod", "mod", None, ""),
        entry("mymod", "mod", None, "blake3:"),
        entry("mymod", "mod", None, &good.to_uppercase()),
        entry("mymod", "mod", None, "blake3:00"),
        entry(
            "mymod",
            "mod",
            None,
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ),
        entry("mymod", "mod", None, &format!("blake3:{}", "g".repeat(64))),
    ];
    for case in cases {
        let (code, _) = parse_error(&manifest(1, std::slice::from_ref(&case)));
        assert_eq!(code, "content.invalid_registry", "entry: {case}");
    }

    let mut too_long = "a".repeat(65);
    too_long.insert(0, 'm');
    let (code, _) = parse_error(&manifest(1, &[entry(&too_long, "mod", None, &good)]));
    assert_eq!(code, "content.invalid_registry");
}

#[test]
fn rejects_duplicate_ids_namespaces_and_conflicting_claims() {
    let cases = [
        vec![
            entry("mymod", "mod", None, &checksum('a')),
            entry("mymod", "mod", None, &checksum('b')),
        ],
        vec![
            entry("mymod", "mod", Some("shared"), &checksum('a')),
            entry("other", "mod", Some("shared"), &checksum('b')),
        ],
        vec![
            entry("mymod", "mod", Some("other"), &checksum('a')),
            entry("other", "mod", Some("mymod"), &checksum('b')),
        ],
    ];
    for entries in cases {
        let (code, _) = parse_error(&manifest(1, &entries));
        assert_eq!(code, "content.invalid_registry");
    }
}

#[test]
fn file_reader_rejects_missing_directory_empty_and_non_utf8() {
    let temp = TempDir::new();
    let missing = temp.path("missing.json");
    let (code, message) = read_manifest_view(&missing).expect_err("missing file");
    assert_eq!(code, "content.registry_io");
    assert!(!message.contains(missing.to_string_lossy().as_ref()));

    let (code, _) = read_manifest_view(&temp.0).expect_err("directory");
    assert_eq!(code, "content.registry_io");

    let empty = temp.write("empty.json", b"");
    let (code, _) = read_manifest_view(&empty).expect_err("empty");
    assert_eq!(code, "content.invalid_registry");

    let truncated = temp.write("truncated.json", br#"{"format_version":1,"ids":["#);
    let (code, _) = read_manifest_view(&truncated).expect_err("truncated");
    assert_eq!(code, "content.invalid_registry");

    let non_utf8 = temp.write("non-utf8.json", &[0xff, 0xfe, 0x00]);
    let (code, _) = read_manifest_view(&non_utf8).expect_err("non utf8");
    assert_eq!(code, "content.registry_io");
}

#[test]
fn file_reader_enforces_the_size_cap() {
    let temp = TempDir::new();
    let oversized = temp.path("oversized.json");
    fs::write(&oversized, vec![b' '; MAX_CONTENT_REGISTRY_BYTES + 1]).expect("write oversize");
    let (code, _) = read_manifest_view(&oversized).expect_err("oversize");
    assert_eq!(code, "content.registry_io");

    let at_cap = temp.path("at-cap.json");
    fs::write(&at_cap, vec![b' '; MAX_CONTENT_REGISTRY_BYTES]).expect("write at cap");
    let (code, _) = read_manifest_view(&at_cap).expect_err("whitespace at cap");
    assert_eq!(code, "content.invalid_registry");
}

#[test]
fn file_reader_handles_spaces_and_missing_paths_without_side_effects() {
    let temp = TempDir::new();
    let before = fs::read_dir(&temp.0).expect("read dir").count();
    let spaced = temp.write(
        "manifest with spaces.json",
        manifest(1, &[entry("mymod", "mod", None, &checksum('a'))]).as_bytes(),
    );
    let view = read_manifest_view(&spaced).expect("spaced path");
    assert_eq!(view.ids[0].id, "mymod");
    let after = fs::read_dir(&temp.0).expect("read dir").count();
    assert_eq!(after, before + 1, "reader must not create extra files");
}

#[cfg(unix)]
#[test]
fn file_reader_handles_symlinks() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new();
    let valid = temp.write(
        "valid.json",
        manifest(1, &[entry("mymod", "mod", None, &checksum('a'))]).as_bytes(),
    );
    let link = temp.path("link.json");
    symlink(&valid, &link).expect("symlink valid");
    assert_eq!(
        read_manifest_view(&link).expect("valid symlink").ids[0].id,
        "mymod"
    );

    let dangling = temp.path("dangling.json");
    symlink(temp.path("missing.json"), &dangling).expect("symlink dangling");
    let (code, _) = read_manifest_view(&dangling).expect_err("dangling");
    assert_eq!(code, "content.registry_io");

    let looped = temp.path("loop.json");
    symlink(&looped, &looped).expect("symlink loop");
    let (code, _) = read_manifest_view(&looped).expect_err("loop");
    assert_eq!(code, "content.registry_io");
}

#[cfg(unix)]
#[test]
fn file_reader_handles_unreadable_files_when_permissions_apply() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new();
    let path = temp.write(
        "secret.json",
        manifest(1, &[entry("mymod", "mod", None, &checksum('a'))]).as_bytes(),
    );
    let original = fs::metadata(&path).expect("metadata").permissions();
    let mut denied = original.clone();
    denied.set_mode(0o000);
    fs::set_permissions(&path, denied).expect("chmod");
    let result = read_manifest_view(&path);
    fs::set_permissions(&path, original).expect("restore chmod");
    if let Err((code, _)) = result {
        assert_eq!(code, "content.registry_io");
    }
}

#[test]
fn missing_path_does_not_create_a_file_or_directory() {
    let temp = TempDir::new();
    let missing = temp.path("does-not-exist.json");
    assert!(!Path::new(&missing).exists());
    let _ = read_manifest_view(&missing);
    assert!(!missing.exists());
}

#[test]
fn repeated_reads_are_deterministic() {
    let temp = TempDir::new();
    let path = temp.write(
        "stable.json",
        manifest(1, &[entry("mymod", "mod", Some("mymod"), &checksum('a'))]).as_bytes(),
    );
    let first = read_manifest_view(&path).expect("first");
    for _ in 0..8 {
        assert_eq!(read_manifest_view(&path).expect("repeat"), first);
    }
}
