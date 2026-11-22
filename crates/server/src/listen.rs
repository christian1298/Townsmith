//! Listen mode: accept protocol v0 sessions for one authoritative world and
//! optionally persist it.
//!
//! The loop is sequential (one client at a time), which matches the v0
//! executor: there is no interest management, authentication, or multi-client
//! broadcast yet. The world survives between connections of the same process.
//!
//! When a save path is configured, the world is saved on every non-empty
//! session end (disconnect, EOF, an idle timeout, or a failed response write)
//! and optionally every `autosave_ticks` accepted tick changes. A session that
//! read no command frame (a port probe or cancelled connect) writes no save.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;

use crate::protocol::{
    configure_stream, decode_client_command, read_frame, read_json_frame, write_json_frame,
    ClientCommand, FrameError, ServerMessage, LOOPBACK_SEED, PROTOCOL_VERSION, SESSION_IO_TIMEOUT,
};
use crate::save::{temp_path, SaveReport};
use crate::world::ServerWorld;

/// Options for one listen run.
#[derive(Debug, Clone, Default)]
pub struct ListenConfig {
    /// Save path. `None` keeps the world in memory only.
    pub save_path: Option<PathBuf>,
    /// Save after this many accepted tick changes. `0` disables periodic saves.
    pub autosave_ticks: u64,
}

/// Summary of one listen run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenReport {
    /// Connections accepted.
    pub sessions: u64,
    /// Command frames read.
    pub commands: u64,
    /// Successful writes to the save path.
    pub saves: u64,
    /// Final authoritative tick.
    pub tick: u64,
    /// Final world checksum.
    pub checksum: String,
}

/// Accept sequential sessions until `max_sessions` is reached.
///
/// The caller owns the bound listener so it can print the real address (useful
/// with port `0`). `max_sessions = None` serves until the process is stopped.
///
/// A session-level failure (socket configuration, a write to a peer that went
/// away, or a framing error) ends **only that session**; the accept loop keeps
/// serving. A save failure ends the loop with an error; the previous save and
/// its backup remain on disk.
pub fn run_listen(
    listener: TcpListener,
    mut world: ServerWorld,
    config: ListenConfig,
    max_sessions: Option<u64>,
) -> io::Result<ListenReport> {
    let mut sessions = 0u64;
    let mut commands = 0u64;
    let mut saves = 0u64;
    let mut last_saved_tick = world.tick;

    loop {
        let (stream, _peer) = listener.accept()?;
        if let Err(error) = configure_stream(&stream) {
            eprintln!("SERVER_LISTEN status=error phase=configure error={error}");
            continue;
        }
        sessions += 1;
        // A reconnect reuses the same in-memory world. This clears the closed
        // flag from the previous connection and, for a dropped socket, opens
        // the resume grace window.
        world.begin_connection();

        let session_commands = serve_session(
            &stream,
            &mut world,
            &config,
            &mut last_saved_tick,
            &mut saves,
        )?;
        commands = commands.saturating_add(session_commands);

        // A port probe (or any connect with no command frame) has nothing to
        // persist. Saving it would rotate the backup and rewrite an identical
        // file for every health check.
        if session_commands > 0 {
            if let Some(path) = &config.save_path {
                persist(&world, path, "disconnect")?;
                last_saved_tick = world.tick;
                saves += 1;
            }
        }

        if max_sessions.is_some_and(|limit| sessions >= limit) {
            break;
        }
    }

    Ok(ListenReport {
        sessions,
        commands,
        saves,
        tick: world.tick,
        checksum: world.checksum(),
    })
}

/// Serve one session on a borrowed stream without letting a peer-side write
/// failure escape as an error.
///
/// `TcpStream` implements `Read`/`Write` for `&TcpStream`, so both halves can
/// borrow the same socket; tests drive [`serve_io`] with independent reader and
/// writer types.
fn serve_session(
    stream: &TcpStream,
    world: &mut ServerWorld,
    config: &ListenConfig,
    last_saved_tick: &mut u64,
    saves: &mut u64,
) -> io::Result<u64> {
    let mut reader = stream;
    let mut writer = stream;
    serve_io(
        &mut reader,
        &mut writer,
        world,
        config,
        last_saved_tick,
        saves,
    )
}

