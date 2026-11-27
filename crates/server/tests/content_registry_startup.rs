//! Real-binary startup/robustness matrix for `--content-registry` and
//! `--check-content-registry`.
//!
//! These tests exercise the operator-facing failure surface: path shapes,
//! manifest limits, stable status codes, exit codes, fail-closed ordering
//! before the listener binds, and the guarantee that a failed registry never
//! writes a world save. They complement the narrower `content_registry_cli.rs`
//! suite with filesystem and startup-order cases.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The binary under test, built by Cargo for this integration test.
const SERVER: &str = env!("CARGO_BIN_EXE_townsmith-server");

/// One canonical checksum shared by the fixtures.
const CHECKSUM: &str = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// Runner-facing manifest caps from docs/84 §8.
const MAX_MANIFEST_ENTRIES: usize = 256;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

/// Process-unique fixture directory; removed on drop, including on panic.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "townsmith-content-startup-{}-{unique}-{name}",
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

fn entry_json(id: &str, kind: &str) -> String {
    format!(r#"{{"id":"{id}","kind":"{kind}","checksum":"{CHECKSUM}"}}"#)
}

fn manifest(entries: &[(&str, &str)]) -> String {
    let entries: Vec<String> = entries
        .iter()
        .map(|(id, kind)| entry_json(id, kind))
        .collect();
    manifest_from_entries(&entries)
}

fn manifest_from_entries(entries: &[String]) -> String {
    format!(r#"{{"format_version":1,"ids":[{}]}}"#, entries.join(","))
}

fn manifest_with_version(version: u16) -> String {
    format!(r#"{{"format_version":{version},"ids":[]}}"#)
}

fn server(args: &[&str]) -> Output {
    Command::new(SERVER)
        .args(args)
        .output()
        .expect("run townsmith-server")
}

fn server_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(SERVER)
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run townsmith-server in a fixture directory")
}

fn check(args: &[&str]) -> Output {
    let mut all = vec!["--check-content-registry"];
    all.extend_from_slice(args);
    server(&all)
}

fn check_registry(registry: &str) -> Output {
    check(&["--content-registry", registry])
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

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut raw = path.as_os_str().to_os_string();
    raw.push(suffix);
    PathBuf::from(raw)
}

/// Assert a world save was never written, including its atomic `.tmp` and
/// rotation `.bak` siblings.
fn assert_save_absent(path: &Path) {
    assert!(!path.exists(), "save must not exist: {}", path.display());
    for suffix in [".tmp", ".bak"] {
        let sibling = with_suffix(path, suffix);
        assert!(
            !sibling.exists(),
            "no partial save artifact: {}",
            sibling.display()
        );
    }
}

fn assert_check_ok(output: &Output) {
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
    assert_eq!(lines.len(), 2, "exactly the two status lines: {text}");
    assert!(
        lines[0].starts_with("SERVER_CONTENT_REGISTRY status=ok "),
        "{text}"
    );
    assert!(lines[0].contains("checksum="), "{text}");
    assert!(
        lines[1].starts_with("SERVER_CONTENT status=ok ids=base"),
        "base is always first: {text}"
    );
}

/// Assert a startup failure uses the stable error line, exit 2, and no success
/// output. Registry failures must fail before the listener or a save exists.
fn assert_registry_error(output: &Output, code: &str) {
    assert_eq!(
        output.status.code(),
        Some(2),
        "stdout: {} stderr: {}",
        stdout(output),
        stderr(output)
    );
    let errors = stderr(output);
    let lines: Vec<&str> = errors.lines().collect();
    assert_eq!(lines.len(), 1, "one stable error line: {errors}");
    assert!(
        lines[0].starts_with("SERVER_CONTENT_REGISTRY status=error "),
        "{errors}"
    );
    assert!(lines[0].contains(&format!("code={code}")), "{errors}");
    assert!(lines[0].contains("error="), "{errors}");
    assert!(
        !stdout(output).contains("SERVER_CONTENT status=ok"),
        "a rejected registry prints no success line: {}",
        stdout(output)
    );
}

fn line_field<'a>(line: &'a str, key: &str) -> &'a str {
    let prefix = format!("{key}=");
    line.split_whitespace()
        .find_map(|token| token.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("missing {key}= in {line}"))
}

