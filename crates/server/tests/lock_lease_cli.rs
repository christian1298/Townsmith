//! Real-binary coverage for `--lock-lease-ticks`.
//!
//! These tests pin the operator-facing surface: invalid values fail closed with
//! `code=lock_lease.invalid` before the listener binds, a supplied value is
//! echoed as `SERVER_LOCK_LEASE ticks=N`, and a run without the flag keeps the
//! default startup lines unchanged.

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The binary under test, built by Cargo for this integration test.
const SERVER: &str = env!("CARGO_BIN_EXE_townsmith-server");

fn server(args: &[&str]) -> Output {
    Command::new(SERVER)
        .args(args)
        .output()
        .expect("run townsmith-server")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Read stdout lines until `marker` appears, the child exits, or the deadline
/// passes. The child is always killed and reaped.
fn run_until_line(args: &[&str], marker: &str) -> (Vec<String>, String) {
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

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut lines = Vec::new();
    while Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(line) => {
                lines.push(line);
                if lines.last().is_some_and(|line| line.starts_with(marker)) {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = stdout_reader.join();
    let stderr = stderr_reader.join().unwrap_or_default();
    (lines, stderr)
}

#[test]
fn invalid_values_exit_2_with_a_stable_code() {
    for bad in [
        vec!["--listen", "0", "--lock-lease-ticks", "0"],
        vec!["--listen", "0", "--lock-lease-ticks", "-1"],
        vec!["--listen", "0", "--lock-lease-ticks", "abc"],
        vec!["--listen", "0", "--lock-lease-ticks", "3601"],
        vec!["--listen", "0", "--lock-lease-ticks=600"],
        vec!["--listen", "0", "--lock-lease-ticks"],
    ] {
        let label = bad.join(" ");
        let output = server(&bad);
        assert_eq!(output.status.code(), Some(2), "{label}: {output:?}");
        let errors = stderr(&output);
        assert!(
            errors.contains("SERVER_LISTEN status=error code=lock_lease.invalid"),
            "{label}: {errors}"
        );
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains("SERVER_LISTEN status=ok"),
            "{label}: a rejected value must fail before binding"
        );
    }
}

#[test]
fn supplied_value_is_echoed_and_absent_value_keeps_the_default_lines() {
    let (lines, stderr) = run_until_line(
        &[
            "--listen",
            "0",
            "--sessions",
            "1",
            "--lock-lease-ticks",
            "600",
        ],
        "SERVER_LISTEN status=ok",
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "SERVER_LOCKS retention=off"),
        "the existing line stays byte-identical: {lines:?} {stderr}"
    );
    let marker = lines
        .iter()
        .position(|line| line.starts_with("SERVER_LOCK_LEASE"))
        .unwrap_or_else(|| panic!("missing marker: {lines:?} {stderr}"));
    assert_eq!(lines[marker], "SERVER_LOCK_LEASE ticks=600", "{lines:?}");
    assert!(
        lines[marker - 1].starts_with("SERVER_LOCKS retention="),
        "the marker follows SERVER_LOCKS: {lines:?}"
    );

    let (lines, stderr) = run_until_line(
        &["--listen", "0", "--sessions", "1"],
        "SERVER_LISTEN status=ok",
    );
    assert!(
        !lines
            .iter()
            .any(|line| line.starts_with("SERVER_LOCK_LEASE")),
        "absence means the 300-tick default and prints no marker: {lines:?} {stderr}"
    );
}
