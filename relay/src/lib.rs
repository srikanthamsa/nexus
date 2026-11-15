use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{self, ErrorKind, Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

pub const DEFAULT_PORT: u16 = 39210;

const MAX_JOIN_LINE: usize = 4_096;
const JOIN_TIMEOUT: Duration = Duration::from_secs(10);
const ROOM_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const ROOM_SWEEP: Duration = Duration::from_millis(250);
const MAX_ROOMS: usize = 1_024;
const ACCEPT_POLL: Duration = Duration::from_millis(20);
const RELAY_BUFFER: usize = 32 * 1024;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum JoinRequest {
    Host { room: String },
    Client { room: String },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum JoinReply<'a> {
    Waiting,
    Ready,
    Error { message: &'a str },
}

struct Room {
    host: TcpStream,
    created: Instant,
}

#[derive(Default)]
struct Rooms {
    inner: Mutex<HashMap<String, Room>>,
}

impl Rooms {
    fn sweep(&self, map: &mut HashMap<String, Room>) {
        map.retain(|_, room| room.created.elapsed() < ROOM_IDLE_TIMEOUT);
    }

    fn insert_host(&self, room: String, host: TcpStream) -> Result<(), &'static str> {
        let mut map = self.inner.lock().expect("relay rooms poisoned");
        self.sweep(&mut map);
        if map.contains_key(&room) {
            return Err("This invite is already waiting on the relay.");
        }
        if map.len() >= MAX_ROOMS {
            return Err("The relay is at capacity.");
        }
        map.insert(
            room,
            Room {
                host,
                created: Instant::now(),
            },
        );
        Ok(())
    }

    fn take_client_room(&self, room: &str) -> Option<TcpStream> {
        let mut map = self.inner.lock().expect("relay rooms poisoned");
        self.sweep(&mut map);
        map.remove(room).map(|room| room.host)
    }

    fn contains(&self, room: &str) -> bool {
        let map = self.inner.lock().expect("relay rooms poisoned");
        map.contains_key(room)
    }

    fn remove(&self, room: &str) {
        let mut map = self.inner.lock().expect("relay rooms poisoned");
        map.remove(room);
    }
}

fn normalize_room(room: &str) -> Result<String, &'static str> {
    let normalized = room.trim().to_ascii_lowercase();
    if normalized.len() != 32 || !normalized.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("The relay invite is invalid.");
    }
    Ok(normalized)
}

fn read_join(stream: &mut TcpStream) -> Result<JoinRequest, &'static str> {
    stream
        .set_read_timeout(Some(JOIN_TIMEOUT))
        .map_err(|_| "The relay could not read this request.")?;
    let mut line = Vec::with_capacity(128);
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => return Err("The relay request ended early."),
            Ok(_) => {
                if byte[0] == b'\n' {
                    break;
                }
                if byte[0] != b'\r' {
                    line.push(byte[0]);
                }
                if line.len() > MAX_JOIN_LINE {
                    return Err("The relay request is too large.");
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => return Err("The relay could not read this request."),
        }
    }
    serde_json::from_slice(&line).map_err(|_| "The relay did not understand this request.")
}

fn write_reply(stream: &mut TcpStream, reply: &JoinReply<'_>) -> io::Result<()> {
    let mut payload = serde_json::to_vec(reply)
        .map_err(|error| io::Error::new(ErrorKind::InvalidData, error.to_string()))?;
    payload.push(b'\n');
    stream.write_all(&payload)?;
    stream.flush()
}

fn reject(mut stream: TcpStream, message: &'static str) {
    let _ = write_reply(&mut stream, &JoinReply::Error { message });
    let _ = stream.shutdown(Shutdown::Both);
}

fn wait_for_pairing(rooms: &Rooms, room: &str, probe: &mut TcpStream) {
    let mut scratch = [0u8; 1];
    loop {
        if !rooms.contains(room) {
            return;
        }
        match probe.peek(&mut scratch) {
            Ok(0) => {
                rooms.remove(room);
                return;
            }
            Ok(_) => {}
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
            Err(_) => {
                rooms.remove(room);
                return;
            }
        }
        let map = rooms.inner.lock().expect("relay rooms poisoned");
        let expired = map
            .get(room)
            .is_none_or(|entry| entry.created.elapsed() >= ROOM_IDLE_TIMEOUT);
        drop(map);
        if expired {
            rooms.remove(room);
            return;
        }
        thread::sleep(ROOM_SWEEP);
    }
}