/// A bounded startup observation: read stdout lines until `marker`, the child
/// exits, or the deadline passes. The child is always killed and reaped.
struct StartupRun {
    lines: Vec<String>,
    stderr: String,
    status: ExitStatus,
    timed_out: bool,
}

fn run_until_line(args: &[&str], marker: &str) -> StartupRun {
    use std::io::{BufRead, BufReader, Read};

    let mut child = Command::new(SERVER)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn townsmith-server");
    let child_stdout = child.stdout.take().expect("piped stdout");
    let child_stderr = child.stderr.take().expect("piped stderr");

    let (sender, receiver) = mpsc::channel();
    let stdout_reader = std::thread::spawn(move || {
        for line in BufReader::new(child_stdout).lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = BufReader::new(child_stderr).read_to_string(&mut text);
        text
    });

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut lines = Vec::new();
    let mut timed_out = false;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            timed_out = true;
            break;
        }
        match receiver.recv_timeout(remaining) {
            Ok(line) => {
                let ready = line.starts_with(marker);
                lines.push(line);
                if ready {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                timed_out = true;
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = child.kill();
    let status = child.wait().expect("reap townsmith-server");
    stdout_reader.join().expect("stdout reader");
    let stderr = stderr_reader.join().expect("stderr reader");

    StartupRun {
        lines,
        stderr,
        status,
        timed_out,
    }
}

#[test]
fn valid_manifest_at_absolute_path_prints_the_two_stable_lines() {
    let dir = TempDir::new("valid-absolute");
    let manifest_path = dir.write(
        "registry.json",
        &manifest(&[("mymod", "mod"), ("mymod:pack", "resource_pack")]),
    );
    let registry = path_arg(&manifest_path);

    let output = check_registry(&registry);
    assert_check_ok(&output);
    let text = stdout(&output);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].contains(&format!("path={registry}")), "{text}");
    assert_eq!(lines[0].matches(&registry).count(), 1, "{text}");
    assert!(lines[0].contains("ids=2 selected=3"), "{text}");
    assert_eq!(
        lines[1], "SERVER_CONTENT status=ok ids=base,mymod,mymod:pack",
        "{text}"
    );
}

#[test]
fn manifest_path_with_spaces_is_accepted_verbatim() {
    let dir = TempDir::new("path with spaces");
    let manifest_path = dir.write("registry with spaces.json", &manifest(&[("mymod", "mod")]));
    let registry = path_arg(&manifest_path);

    let output = check_registry(&registry);
    assert_check_ok(&output);
    assert!(
        stdout(&output).contains(&format!("path={registry}")),
        "{}",
        stdout(&output)
    );
}

#[test]
fn relative_manifest_path_is_reported_without_canonicalization() {
    let dir = TempDir::new("relative");
    dir.write("registry.json", &manifest(&[("mymod", "mod")]));

    let output = server_in(
        &dir.path,
        &[
            "--check-content-registry",
            "--content-registry",
            "registry.json",
        ],
    );
    assert_check_ok(&output);
    let text = stdout(&output);
    assert!(text.contains("path=registry.json"), "{text}");
    assert!(
        !text.contains(&path_arg(&dir.path)),
        "relative input must not be rewritten to an absolute path: {text}"
    );
}

#[test]
fn missing_manifest_path_exits_2_with_registry_io() {
    let dir = TempDir::new("missing");
    let registry = path_arg(&dir.path.join("does-not-exist.json"));

    let output = check_registry(&registry);
    assert_registry_error(&output, "content.registry_io");
    assert!(
        stderr(&output).contains(&registry),
        "the diagnostic may name only the supplied path: {}",
        stderr(&output)
    );
}

