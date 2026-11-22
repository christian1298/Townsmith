//! Headless Townsmith server tick harness.
//!
//! Phase 0: builds a cohort-scale city, runs a fixed number of ticks on the
//! CPU, and emits JSON. `--loopback-smoke` exercises protocol v0 over a local
//! socket, and `--listen PORT` serves the authoritative world with optional
//! persistence.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::time::Instant;

use core_sim::{Cohort, CohortSimulation};
use townsmith_server::listen::{run_listen, run_save_load_smoke, ListenConfig};
use townsmith_server::protocol::{run_loopback_smoke, LOOPBACK_SEED};
use townsmith_server::save::LoadedWorld;
use townsmith_server::world::{run_authority_loopback, ServerWorld};

fn arg_value(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn parse_arg(name: &str, default: usize) -> usize {
    arg_value(name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn has_flag(name: &str) -> bool {
    std::env::args().any(|argument| argument == name)
}

fn percentile(samples: &[f64], fraction: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let index = ((samples.len() - 1) as f64 * fraction).round() as usize;
    samples[index]
}

/// Serve protocol v0 sessions sequentially on the IPv4 loopback interface
/// until the process is stopped. v0 is single-connection-at-a-time by design.
fn main() {
    if has_flag("--loopback-smoke") {
        match run_loopback_smoke() {
            Ok(report) => {
                println!(
                    "SERVER_PROTOCOL status=ok frames={} checksum={}",
                    report.frames, report.checksum
                );
            }
            Err(error) => {
                eprintln!("SERVER_PROTOCOL status=error error={error}");
                std::process::exit(1);
            }
        }
        match run_authority_loopback() {
            Ok(report) => {
                println!(
                    "SERVER_AUTHORITY status=ok commands={} checksum={}",
                    report.commands, report.checksum
                );
            }
            Err(error) => {
                eprintln!("SERVER_AUTHORITY status=error error={error}");
                std::process::exit(1);
            }
        }
        return;
    }

    if has_flag("--save-load-smoke") {
        match run_save_load_smoke() {
            Ok(report) => println!(
                "SERVER_SAVE status=ok action=loopback bytes={} tick={} checksum={}",
                report.bytes, report.tick, report.checksum
            ),
            Err(error) => {
                eprintln!("SERVER_SAVE status=error action=loopback error={error}");
                std::process::exit(1);
            }
        }
        return;
    }

    if let Some(port) = arg_value("--listen") {
        run_listen_mode(&port);
        return;
    }

    if let Some(path) = arg_value("--load") {
        let path = PathBuf::from(path);
        match ServerWorld::load_from(&path) {
            Ok(loaded) => print_load(&path, &loaded),
            Err(error) => {
                eprintln!(
                    "SERVER_SAVE status=error action=load path={} code={} error={error}",
                    path.display(),
                    error.code()
                );
                std::process::exit(1);
            }
        }
        return;
    }

    run_harness();
}

fn run_listen_mode(port: &str) {
    let Ok(port) = port.parse::<u16>() else {
        eprintln!("SERVER_LISTEN status=error error=invalid port `{port}`");
        std::process::exit(1);
    };
    let bind = arg_value("--bind").unwrap_or_else(|| Ipv4Addr::LOCALHOST.to_string());
    let Ok(ip) = bind.parse::<IpAddr>() else {
        eprintln!("SERVER_LISTEN status=error error=invalid bind address `{bind}`");
        std::process::exit(1);
    };
    let address = SocketAddr::new(ip, port);

    let load_path = arg_value("--load").map(PathBuf::from);
    let save_path = arg_value("--save")
        .map(PathBuf::from)
        .or_else(|| load_path.clone());
    let seed = arg_value("--seed")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(LOOPBACK_SEED);
    let autosave_ticks = arg_value("--autosave-ticks")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let once = has_flag("--once");

    let world = match &load_path {
        Some(path) => match ServerWorld::load_from(path) {
            Ok(loaded) => {
                print_load(path, &loaded);
                loaded.world
            }
            Err(error) => {
                eprintln!(
                    "SERVER_SAVE status=error action=load path={} code={} error={error}",
                    path.display(),
                    error.code()
                );
                std::process::exit(1);
            }
        },
        None => match &save_path {
            Some(path) if path.exists() => match ServerWorld::load_from(path) {
                Ok(loaded) => {
                    print_load(path, &loaded);
                    loaded.world
                }
                Err(error) => {
                    eprintln!(
                        "SERVER_SAVE status=error action=load path={} code={} error={error}",
                        path.display(),
                        error.code()
                    );
                    std::process::exit(1);
                }
            },
            _ => ServerWorld::new(seed),
        },
    };

    let listener = match TcpListener::bind(address) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("SERVER_LISTEN status=error address={address} error={error}");
            std::process::exit(1);
        }
    };
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => {
            eprintln!("SERVER_LISTEN status=error error={error}");
            std::process::exit(1);
        }
    };

    println!(
        "SERVER_LISTEN status=ok address={address} seed={} tick={} checksum={} save={} autosave_ticks={}",
        world.seed,
        world.tick,
        world.checksum(),
        save_path
            .as_ref()
            .map_or_else(|| "none".to_string(), |path| path.display().to_string()),
        autosave_ticks
    );

    let config = ListenConfig {
        save_path,
        autosave_ticks,
    };
    match run_listen(listener, world, config, once.then_some(1)) {
        Ok(report) => println!(
            "SERVER_LISTEN status=ok sessions={} commands={} saves={} tick={} checksum={}",
            report.sessions, report.commands, report.saves, report.tick, report.checksum
        ),
        Err(error) => {
            eprintln!("SERVER_SAVE status=error action=save error={error}");
            std::process::exit(1);
        }
    }
}

fn print_load(path: &Path, loaded: &LoadedWorld) {
    println!(
        "SERVER_SAVE status=ok action=load path={} bytes={} tick={} checksum={} recovered_from_backup={}",
        path.display(),
        loaded.report.bytes,
        loaded.world.tick,
        loaded.world.checksum(),
        loaded.report.recovered_from_backup
    );
}

fn run_harness() {
    let cohort_count = parse_arg("--cohorts", 10_000).max(1);
    let district_count = parse_arg("--districts", 128).max(1);
    let ticks = parse_arg("--ticks", 100).max(1);
    let citizens_per_cohort = 100u32;

    let mut sim = CohortSimulation::new(district_count);
    for index in 0..cohort_count {
        sim.add_cohort(Cohort::new(
            (index % district_count) as u32,
            citizens_per_cohort,
            0.8,
        ))
        .expect("district exists");
    }

    let mut samples = Vec::with_capacity(ticks);
    let start = Instant::now();
    for _ in 0..ticks {
        let tick_start = Instant::now();
        sim.tick(1.0);
        samples.push(tick_start.elapsed().as_secs_f64() * 1000.0);
    }
    let total_ms = start.elapsed().as_secs_f64() * 1000.0;
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    println!(
        concat!(
            "{{\"server\":\"townsmith-server\",\"mode\":\"headless\",",
            "\"cohorts\":{},\"districts\":{},\"ticks\":{},",
            "\"citizens\":{},\"tick_ms_median\":{:.4},",
            "\"tick_ms_p95\":{:.4},\"total_ms\":{:.3},",
            "\"avg_happiness\":{:.4},\"avg_need\":{:.4}}}"
        ),
        sim.cohort_count(),
        district_count,
        ticks,
        sim.total_population(),
        percentile(&samples, 0.50),
        percentile(&samples, 0.95),
        total_ms,
        sim.average_happiness(),
        sim.average_need()
    );
}
