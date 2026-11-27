//! Cross-checks the server's `--check-content-registry` selection against the
//! core D-042 parser for the same manifest.
//!
//! The server path is intentionally thin: it reads the file, calls
//! `parse_content_registry_manifest` / `ContentRegistryManifest::select`, and
//! prints the result. These tests protect that contract from drift by parsing
//! each fixture independently with `core_city`, then comparing the ordered IDs
//! and the documented deterministic checksum with the real binary output.

use std::fs;
use std::io::Read;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use core_city::content_registry::{
    parse_content_registry_manifest, ContentRegistryManifest, BASE_CONTENT_ID,
};

/// The binary under test, built by Cargo for this integration test.
const SERVER: &str = env!("CARGO_BIN_EXE_townsmith-server");

/// Upper bound for one check-mode process. Check mode never binds or loops, so
/// this only turns a regression into a failed test instead of a hang.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Process-unique fixture directory; removed on drop, including on panic.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "townsmith-content-consistency-{}-{unique}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp manifest directory");
        Self { path }
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.path.join(name);
        fs::write(&path, contents).expect("write temp manifest");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn path_arg(path: &Path) -> String {
    path.to_str().expect("utf-8 temp path").to_string()
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut raw = path.as_os_str().to_os_string();
    raw.push(suffix);
    PathBuf::from(raw)
}

/// A canonical 64-hex checksum that varies per fixture entry.
fn checksum(index: usize) -> String {
    format!("blake3:{index:064x}")
}