/// Read and answer command frames until the peer leaves or the session closes.
///
/// Returns the number of command frames read. The only `Err` is a save failure:
/// a read error ends the session quietly and a response-write error (a reset or
/// half-closed peer, or an oversized response) ends it too, without taking down
/// the accept loop.
fn serve_io<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    world: &mut ServerWorld,
    config: &ListenConfig,
    last_saved_tick: &mut u64,
    saves: &mut u64,
) -> io::Result<u64> {
    let mut session_commands = 0u64;
    loop {
        let payload = match read_frame(&mut *reader) {
            Ok(payload) => payload,
            // A clean EOF at the frame boundary: the client closed without
            // `disconnect`.
            Err(FrameError::Truncated { actual: 0, .. }) => return Ok(session_commands),
            Err(FrameError::Io(error)) if session_ended(&error) => return Ok(session_commands),
            Err(error) => {
                eprintln!(
                    "SERVER_PROTOCOL status=error code={} message={}",
                    error.code(),
                    error.message()
                );
                return Ok(session_commands);
            }
        };
        session_commands += 1;

        let tick_before = world.tick;
        let responses = match decode_client_command(&payload) {
            Ok(command) => world.apply_all(&command),
            Err(rejection) => vec![rejection.into_message()],
        };
        for response in &responses {
            if let Err(error) = write_json_frame(&mut *writer, response) {
                eprintln!(
                    "SERVER_SESSION status=end reason=write_error code={} message={}",
                    error.code(),
                    error.message()
                );
                return Ok(session_commands);
            }
        }

        if world.is_closed() {
            return Ok(session_commands);
        }

        if config.autosave_ticks > 0
            && world.tick > tick_before
            && world.tick.saturating_sub(*last_saved_tick) >= config.autosave_ticks
        {
            if let Some(path) = &config.save_path {
                persist(world, path, "ticks")?;
                *last_saved_tick = world.tick;
                *saves += 1;
            }
        }
    }
}

fn session_ended(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::TimedOut
    )
}

fn persist(world: &ServerWorld, path: &Path, reason: &str) -> io::Result<SaveReport> {
    let report = world
        .save_to(path)
        .map_err(|error| io::Error::other(error.to_string()))?;
    println!(
        "SERVER_SAVE status=ok action=save reason={reason} path={} bytes={} tick={} checksum={} backup_rotated={}",
        path.display(),
        report.bytes,
        world.tick,
        world.checksum(),
        report.backup_rotated
    );
    Ok(report)
}

/// Report from the in-process listen/save loopback smoke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveSmokeReport {
    /// Final save file size.
    pub bytes: u64,
    /// Tick restored from disk.
    pub tick: u64,
    /// Checksum restored from disk.
    pub checksum: String,
    /// Backing file used by the smoke (already removed).
    pub path: PathBuf,
}