#[test]
fn directory_manifest_path_exits_2_with_registry_io() {
    let dir = TempDir::new("directory");
    let registry = path_arg(&dir.path);

    let output = check_registry(&registry);
    assert_registry_error(&output, "content.registry_io");
}

#[cfg(unix)]
#[test]
fn unreadable_manifest_path_exits_2_with_registry_io() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new("unreadable");
    let manifest_path = dir.write("registry.json", &manifest(&[("mymod", "mod")]));
    let unreadable = std::fs::Permissions::from_mode(0o000);
    std::fs::set_permissions(&manifest_path, unreadable).expect("chmod 000");

    // Root ignores mode bits; the case is not meaningful there.
    if std::fs::File::open(&manifest_path).is_ok() {
        let _ = std::fs::set_permissions(&manifest_path, std::fs::Permissions::from_mode(0o600));
        eprintln!("skipping unreadable-manifest assertion: chmod 000 is still readable");
        return;
    }

    let registry = path_arg(&manifest_path);
    let output = check_registry(&registry);
    let _ = std::fs::set_permissions(&manifest_path, std::fs::Permissions::from_mode(0o600));

    assert_registry_error(&output, "content.registry_io");
}

#[cfg(unix)]
#[test]
fn symlink_to_valid_manifest_succeeds_without_rewriting_the_path() {
    let dir = TempDir::new("symlink-valid");
    let target = dir.write("registry.json", &manifest(&[("mymod", "mod")]));
    let link = dir.path.join("registry-link.json");
    std::os::unix::fs::symlink(&target, &link).expect("create symlink");
    let registry = path_arg(&link);

    let output = check_registry(&registry);
    assert_check_ok(&output);
    let text = stdout(&output);
    assert!(text.contains(&format!("path={registry}")), "{text}");
    assert!(
        !text.contains(&path_arg(&target)),
        "the symlink path is reported, not its target: {text}"
    );
}

#[cfg(unix)]
#[test]
fn symlink_to_missing_or_malformed_manifest_fails_closed() {
    let dir = TempDir::new("symlink-invalid");
    let dangling = dir.path.join("dangling.json");
    std::os::unix::fs::symlink(dir.path.join("missing.json"), &dangling).expect("create symlink");
    let output = check_registry(&path_arg(&dangling));
    assert_registry_error(&output, "content.registry_io");

    let malformed = dir.write("malformed.json", "{not json");
    let malformed_link = dir.path.join("malformed-link.json");
    std::os::unix::fs::symlink(&malformed, &malformed_link).expect("create symlink");
    let output = check_registry(&path_arg(&malformed_link));
    assert_registry_error(&output, "content.invalid_registry");
}

