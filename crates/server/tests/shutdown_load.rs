//! Shutdown-save coverage under real sockets.
//!
//! Unix cases send SIGTERM while clients are connected and assert the process
//! exits 0, writes one atomic save, closes every socket, and leaves no temp
//! file. The portable case uses `--sessions 1` and a normal disconnect.

use std::io::{BufRead, BufReader};
#[cfg(unix)]
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use townsmith_server::protocol::{
    read_json_frame, write_json_frame, ClientCommand, ServerMessage, PROTOCOL_VERSION,
    SESSION_IO_TIMEOUT,
};
use townsmith_server::save::temp_path;
use townsmith_server::world::ServerWorld;

const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const BIN: &str = env!("CARGO_BIN_EXE_townsmith-server");

static NEXT: AtomicU64 = AtomicU64::new(1);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "townsmith-shutdown-load-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    fn save(&self) -> PathBuf {
        self.0.join("world.json")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Server {
    child: Child,
    address: SocketAddr,
    stdout: thread::JoinHandle<String>,
    stderr: thread::JoinHandle<String>,
}

impl Server {
    fn start(save: &Path, extra: &[&str]) -> Self {
        let mut command = Command::new(BIN);
        command
            .args(["--listen", "0", "--save"])
            .arg(save)
            .args(extra)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("spawn server");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");
        let (sender, receiver) = mpsc::channel();
        let stdout = thread::spawn(move || {
            let mut text = String::new();
            for line in BufReader::new(stdout).lines() {
                let line = line.expect("read stdout");
                if let Some(rest) = line.strip_prefix("SERVER_LISTEN status=ok address=") {
                    let address = rest
                        .split_whitespace()
                        .next()
                        .expect("address token")
                        .parse()
                        .expect("parse address");
                    let _ = sender.send(address);
                }
                text.push_str(&line);
                text.push('\n');
            }
            text
        });
        let stderr = thread::spawn(move || {
            let mut text = String::new();
            for line in BufReader::new(stderr).lines() {
                let line = line.expect("read stderr");
                text.push_str(&line);
                text.push('\n');
            }
            text
        });
        let address = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("server did not report SERVER_LISTEN");
        Self {
            child,
            address,
            stdout,
            stderr,
        }
    }

    fn wait(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status;
            }
            assert!(Instant::now() < deadline, "server did not exit in time");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn finish(self) -> (String, String) {
        (
            self.stdout.join().expect("stdout join"),
            self.stderr.join().expect("stderr join"),
        )
    }
}

fn connect(address: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(address).expect("connect");
    stream
        .set_read_timeout(Some(SESSION_IO_TIMEOUT))
        .expect("read timeout");
    stream
        .set_write_timeout(Some(SESSION_IO_TIMEOUT))
        .expect("write timeout");
    stream
}

fn send(stream: &mut TcpStream, command: &ClientCommand) {
    write_json_frame(stream, command).expect("write command");
}

fn hello(stream: &mut TcpStream, name: &str) {
    send(
        stream,
        &ClientCommand::Hello {
            protocol_version: PROTOCOL_VERSION,
            content_hash: CONTENT_HASH.to_string(),
            player_name: name.to_string(),
        },
    );
    assert!(matches!(
        read_json_frame::<_, ServerMessage>(stream).expect("welcome"),
        ServerMessage::Welcome { .. }
    ));
}

fn ping(stream: &mut TcpStream, nonce: u64) {
    send(stream, &ClientCommand::Ping { nonce });
    loop {
        match read_json_frame::<_, ServerMessage>(stream).expect("pong") {
            ServerMessage::Pong { nonce: seen } => {
                assert_eq!(seen, nonce);
                return;
            }
            ServerMessage::ChatSent { .. } => {}
            other => panic!("expected pong, got {other:?}"),
        }
    }
}

fn advance_tick(stream: &mut TcpStream) {
    send(
        stream,
        &ClientCommand::SetTax {
            resident_rate: 7,
            business_rate: 5,
        },
    );
    loop {
        match read_json_frame::<_, ServerMessage>(stream).expect("snapshot") {
            ServerMessage::Snapshot { .. } => return,
            ServerMessage::ChatSent { .. } => {}
            other => panic!("expected snapshot, got {other:?}"),
        }
    }
}

#[cfg(unix)]
fn terminate(pid: u32) {
    let status = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .expect("kill");
    assert!(status.success());
}

#[cfg(unix)]
fn assert_closed(stream: &mut TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut scratch = [0_u8; 256];
    loop {
        match stream.read(&mut scratch) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::UnexpectedEof
                ) =>
            {
                return;
            }
            Err(error) => panic!("unexpected read error after shutdown: {error}"),
        }
        assert!(Instant::now() < deadline, "client socket did not close");
    }
}

