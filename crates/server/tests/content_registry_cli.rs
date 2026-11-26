//! Integration tests for the non-binding `--check-content-registry` mode.
//!
//! Every test drives the real `townsmith-server` binary so flag parsing,
//! status lines, and exit codes stay pinned to the operator-facing surface.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

/// One canonical checksum shared by the fixtures.
const CHECKSUM: &str = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// Process-unique fixture directory; removed on drop, including on panic.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "townsmith-content-check-{}-{unique}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp manifest directory");
        Self { path }
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.path.join(name);
        std::fs::write(&path, contents).expect("write temp manifest");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn manifest(entries: &[(&str, &str)]) -> String {
    let entries: Vec<String> = entries
        .iter()
        .map(|(id, kind)| format!(r#"{{"id":"{id}","kind":"{kind}","checksum":"{CHECKSUM}"}}"#))
        .collect();
    format!(r#"{{"format_version":1,"ids":[{}]}}"#, entries.join(","))
}

fn server(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_townsmith-server"))
        .args(args)
        .output()
        .expect("run townsmith-server")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn path_arg(path: &Path) -> String {
    path.to_str().expect("utf-8 temp path").to_string()
}

#[test]
fn valid_manifest_selects_all_non_base_ids_and_prints_both_lines() {
    let dir = TempDir::new("all");
    let manifest_path = dir.write(
        "registry.json",
        &manifest(&[("mymod", "mod"), ("mymod:pack", "resource_pack")]),
    );
    let registry = path_arg(&manifest_path);

    let output = server(&["--check-content-registry", "--content-registry", &registry]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "exactly the two status lines: {stdout}");
    assert!(
        lines[0].starts_with("SERVER_CONTENT_REGISTRY status=ok"),
        "{stdout}"
    );
    assert!(lines[0].contains(&format!("path={registry}")), "{stdout}");
    assert!(lines[0].contains("ids=2"), "{stdout}");
    assert!(lines[0].contains("selected=3"), "{stdout}");
    assert!(lines[0].contains("checksum="), "{stdout}");
    assert_eq!(
        lines[1], "SERVER_CONTENT status=ok ids=base,mymod,mymod:pack",
        "implicit base plus every manifest id: {stdout}"
    );
    assert!(
        stderr(&output).is_empty(),
        "no diagnostics on success: {}",
        stderr(&output)
    );
}

#[test]
fn content_ids_selects_a_validated_sorted_subset() {
    let dir = TempDir::new("subset");
    let manifest_path = dir.write(
        "registry.json",
        &manifest(&[("mymod", "mod"), ("zulu", "mod")]),
    );
    let registry = path_arg(&manifest_path);

    let output = server(&[
        "--check-content-registry",
        "--content-registry",
        &registry,
        "--content-ids",
        "zulu,mymod,base",
    ]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(
        stdout.contains("ids=2 selected=3"),
        "manifest size and selected size differ: {stdout}"
    );
    assert!(
        stdout.contains("SERVER_CONTENT status=ok ids=base,mymod,zulu\n"),
        "selection is sorted: {stdout}"
    );
}

#[test]
fn unknown_content_id_exits_2_with_stable_code() {
    let dir = TempDir::new("unknown");
    let manifest_path = dir.write("registry.json", &manifest(&[("mymod", "mod")]));
    let registry = path_arg(&manifest_path);

    let output = server(&[
        "--check-content-registry",
        "--content-registry",
        &registry,
        "--content-ids",
        "missing",
    ]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("SERVER_CONTENT_REGISTRY status=error"),
        "{}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("code=content.unknown_id"),
        "{}",
        stderr(&output)
    );
    assert!(
        !stdout(&output).contains("SERVER_CONTENT status=ok"),
        "a rejected selection prints no success line: {}",
        stdout(&output)
    );
}

#[test]
fn missing_manifest_path_exits_2() {
    let dir = TempDir::new("missing-path");
    let registry = path_arg(&dir.path.join("does-not-exist.json"));

    let output = server(&["--check-content-registry", "--content-registry", &registry]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("code=content.registry_io"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn missing_content_registry_argument_exits_2() {
    let output = server(&["--check-content-registry"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("SERVER_CONTENT_REGISTRY status=error"),
        "{}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("code=content.registry_required"),
        "{}",
        stderr(&output)
    );
    assert!(stdout(&output).is_empty(), "{}", stdout(&output));
}

#[test]
fn unsupported_manifest_version_exits_2() {
    let dir = TempDir::new("version");
    let manifest_path = dir.write("registry.json", r#"{"format_version":2,"ids":[]}"#);
    let registry = path_arg(&manifest_path);

    let output = server(&["--check-content-registry", "--content-registry", &registry]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("code=content.unsupported_registry_version"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn check_mode_does_not_bind_the_requested_listen_port() {
    // Occupy a port for the duration of the run. If the check mode reached the
    // listen path, `TcpListener::bind` would fail with "address in use" and
    // exit 1. An invalid address would merely prove parsing stopped early; an
    // occupied valid port proves no bind was attempted at all.
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("occupy a loopback port");
    let port = listener
        .local_addr()
        .expect("local addr")
        .port()
        .to_string();

    let dir = TempDir::new("no-bind");
    let manifest_path = dir.write("registry.json", &manifest(&[("mymod", "mod")]));
    let registry = path_arg(&manifest_path);
    let save = path_arg(&dir.path.join("check-mode-must-not-save.json"));

    let output = server(&[
        "--check-content-registry",
        "--content-registry",
        &registry,
        "--listen",
        &port,
        "--once",
        "--save",
        &save,
    ]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(
        stdout.contains("SERVER_CONTENT_REGISTRY status=ok"),
        "{stdout}"
    );
    assert!(
        stdout.contains("SERVER_CONTENT status=ok ids=base,mymod"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("SERVER_LISTEN"),
        "--listen is ignored: {stdout}"
    );
    assert!(
        !Path::new(&save).exists(),
        "--save is ignored; no world is loaded or written"
    );

    drop(listener);
}