/// Exercise the real listen path over TCP loopback and then reload the file.
///
/// The server binds `127.0.0.1:0`, accepts one session, applies
/// hello → set_tax → place_road → disconnect, saves on disconnect, and exits.
/// The client then loads the save and requires the same tick and checksum. The
/// temp directory is removed before returning.
pub fn run_save_load_smoke() -> io::Result<SaveSmokeReport> {
    let directory =
        std::env::temp_dir().join(format!("townsmith-save-loopback-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory)?;
    let path = directory.join("world.json");

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let address = listener.local_addr()?;
    let config = ListenConfig {
        save_path: Some(path.clone()),
        autosave_ticks: 0,
    };

    let server = thread::spawn(move || {
        run_listen(listener, ServerWorld::new(LOOPBACK_SEED), config, Some(1))
    });
    let client_result = run_smoke_client(address);
    let server_result = server
        .join()
        .map_err(|_| io::Error::other("listen save smoke server thread panicked"))?;

    client_result?;
    let report = server_result?;

    let loaded =
        ServerWorld::load_from(&path).map_err(|error| io::Error::other(error.to_string()))?;
    if loaded.world.tick != report.tick || loaded.world.checksum() != report.checksum {
        return Err(io::Error::other(format!(
            "save smoke mismatch: server tick={} checksum={}, disk tick={} checksum={}",
            report.tick,
            report.checksum,
            loaded.world.tick,
            loaded.world.checksum()
        )));
    }
    if temp_path(&path).exists() {
        return Err(io::Error::other("save smoke left a .tmp file"));
    }

    let bytes = loaded.report.bytes;
    let checksum = loaded.world.checksum();
    let tick = loaded.world.tick;
    std::fs::remove_dir_all(&directory)?;
    Ok(SaveSmokeReport {
        bytes,
        tick,
        checksum,
        path,
    })
}

fn run_smoke_client(address: SocketAddr) -> io::Result<()> {
    let mut stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT)?;
    configure_stream(&stream)?;

    let mut reference = ServerWorld::new(LOOPBACK_SEED);
    let commands = [
        ClientCommand::Hello {
            protocol_version: PROTOCOL_VERSION,
            content_hash: "0123456789abcdef".repeat(4),
            player_name: "save-smoke".to_string(),
        },
        ClientCommand::SetTax {
            resident_rate: 12,
            business_rate: 8,
        },
        ClientCommand::PlaceRoad {
            start: [0, 0],
            end: [8, 0],
        },
        ClientCommand::Disconnect,
    ];

    for command in commands {
        let expected = reference.apply(&command);
        write_json_frame(&mut stream, &command)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let actual: ServerMessage =
            read_json_frame(&mut stream).map_err(|error| io::Error::other(error.to_string()))?;
        if actual != expected {
            return Err(io::Error::other(format!(
                "save smoke mismatch for `{}`: expected {expected:?}, received {actual:?}",
                command.tag()
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("townsmith-listen-{}-{unique}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp dir");
        path
    }

    fn client_exchange(
        address: SocketAddr,
        commands: &[ClientCommand],
    ) -> io::Result<Vec<ServerMessage>> {
        let mut stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT)?;
        configure_stream(&stream)?;
        let mut responses = Vec::with_capacity(commands.len());
        for command in commands {
            write_json_frame(&mut stream, command)
                .map_err(|error| io::Error::other(error.to_string()))?;
            responses.push(
                read_json_frame(&mut stream)
                    .map_err(|error| io::Error::other(error.to_string()))?,
            );
        }
        Ok(responses)
    }

    fn hello() -> ClientCommand {
        ClientCommand::Hello {
            protocol_version: PROTOCOL_VERSION,
            content_hash: "0123456789abcdef".repeat(4),
            player_name: "tester".to_string(),
        }
    }

    #[test]
    fn disconnect_saves_and_reload_resumes_the_tick() {
        let directory = temp_dir();
        let path = directory.join("world.json");
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let address = listener.local_addr().expect("address");
        let config = ListenConfig {
            save_path: Some(path.clone()),
            autosave_ticks: 0,
        };

        let server = thread::spawn(move || {
            run_listen(listener, ServerWorld::new(LOOPBACK_SEED), config, Some(1))
        });

        let responses = client_exchange(
            address,
            &[
                hello(),
                ClientCommand::SetTax {
                    resident_rate: 12,
                    business_rate: 8,
                },
                ClientCommand::PlaceRoad {
                    start: [0, 0],
                    end: [8, 0],
                },
                ClientCommand::Disconnect,
            ],
        )
        .expect("exchange");
        assert!(matches!(
            responses[0],
            ServerMessage::Welcome { tick: 0, .. }
        ));
        assert!(matches!(
            responses[1],
            ServerMessage::Snapshot { tick: 1, .. }
        ));
        assert!(matches!(
            responses[2],
            ServerMessage::Snapshot { tick: 2, .. }
        ));
        assert_eq!(responses[3], ServerMessage::Goodbye);

        let report = server.join().expect("server thread").expect("listen");
        assert_eq!(report.sessions, 1);
        assert_eq!(report.commands, 4);
        assert_eq!(report.saves, 1);
        assert_eq!(report.tick, 2);

        let loaded = ServerWorld::load_from(&path).expect("load");
        assert_eq!(loaded.world.tick, 2);
        assert_eq!(loaded.world.checksum(), report.checksum);
        assert_eq!(loaded.world.roads.roads.len(), 1);
        assert!(!temp_path(&path).exists());
        fs::remove_dir_all(&directory).expect("cleanup");
    }

    #[test]
    fn autosave_writes_once_per_configured_tick_window() {
        let directory = temp_dir();
        let path = directory.join("world.json");
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let address = listener.local_addr().expect("address");
        let config = ListenConfig {
            save_path: Some(path.clone()),
            autosave_ticks: 1,
        };

        let server = thread::spawn(move || {
            run_listen(listener, ServerWorld::new(LOOPBACK_SEED), config, Some(1))
        });

        // Only `set_tax` changes the tick; `ping` does not trigger an autosave.
        client_exchange(
            address,
            &[
                hello(),
                ClientCommand::SetTax {
                    resident_rate: 12,
                    business_rate: 8,
                },
                ClientCommand::Ping { nonce: 3 },
                ClientCommand::Disconnect,
            ],
        )
        .expect("exchange");

        let report = server.join().expect("server thread").expect("listen");
        assert_eq!(
            report.saves, 2,
            "one periodic save plus the disconnect save"
        );
        assert_eq!(report.tick, 1);
        assert!(path.is_file());
        fs::remove_dir_all(&directory).expect("cleanup");
    }

    #[test]
    fn save_load_smoke_is_deterministic() {
        let first = run_save_load_smoke().expect("first smoke");
        let second = run_save_load_smoke().expect("second smoke");
        assert_eq!(first.bytes, second.bytes);
        assert_eq!(first.tick, 2);
        assert_eq!(first.checksum, second.checksum);
        assert_eq!(first.path, second.path);
        assert_eq!(first.checksum.len(), 16);
    }
}