#[cfg(unix)]
#[test]
fn sigterm_saves_with_four_active_clients_and_closes_them() {
    let temp = TempDir::new();
    let save = temp.save();
    let mut server = Server::start(&save, &[]);
    let mut clients: Vec<TcpStream> = (0..4)
        .map(|index| {
            let mut stream = connect(server.address);
            hello(&mut stream, &format!("player-{index}"));
            stream
        })
        .collect();
    for (index, stream) in clients.iter_mut().enumerate() {
        ping(stream, index as u64 + 1);
        send(
            stream,
            &ClientCommand::Chat {
                text: format!("before shutdown {index}"),
            },
        );
        assert!(matches!(
            read_json_frame::<_, ServerMessage>(stream).expect("chat"),
            ServerMessage::ChatSent { .. }
        ));
    }
    advance_tick(&mut clients[0]);

    terminate(server.child.id());
    let status = server.wait(Duration::from_secs(15));
    assert!(status.success(), "server exited {status:?}");
    let (stdout, stderr) = server.finish();
    assert!(stderr.contains("SERVER_SHUTDOWN save=ok"), "{stderr}");
    assert!(stdout.contains("action=save reason=shutdown"), "{stdout}");
    assert!(!temp_path(&save).exists(), "shutdown left a .tmp file");

    let loaded = ServerWorld::load_from(&save).expect("load shutdown save");
    assert!(!loaded.report.recovered_from_backup);
    assert_eq!(loaded.world.players.count(), 4);
    assert_eq!(loaded.world.chat_seq(), 0, "chat is in-memory only");
    assert!(loaded.world.tick > 0);

    for stream in &mut clients {
        assert_closed(stream);
    }
}

#[cfg(unix)]
#[test]
fn sigterm_saves_while_one_client_is_not_reading() {
    let temp = TempDir::new();
    let save = temp.save();
    let mut server = Server::start(&save, &[]);
    let mut passive = connect(server.address);
    hello(&mut passive, "passive");
    let mut active = connect(server.address);
    hello(&mut active, "active");
    for index in 0..8 {
        send(
            &mut active,
            &ClientCommand::Chat {
                text: format!("load {index}"),
            },
        );
        assert!(matches!(
            read_json_frame::<_, ServerMessage>(&mut active).expect("chat"),
            ServerMessage::ChatSent { .. }
        ));
    }
    terminate(server.child.id());
    let status = server.wait(Duration::from_secs(15));
    assert!(status.success(), "server exited {status:?}");
    let (_, stderr) = server.finish();
    assert!(stderr.contains("SERVER_SHUTDOWN save=ok"), "{stderr}");
    let loaded = ServerWorld::load_from(&save).expect("load");
    assert_eq!(loaded.world.players.count(), 2);
    assert_closed(&mut passive);
    assert_closed(&mut active);
}

#[cfg(unix)]
#[test]
fn sigterm_saves_with_a_client_mid_frame() {
    let temp = TempDir::new();
    let save = temp.save();
    let mut server = Server::start(&save, &[]);
    let mut writer = connect(server.address);
    hello(&mut writer, "mid-frame");
    let mut raw = connect(server.address);
    raw.write_all(&64_u32.to_be_bytes()).expect("length");
    raw.write_all(b"{\"type\":\"ping\"").expect("partial");
    raw.flush().expect("flush");

    terminate(server.child.id());
    let status = server.wait(Duration::from_secs(15));
    assert!(status.success(), "server exited {status:?}");
    let (_, stderr) = server.finish();
    assert!(stderr.contains("SERVER_SHUTDOWN save=ok"), "{stderr}");
    let loaded = ServerWorld::load_from(&save).expect("load");
    assert_eq!(loaded.world.players.count(), 1);
    assert_closed(&mut raw);
    assert_closed(&mut writer);
}

#[test]
fn sessions_one_disconnect_save_is_portable() {
    let temp = TempDir::new();
    let save = temp.save();
    let mut server = Server::start(&save, &["--sessions", "1"]);
    let mut stream = connect(server.address);
    hello(&mut stream, "portable");
    ping(&mut stream, 1);
    advance_tick(&mut stream);
    send(&mut stream, &ClientCommand::Disconnect);
    assert!(matches!(
        read_json_frame::<_, ServerMessage>(&mut stream).expect("goodbye"),
        ServerMessage::Goodbye
    ));
    drop(stream);
    let status = server.wait(Duration::from_secs(15));
    assert!(status.success(), "server exited {status:?}");
    let (stdout, _) = server.finish();
    assert!(
        stdout.contains("SERVER_SAVE status=ok action=save reason=disconnect"),
        "{stdout}"
    );
    assert!(!temp_path(&save).exists(), "disconnect left a .tmp file");
    let loaded = ServerWorld::load_from(&save).expect("load");
    assert_eq!(loaded.world.players.count(), 1);
    assert!(loaded.world.tick > 0);
}