#[test]
fn explicit_base_entry_is_rejected_as_invalid_registry() {
    let dir = TempDir::new("explicit-base-entry");
    let manifest_path = dir.write("registry.json", &manifest(&[("base", "mod")]));
    let registry = path_arg(&manifest_path);

    let output = check_registry(&registry);
    assert_registry_error(&output, "content.invalid_registry");
    assert!(
        stderr(&output).contains("`base` is implicit"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn content_ids_subset_explicit_base_and_blank_tokens_normalize() {
    let dir = TempDir::new("selection");
    let manifest_path = dir.write(
        "registry.json",
        &manifest(&[("mymod", "mod"), ("zulu", "mod")]),
    );
    let registry = path_arg(&manifest_path);

    let subset = check(&[
        "--content-registry",
        &registry,
        "--content-ids",
        "zulu,mymod,base",
    ]);
    assert_check_ok(&subset);
    let text = stdout(&subset);
    assert!(text.contains("ids=2 selected=3"), "{text}");
    assert_eq!(
        text.lines().nth(1).expect("content line"),
        "SERVER_CONTENT status=ok ids=base,mymod,zulu"
    );

    for requested in ["base", " , , "] {
        let output = check(&["--content-registry", &registry, "--content-ids", requested]);
        assert_check_ok(&output);
        let text = stdout(&output);
        assert!(text.contains("ids=2 selected=1"), "{requested}: {text}");
        assert_eq!(
            text.lines().nth(1).expect("content line"),
            "SERVER_CONTENT status=ok ids=base",
            "{requested}"
        );
    }
}

#[test]
fn unknown_content_id_in_check_mode_is_unknown_id() {
    let dir = TempDir::new("unknown");
    let manifest_path = dir.write("registry.json", &manifest(&[("mymod", "mod")]));
    let registry = path_arg(&manifest_path);

    let output = check(&["--content-registry", &registry, "--content-ids", "missing"]);
    assert_registry_error(&output, "content.unknown_id");
}

#[test]
fn duplicate_manifest_id_is_rejected_as_invalid_registry() {
    let dir = TempDir::new("duplicate");
    let manifest_path = dir.write(
        "registry.json",
        &manifest_from_entries(&[entry_json("mymod", "mod"), entry_json("mymod", "mod")]),
    );
    let registry = path_arg(&manifest_path);

    let output = check_registry(&registry);
    assert_registry_error(&output, "content.invalid_registry");
    assert!(
        stderr(&output).contains("duplicate content id"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn cross_kind_claim_is_rejected_as_kind_mismatch() {
    let dir = TempDir::new("kind-mismatch");
    let manifest_path = dir.write(
        "registry.json",
        &manifest_from_entries(&[
            entry_json("mymod", "mod"),
            entry_json("mymod", "resource_pack"),
        ]),
    );
    let registry = path_arg(&manifest_path);

    let output = check_registry(&registry);
    assert_registry_error(&output, "content.kind_mismatch");
}

#[test]
fn format_versions_zero_and_two_are_unsupported() {
    let dir = TempDir::new("versions");
    for version in [0, 2] {
        let manifest_path = dir.write(
            &format!("registry-v{version}.json"),
            &manifest_with_version(version),
        );
        let output = check_registry(&path_arg(&manifest_path));
        assert_registry_error(&output, "content.unsupported_registry_version");
    }
}

#[test]
fn empty_ids_array_selects_base_only() {
    let dir = TempDir::new("empty-ids");
    let manifest_path = dir.write("registry.json", r#"{"format_version":1,"ids":[]}"#);
    let registry = path_arg(&manifest_path);

    let output = check_registry(&registry);
    assert_check_ok(&output);
    let text = stdout(&output);
    assert!(text.contains("ids=0 selected=1"), "{text}");
    assert_eq!(
        text.lines().nth(1).expect("content line"),
        "SERVER_CONTENT status=ok ids=base"
    );
}

#[test]
fn max_entries_256_accepted_257_rejected() {
    let dir = TempDir::new("entry-cap");
    let entries = |count: usize| -> Vec<String> {
        (0..count)
            .map(|index| entry_json(&format!("mod{index}"), "mod"))
            .collect()
    };

    let at_cap = dir.write("at-cap.json", &manifest_from_entries(&entries(256)));
    let output = check_registry(&path_arg(&at_cap));
    assert_check_ok(&output);
    assert!(
        stdout(&output).contains(&format!(
            "ids={MAX_MANIFEST_ENTRIES} selected={}",
            MAX_MANIFEST_ENTRIES + 1
        )),
        "{}",
        stdout(&output)
    );

    let over_cap = dir.write("over-cap.json", &manifest_from_entries(&entries(257)));
    let output = check_registry(&path_arg(&over_cap));
    assert_registry_error(&output, "content.invalid_registry");
    assert!(
        stderr(&output).contains(&format!("at most {MAX_MANIFEST_ENTRIES} entries")),
        "{}",
        stderr(&output)
    );
}

#[test]
fn manifest_over_one_mib_is_rejected_before_parsing() {
    let dir = TempDir::new("oversized");
    let mut json = String::from(r#"{"format_version":1,"ids":[]}"#);
    json.push_str(&" ".repeat(MAX_MANIFEST_BYTES + 1 - json.len()));
    assert_eq!(json.len(), MAX_MANIFEST_BYTES + 1);
    let manifest_path = dir.write("oversized.json", &json);

    let output = check_registry(&path_arg(&manifest_path));
    assert_registry_error(&output, "content.invalid_registry");
    assert!(
        stderr(&output).contains(&format!("manifest exceeds {MAX_MANIFEST_BYTES} bytes")),
        "{}",
        stderr(&output)
    );
}

#[test]
fn check_mode_requires_content_registry() {
    let output = check(&[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).is_empty(), "{}", stdout(&output));
    let errors = stderr(&output);
    assert!(
        errors.contains("SERVER_CONTENT_REGISTRY status=error"),
        "{errors}"
    );
    assert!(
        errors.contains("code=content.registry_required"),
        "{errors}"
    );
}

#[test]
fn check_mode_valid_manifest_ignores_listen_and_save() {
    // An occupied valid port proves no bind is attempted: if check mode reached
    // the listen path, binding would fail loudly instead of exiting 0.
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("occupy a loopback port");
    let port = listener
        .local_addr()
        .expect("local addr")
        .port()
        .to_string();

    let dir = TempDir::new("check-no-bind");
    let manifest_path = dir.write("registry.json", &manifest(&[("mymod", "mod")]));
    let registry = path_arg(&manifest_path);
    let save = dir.path.join("check-mode-must-not-save.json");
    let save_arg = path_arg(&save);

    let output = check(&[
        "--content-registry",
        &registry,
        "--listen",
        &port,
        "--once",
        "--save",
        &save_arg,
    ]);

    assert_check_ok(&output);
    assert!(
        !stdout(&output).contains("SERVER_LISTEN"),
        "--listen is ignored: {}",
        stdout(&output)
    );
    assert_save_absent(&save);
    drop(listener);
}

#[test]
fn failed_registry_in_listen_mode_exits_2_before_bind_and_writes_no_save() {
    let dir = TempDir::new("listen-failed-registry");
    let missing = path_arg(&dir.path.join("missing-registry.json"));
    let save = dir.path.join("must-not-exist.json");
    let save_arg = path_arg(&save);

    let output = server(&[
        "--listen",
        "0",
        "--content-registry",
        &missing,
        "--save",
        &save_arg,
    ]);

    assert_registry_error(&output, "content.registry_io");
    assert!(stdout(&output).is_empty(), "{}", stdout(&output));
    assert_save_absent(&save);
}

#[test]
fn occupied_port_with_failed_registry_exits_2_before_bind_and_writes_no_save() {
    // Registry validation precedes binding: even with the port occupied, the
    // registry failure (not a bind error) decides the exit code and no world is
    // saved.
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("occupy a loopback port");
    let port = listener
        .local_addr()
        .expect("local addr")
        .port()
        .to_string();

    let dir = TempDir::new("occupied-port-bad-registry");
    let manifest_path = dir.write("registry.json", &manifest_with_version(2));
    let registry = path_arg(&manifest_path);
    let save = dir.path.join("must-not-exist.json");
    let save_arg = path_arg(&save);

    let output = server(&[
        "--listen",
        &port,
        "--content-registry",
        &registry,
        "--save",
        &save_arg,
        "--once",
    ]);

    assert_registry_error(&output, "content.unsupported_registry_version");
    assert!(
        !stderr(&output).contains("SERVER_LISTEN"),
        "the bind path is never reached: {}",
        stderr(&output)
    );
    assert_save_absent(&save);
    drop(listener);
}

#[test]
fn occupied_port_with_valid_manifest_exits_1_and_writes_no_save() {
    // The registry is valid, so startup reaches the bind and fails there. A
    // bind failure is the existing exit-1 SERVER_LISTEN path (docs/84 reserves
    // exit 2 for registry failures, which fail earlier); either way it is
    // fail-closed and never writes a world save.
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("occupy a loopback port");
    let port = listener
        .local_addr()
        .expect("local addr")
        .port()
        .to_string();

    let dir = TempDir::new("occupied-port");
    let manifest_path = dir.write("registry.json", &manifest(&[("mymod", "mod")]));
    let registry = path_arg(&manifest_path);
    let save = dir.path.join("must-not-exist.json");
    let save_arg = path_arg(&save);

    let output = server(&[
        "--listen",
        &port,
        "--content-registry",
        &registry,
        "--save",
        &save_arg,
        "--once",
    ]);

    assert_eq!(
        output.status.code(),
        Some(1),
        "stdout: {} stderr: {}",
        stdout(&output),
        stderr(&output)
    );
    let out = stdout(&output);
    assert!(
        out.contains("SERVER_CONTENT_REGISTRY status=ok"),
        "the manifest is applied before the bind attempt: {out}"
    );
    assert!(
        out.contains("SERVER_CONTENT status=ok ids=base,mymod"),
        "{out}"
    );
    assert!(
        !out.contains("SERVER_LISTEN status=ok"),
        "the listener must not report success: {out}"
    );
    let errors = stderr(&output);
    assert!(errors.contains("SERVER_LISTEN status=error"), "{errors}");
    assert!(
        errors.contains(&format!("address=127.0.0.1:{port}")),
        "{errors}"
    );
    assert_save_absent(&save);
    drop(listener);
}

#[test]
fn valid_manifest_applies_on_listen_startup_before_bind() {
    let dir = TempDir::new("listen-ready");
    let manifest_path = dir.write("registry.json", &manifest(&[("mymod", "mod")]));
    let registry = path_arg(&manifest_path);

    let run = run_until_line(
        &["--listen", "0", "--content-registry", &registry],
        "SERVER_LISTEN status=ok",
    );

    assert!(
        !run.timed_out,
        "server did not become ready (status {:?}); lines: {:?} stderr: {}",
        run.status, run.lines, run.stderr
    );
    assert!(
        run.lines
            .last()
            .is_some_and(|line| line.starts_with("SERVER_LISTEN status=ok")),
        "lines: {:?} stderr: {}",
        run.lines,
        run.stderr
    );
    assert_eq!(
        run.lines.len(),
        4,
        "registry, content, locks, listen: {:?}",
        run.lines
    );
    assert!(
        run.lines[0].starts_with("SERVER_CONTENT_REGISTRY status=ok "),
        "{:?}",
        run.lines
    );
    assert!(
        run.lines[0].contains(&format!("path={registry}")),
        "{:?}",
        run.lines
    );
    assert_eq!(
        run.lines[1], "SERVER_CONTENT status=ok ids=base,mymod",
        "{:?}",
        run.lines
    );
    assert!(
        run.lines[2].starts_with("SERVER_LOCKS retention="),
        "{:?}",
        run.lines
    );
    assert!(
        run.lines[3].contains("seed=") && run.lines[3].contains("checksum="),
        "{:?}",
        run.lines
    );
    assert!(
        run.stderr.is_empty(),
        "no diagnostics on a clean startup: {}",
        run.stderr
    );
}

#[test]
fn checksum_is_stable_across_entry_order_and_repeated_runs() {
    let dir = TempDir::new("checksum-stability");
    let first_path = dir.write(
        "first.json",
        &manifest(&[("mymod", "mod"), ("mymod:pack", "resource_pack")]),
    );
    let reordered_path = dir.write(
        "reordered.json",
        &manifest(&[("mymod:pack", "resource_pack"), ("mymod", "mod")]),
    );

    let first = check_registry(&path_arg(&first_path));
    let repeated = check_registry(&path_arg(&first_path));
    let reordered = check_registry(&path_arg(&reordered_path));
    for output in [&first, &repeated, &reordered] {
        assert_check_ok(output);
    }

    let checksum = |output: &Output| -> String {
        let text = stdout(output);
        line_field(text.lines().next().expect("registry line"), "checksum").to_string()
    };
    assert_eq!(checksum(&first), checksum(&repeated));
    assert_eq!(checksum(&first), checksum(&reordered));
    assert_eq!(
        stdout(&first).lines().nth(1),
        stdout(&reordered).lines().nth(1)
    );
}