fn entry(id: &str, kind: &str, namespace: Option<&str>, checksum: &str) -> String {
    let namespace = namespace
        .map(|namespace| format!(r#","namespace":"{namespace}""#))
        .unwrap_or_default();
    format!(r#"{{"id":"{id}","kind":"{kind}"{namespace},"checksum":"{checksum}"}}"#)
}

fn manifest(entries: &[String]) -> String {
    format!(r#"{{"format_version":1,"ids":[{}]}}"#, entries.join(","))
}

fn check_args(path: &Path, requested: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "--check-content-registry".to_string(),
        "--content-registry".to_string(),
        path_arg(path),
    ];
    if let Some(requested) = requested {
        args.push("--content-ids".to_string());
        args.push(requested.to_string());
    }
    args
}

/// Run the real binary with a bounded wait and captured streams.
///
/// `yield_now` polls without a wall-clock sleep; the check mode exits as soon
/// as it has written its two status lines.
fn run_server(args: &[String]) -> Output {
    let mut child = Command::new(SERVER)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn townsmith-server");

    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll townsmith-server") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("townsmith-server did not exit within {TIMEOUT:?}");
        }
        std::thread::yield_now();
    };

    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .expect("piped stdout")
        .read_to_end(&mut stdout)
        .expect("read stdout");
    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .expect("piped stderr")
        .read_to_end(&mut stderr)
        .expect("read stderr");

    Output {
        status,
        stdout,
        stderr,
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn line_field<'a>(line: &'a str, key: &str) -> &'a str {
    let prefix = format!("{key}=");
    line.split_whitespace()
        .find_map(|token| token.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("missing {key}= in {line}"))
}

struct CheckSuccess {
    manifest_ids: usize,
    selected: usize,
    checksum: String,
    content_ids: Vec<String>,
}

fn parse_check_success(output: &Output) -> CheckSuccess {
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout: {} stderr: {}",
        stdout(output),
        stderr(output)
    );
    assert!(
        stderr(output).is_empty(),
        "no diagnostics on success: {}",
        stderr(output)
    );

    let text = stdout(output);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "exactly two status lines: {text}");
    assert!(
        lines[0].starts_with("SERVER_CONTENT_REGISTRY status=ok "),
        "{text}"
    );

    let manifest_ids = line_field(lines[0], "ids")
        .parse::<usize>()
        .expect("manifest id count");
    let selected = line_field(lines[0], "selected")
        .parse::<usize>()
        .expect("selected id count");
    let checksum = line_field(lines[0], "checksum").to_string();
    let ids = lines[1]
        .strip_prefix("SERVER_CONTENT status=ok ids=")
        .unwrap_or_else(|| panic!("missing SERVER_CONTENT success line: {text}"));
    let content_ids = if ids.is_empty() {
        Vec::new()
    } else {
        ids.split(',').map(str::to_string).collect()
    };

    CheckSuccess {
        manifest_ids,
        selected,
        checksum,
        content_ids,
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Reference implementation of the docs/84 §4 registry checksum, built from
/// the core-parsed manifest and the core-selected ordered IDs.
fn expected_checksum(manifest: &ContentRegistryManifest, selected: &[String]) -> String {
    let mut summary = String::new();
    for id in selected {
        if id.as_str() == BASE_CONTENT_ID {
            summary.push_str("base\tbase\t\t\n");
            continue;
        }
        let entry = manifest
            .ids
            .iter()
            .find(|entry| entry.id.as_str() == id.as_str())
            .unwrap_or_else(|| panic!("selected id `{id}` missing from parsed manifest"));
        summary.push_str(&entry.id);
        summary.push('\t');
        summary.push_str(entry.kind.as_str());
        summary.push('\t');
        summary.push_str(entry.namespace.as_deref().unwrap_or(""));
        summary.push('\t');
        summary.push_str(entry.checksum.as_deref().unwrap_or(""));
        summary.push('\n');
    }
    format!("{:016x}", fnv1a64(summary.as_bytes()))
}

/// Mirrors the server's `parse_content_ids` tokenization over a requested list.
fn requested_tokens(raw: &str) -> Vec<&str> {
    raw.split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect()
}

fn core_selection(manifest: &ContentRegistryManifest, requested: Option<&str>) -> Vec<String> {
    match requested {
        Some(raw) => manifest
            .select(&requested_tokens(raw))
            .expect("core parser selection"),
        None => manifest.implicit_ids(),
    }
}

fn assert_success_matches_core(
    manifest: &ContentRegistryManifest,
    expected: &[String],
    output: &Output,
) {
    let actual = parse_check_success(output);
    assert_eq!(
        actual.manifest_ids,
        manifest.ids.len(),
        "manifest id count must match the core parse"
    );
    assert_eq!(
        actual.selected,
        expected.len(),
        "selected count must match the core selection"
    );
    assert_eq!(
        actual.content_ids.as_slice(),
        expected,
        "ordered selected ids must match the core selection"
    );
    assert_eq!(
        actual.checksum,
        expected_checksum(manifest, expected),
        "registry checksum must match the core-derived reference"
    );
}

fn core_failure_code(manifest_json: &str, requested: Option<&str>) -> &'static str {
    match parse_content_registry_manifest(manifest_json) {
        Ok(manifest) => {
            let raw = requested.expect("a selector is required for a selection failure");
            manifest
                .select(&requested_tokens(raw))
                .expect_err("core parser must reject the fixture")
                .code()
        }
        Err(error) => error.code(),
    }
}

fn assert_failure_matches_core(manifest_json: &str, requested: Option<&str>, path: &Path) {
    let expected_code = core_failure_code(manifest_json, requested);
    let output = run_server(&check_args(path, requested));

    assert_eq!(
        output.status.code(),
        Some(2),
        "stdout: {} stderr: {}",
        stdout(&output),
        stderr(&output)
    );
    let errors = stderr(&output);
    let lines: Vec<&str> = errors.lines().collect();
    assert_eq!(lines.len(), 1, "one stable error line: {errors}");
    assert!(
        lines[0].starts_with("SERVER_CONTENT_REGISTRY status=error "),
        "{errors}"
    );
    assert_eq!(
        line_field(lines[0], "code"),
        expected_code,
        "server marker and core error code must agree: {errors}"
    );
    assert!(
        !stdout(&output).contains("SERVER_CONTENT status=ok"),
        "a rejected registry prints no success line: {}",
        stdout(&output)
    );
}