fn handle_host(mut stream: TcpStream, rooms: Arc<Rooms>, room: String) {
    let host = match stream.try_clone() {
        Ok(host) => host,
        Err(_) => return,
    };
    if let Err(message) = rooms.insert_host(room.clone(), host) {
        reject(stream, message);
        return;
    }
    if write_reply(&mut stream, &JoinReply::Waiting).is_err() {
        rooms.remove(&room);
        return;
    }
    let mut probe = match stream.try_clone() {
        Ok(probe) => probe,
        Err(_) => {
            rooms.remove(&room);
            return;
        }
    };
    // A read timeout keeps the shared socket blocking for the byte pump later;
    // peeking only detects whether the peer has gone away.
    let _ = probe.set_read_timeout(Some(ROOM_SWEEP));
    wait_for_pairing(&rooms, &room, &mut probe);
}

fn pump(mut from: TcpStream, mut to: TcpStream) {
    let mut buffer = vec![0u8; RELAY_BUFFER];
    loop {
        match from.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                if to.write_all(&buffer[..read]).is_err() {
                    break;
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    let _ = to.shutdown(Shutdown::Both);
    let _ = from.shutdown(Shutdown::Both);
}

fn handle_client(mut stream: TcpStream, rooms: Arc<Rooms>, room: String) {
    let Some(host) = rooms.take_client_room(&room) else {
        reject(stream, "No host is waiting for this invite.");
        return;
    };
    let mut host = host;
    // Drop the probe timeout before the sockets become a byte pump.
    let _ = host.set_read_timeout(None);
    if write_reply(&mut host, &JoinReply::Ready).is_err() {
        reject(stream, "The waiting host disconnected.");
        return;
    }
    if write_reply(&mut stream, &JoinReply::Ready).is_err() {
        let _ = host.shutdown(Shutdown::Both);
        return;
    }

    let host_reader = match host.try_clone() {
        Ok(stream) => stream,
        Err(_) => {
            let _ = host.shutdown(Shutdown::Both);
            return;
        }
    };
    let client_writer = match stream.try_clone() {
        Ok(stream) => stream,
        Err(_) => {
            let _ = host.shutdown(Shutdown::Both);
            return;
        }
    };

    let forward = thread::spawn(move || pump(host_reader, client_writer));
    pump(stream, host);
    let _ = forward.join();
}

fn handle_connection(stream: TcpStream, rooms: Arc<Rooms>) {
    // Accepted sockets can inherit nonblocking mode from the listener on macOS.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    let mut stream = stream;
    let request = match read_join(&mut stream) {
        Ok(request) => request,
        Err(message) => {
            reject(stream, message);
            return;
        }
    };
    let _ = stream.set_read_timeout(None);
    match request {
        JoinRequest::Host { room } => match normalize_room(&room) {
            Ok(room) => handle_host(stream, rooms, room),
            Err(message) => reject(stream, message),
        },
        JoinRequest::Client { room } => match normalize_room(&room) {
            Ok(room) => handle_client(stream, rooms, room),
            Err(message) => reject(stream, message),
        },
    }
}

/// Runs the relay until `stop` is set. Each TCP connection either registers a
/// host room, joins one as a client, or is rejected. Once a host and client are
/// paired, the relay copies bytes without inspecting them, so the Noise session
/// above it stays end to end encrypted.
pub fn serve(listener: TcpListener, stop: Arc<AtomicBool>) {
    let _ = listener.set_nonblocking(true);
    let rooms = Arc::new(Rooms::default());
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _peer)) => {
                let rooms = Arc::clone(&rooms);
                let _ = thread::Builder::new()
                    .name("nexus-relay-session".to_string())
                    .spawn(move || handle_connection(stream, rooms));
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => {
                eprintln!("relay: listener stopped: {error}");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::{
        io::{BufRead, BufReader},
        net::SocketAddr,
    };

    fn start_relay() -> (SocketAddr, Arc<AtomicBool>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("relay listener");
        let address = listener.local_addr().expect("relay address");
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || serve(listener, thread_stop));
        (address, stop, handle)
    }

    fn send_join(stream: &mut TcpStream, payload: Value) {
        let mut line = serde_json::to_vec(&payload).expect("join json");
        line.push(b'\n');
        stream.write_all(&line).expect("send join");
    }

    fn read_reply(stream: &mut TcpStream) -> Value {
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("read timeout");
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read reply");
        serde_json::from_str(&line).expect("reply json")
    }

    #[test]
    fn validates_room_tokens() {
        assert_eq!(
            normalize_room("AABBCCDDEEFF00112233445566778899").unwrap(),
            "aabbccddeeff00112233445566778899"
        );
        assert!(normalize_room("aabbccdd").is_err());
        assert!(normalize_room("aabbccddeeff0011223344556677889z").is_err());
    }

    #[test]
    fn relays_bytes_between_a_paired_host_and_client() {
        let (address, stop, handle) = start_relay();
        let mut host = TcpStream::connect(address).expect("host connect");
        send_join(
            &mut host,
            json!({ "type": "host", "room": "aabbccddeeff00112233445566778899" }),
        );

        let mut host_reply = BufReader::new(host.try_clone().expect("host clone"));
        let mut line = String::new();
        host_reply.read_line(&mut line).expect("host waiting");
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["type"], "waiting");

        let mut client = TcpStream::connect(address).expect("client connect");
        send_join(
            &mut client,
            json!({ "type": "client", "room": "aabbccddeeff00112233445566778899" }),
        );
        let client_reply = read_reply(&mut client);
        assert_eq!(client_reply["type"], "ready");

        host.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut host_line = String::new();
        host_reply.read_line(&mut host_line).expect("host ready");
        assert_eq!(serde_json::from_str::<Value>(&host_line).unwrap()["type"], "ready");

        client.write_all(b"client-to-host").expect("client write");
        let mut buffer = [0u8; 14];
        host.read_exact(&mut buffer).expect("host read");
        assert_eq!(&buffer, b"client-to-host");

        host.write_all(b"host-to-client").expect("host write");
        let mut buffer = [0u8; 14];
        client.read_exact(&mut buffer).expect("client read");
        assert_eq!(&buffer, b"host-to-client");

        drop(client);
        stop.store(true, Ordering::SeqCst);
        handle.join().expect("relay thread");
    }

    #[test]
    fn re_registers_a_room_after_the_host_disconnects() {
        let (address, stop, handle) = start_relay();
        let room = "00112233445566778899aabbccddeeff";

        let mut host = TcpStream::connect(address).expect("host connect");
        send_join(&mut host, json!({ "type": "host", "room": room }));
        assert_eq!(read_reply(&mut host)["type"], "waiting");
        drop(host);

        thread::sleep(Duration::from_millis(900));

        let mut reconnect = TcpStream::connect(address).expect("reconnect");
        send_join(&mut reconnect, json!({ "type": "host", "room": room }));
        assert_eq!(read_reply(&mut reconnect)["type"], "waiting");

        stop.store(true, Ordering::SeqCst);
        handle.join().expect("relay thread");
    }

    #[test]
    fn rejects_unknown_rooms_duplicate_hosts_and_bad_tokens() {
        let (address, stop, handle) = start_relay();
        let room = "aabbccddeeff00112233445566778899";

        let mut host = TcpStream::connect(address).expect("host connect");
        send_join(&mut host, json!({ "type": "host", "room": room }));
        let mut reader = BufReader::new(host.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["type"], "waiting");

        let mut duplicate = TcpStream::connect(address).expect("duplicate connect");
        send_join(&mut duplicate, json!({ "type": "host", "room": room }));
        assert_eq!(read_reply(&mut duplicate)["type"], "error");

        let mut unknown = TcpStream::connect(address).expect("unknown connect");
        send_join(
            &mut unknown,
            json!({ "type": "client", "room": "00112233445566778899aabbccddeeff" }),
        );
        assert_eq!(read_reply(&mut unknown)["type"], "error");

        let mut invalid = TcpStream::connect(address).expect("invalid connect");
        send_join(&mut invalid, json!({ "type": "host", "room": "not-a-room" }));
        assert_eq!(read_reply(&mut invalid)["type"], "error");

        stop.store(true, Ordering::SeqCst);
        handle.join().expect("relay thread");
    }
}