#[test]
fn one_mod_selection_matches_core_and_repeats() {
    let dir = TempDir::new("one-mod");
    let json = manifest(&[entry("mymod", "mod", Some("mymod"), &checksum(1))]);
    let path = dir.write("registry.json", &json);
    let manifest = parse_content_registry_manifest(&json).expect("core parse");
    let expected = core_selection(&manifest, None);
    assert_eq!(expected.join(","), "base,mymod");

    let first = run_server(&check_args(&path, None));
    let second = run_server(&check_args(&path, None));

    assert_success_matches_core(&manifest, &expected, &first);
    assert_success_matches_core(&manifest, &expected, &second);
    assert_eq!(
        parse_check_success(&first).checksum,
        "c17fdbc47a3e4002",
        "docs/84 §4 checksum golden for base+mymod"
    );
    assert_eq!(
        first.stdout, second.stdout,
        "two runs must print identical status bytes"
    );
}

#[test]
fn one_resource_pack_selection_matches_core() {
    let dir = TempDir::new("one-pack");
    let json = manifest(&[entry(
        "mymod:pack",
        "resource_pack",
        Some("mymod:pack"),
        &checksum(2),
    )]);
    let path = dir.write("registry.json", &json);
    let manifest = parse_content_registry_manifest(&json).expect("core parse");
    let expected = core_selection(&manifest, None);
    assert_eq!(expected.join(","), "base,mymod:pack");

    let output = run_server(&check_args(&path, None));
    assert_success_matches_core(&manifest, &expected, &output);
}

#[test]
fn shared_namespace_manifest_and_subset_ids_match_core() {
    let dir = TempDir::new("shared-namespace");
    let json = manifest(&[
        entry("mymod", "mod", Some("mymod"), &checksum(1)),
        entry(
            "mymod:pack",
            "resource_pack",
            Some("mymod:pack"),
            &checksum(2),
        ),
        entry("other", "mod", None, &checksum(3)),
    ]);
    let path = dir.write("registry.json", &json);
    let manifest = parse_content_registry_manifest(&json).expect("core parse");

    let all = core_selection(&manifest, Some("mymod:pack, mymod, base, other"));
    assert_eq!(all.join(","), "base,mymod,mymod:pack,other");
    let output = run_server(&check_args(&path, Some("mymod:pack, mymod, base, other")));
    assert_success_matches_core(&manifest, &all, &output);

    // The exact ID wins over the shared namespace: selecting `mymod` does not
    // pull in `mymod:pack`, and the server must agree.
    let mod_only = core_selection(&manifest, Some("mymod"));
    assert_eq!(mod_only.join(","), "base,mymod");
    let output = run_server(&check_args(&path, Some("mymod")));
    assert_success_matches_core(&manifest, &mod_only, &output);

    let pack_only = core_selection(&manifest, Some("mymod:pack"));
    assert_eq!(pack_only.join(","), "base,mymod:pack");
    let output = run_server(&check_args(&path, Some("mymod:pack")));
    assert_success_matches_core(&manifest, &pack_only, &output);
}

#[test]
fn max_entries_and_subset_selection_match_core() {
    let dir = TempDir::new("max-entries");
    let entries: Vec<String> = (0..256)
        .map(|index| entry(&format!("mod{index:03}"), "mod", None, &checksum(index)))
        .collect();
    let json = manifest(&entries);
    let path = dir.write("registry.json", &json);
    let manifest = parse_content_registry_manifest(&json).expect("core parse");
    assert_eq!(manifest.ids.len(), 256);

    let all = core_selection(&manifest, None);
    assert_eq!(all.len(), 257, "implicit base plus 256 entries");
    let output = run_server(&check_args(&path, None));
    assert_success_matches_core(&manifest, &all, &output);
    assert_eq!(
        parse_check_success(&output).checksum,
        "6e64cf4502bc3ba6",
        "docs/84 §4 checksum golden for base plus 256 mods"
    );

    let subset = core_selection(&manifest, Some("mod255, mod000, base, mod128"));
    assert_eq!(subset.join(","), "base,mod000,mod128,mod255");
    let first = run_server(&check_args(&path, Some("mod255, mod000, base, mod128")));
    let second = run_server(&check_args(&path, Some("mod255, mod000, base, mod128")));
    assert_success_matches_core(&manifest, &subset, &first);
    assert_success_matches_core(&manifest, &subset, &second);
    assert_eq!(first.stdout, second.stdout);
}

#[test]
fn unknown_id_failure_code_matches_core() {
    let dir = TempDir::new("unknown-id");
    let json = manifest(&[entry("mymod", "mod", None, &checksum(1))]);
    let path = dir.write("registry.json", &json);

    assert_eq!(
        core_failure_code(&json, Some("missing")),
        "content.unknown_id"
    );
    assert_failure_matches_core(&json, Some("missing"), &path);
}

#[test]
fn duplicate_id_failure_code_matches_core() {
    let dir = TempDir::new("duplicate-id");
    let json = manifest(&[
        entry("mymod", "mod", None, &checksum(1)),
        entry("mymod", "mod", None, &checksum(2)),
    ]);
    let path = dir.write("registry.json", &json);

    assert_eq!(core_failure_code(&json, None), "content.invalid_registry");
    assert_failure_matches_core(&json, None, &path);
}

#[test]
fn explicit_base_failure_code_matches_core() {
    let dir = TempDir::new("explicit-base");
    let json = manifest(&[entry("base", "mod", None, &checksum(1))]);
    let path = dir.write("registry.json", &json);

    assert_eq!(core_failure_code(&json, None), "content.invalid_registry");
    assert_failure_matches_core(&json, None, &path);
}

#[test]
fn bad_version_failure_code_matches_core() {
    let dir = TempDir::new("bad-version");
    let json = r#"{"format_version":2,"ids":[]}"#;
    let path = dir.write("registry.json", json);

    assert_eq!(
        core_failure_code(json, None),
        "content.unsupported_registry_version"
    );
    assert_failure_matches_core(json, None, &path);
}

#[test]
fn check_mode_never_binds_or_writes_save_artifacts() {
    // An occupied valid port proves no bind is attempted: if check mode reached
    // the listen path, binding would fail loudly instead of exiting 0.
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("occupy a loopback port");
    let port = listener
        .local_addr()
        .expect("local addr")
        .port()
        .to_string();

    let dir = TempDir::new("no-side-effects");
    let json = manifest(&[entry("mymod", "mod", None, &checksum(1))]);
    let path = dir.write("registry.json", &json);
    let save = dir.path.join("world.json");
    let registry_arg = path_arg(&path);
    let save_arg = path_arg(&save);
    let args = vec![
        "--check-content-registry".to_string(),
        "--content-registry".to_string(),
        registry_arg,
        "--listen".to_string(),
        port,
        "--once".to_string(),
        "--save".to_string(),
        save_arg,
    ];

    let manifest = parse_content_registry_manifest(&json).expect("core parse");
    let expected = manifest.implicit_ids();
    let output = run_server(&args);
    assert_success_matches_core(&manifest, &expected, &output);
    assert!(
        !stdout(&output).contains("SERVER_LISTEN"),
        "--listen is ignored: {}",
        stdout(&output)
    );
    assert!(!save.exists(), "--save is ignored: {}", save.display());
    for suffix in [".tmp", ".bak"] {
        let sibling = with_suffix(&save, suffix);
        assert!(
            !sibling.exists(),
            "no partial save artifact: {}",
            sibling.display()
        );
    }
    for entry in fs::read_dir(&dir.path).expect("read fixture directory") {
        let name = entry
            .expect("fixture directory entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        assert!(
            !name.ends_with(".tmp") && !name.ends_with(".bak"),
            "unexpected save artifact: {name}"
        );
    }

    drop(listener);
}
