use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fmt,
    io::{ErrorKind, Read, Write},
    net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs},
    process::Command,
    time::{Duration, Instant},
};

pub const DEFAULT_RELAY_PORT: u16 = 39210;
pub const SERVICE_TYPE: &str = "_parsecplus._tcp.local.";

const RELAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const RELAY_JOIN_TIMEOUT: Duration = Duration::from_secs(8);
const RELAY_POLL: Duration = Duration::from_millis(250);
const MAX_REPLY_LINE: usize = 4_096;
const DEFAULT_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayTarget {
    pub host: String,
    pub port: u16,
}

impl RelayTarget {
    pub fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        let value = value.strip_prefix("relay://").unwrap_or(value);
        let value = value.trim_end_matches('/');
        if value.is_empty() {
            return Err("Enter the relay's address.".to_string());
        }
        if value.contains('#') {
            return Err("Relay addresses do not include the invite code.".to_string());
        }
        if let Ok(IpAddr::V6(_)) = value.parse::<IpAddr>() {
            return Ok(Self {
                host: value.to_string(),
                port: DEFAULT_RELAY_PORT,
            });
        }
        if let Some(rest) = value.strip_prefix('[') {
            let (host, tail) = rest
                .split_once(']')
                .ok_or_else(|| "Close the IPv6 relay address with ].".to_string())?;
            if host.is_empty() {
                return Err("Enter the relay's address.".to_string());
            }
            let port = match tail {
                "" => DEFAULT_RELAY_PORT,
                tail => match tail.strip_prefix(':') {
                    Some(port) => parse_port(port)?,
                    None => return Err("Put the relay port after the IPv6 address.".to_string()),
                },
            };
            return Ok(Self {
                host: host.to_string(),
                port,
            });
        }
        match value.rsplit_once(':') {
            Some((host, port)) => {
                if host.is_empty() {
                    return Err("Enter the relay's address.".to_string());
                }
                Ok(Self {
                    host: host.to_string(),
                    port: parse_port(port)?,
                })
            }
            None => Ok(Self {
                host: value.to_string(),
                port: DEFAULT_RELAY_PORT,
            }),
        }
    }

    pub fn socket_address(&self) -> Result<SocketAddr, String> {
        (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|error| format!("Could not resolve {}: {error}", self.host))?
            .next()
            .ok_or_else(|| format!("Could not resolve {}.", self.host))
    }
}

impl fmt::Display for RelayTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(formatter, "[{}]:{}", self.host, self.port)
        } else {
            write!(formatter, "{}:{}", self.host, self.port)
        }
    }
}

fn parse_port(value: &str) -> Result<u16, String> {
    let port = value
        .parse::<u16>()
        .map_err(|_| "Relay ports are numbers between 1 and 65535.".to_string())?;
    if port == 0 {
        return Err("Relay ports are numbers between 1 and 65535.".to_string());
    }
    Ok(port)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayInvite {
    pub target: RelayTarget,
    pub room: String,
}

impl RelayInvite {
    pub fn new(target: RelayTarget, room: String) -> Result<Self, String> {
        let room = normalize_room(&room)?;
        Ok(Self { target, room })
    }

    /// Invites look like `host:port#<32 hex characters>`. The room token only
    /// routes the two peers through the relay; the pairing code still gates the
    /// Noise handshake, so the relay never learns either secret.
    pub fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        if value.is_empty() {
            return Err("Paste the relay invite from the host.".to_string());
        }
        let (address, room) = value
            .rsplit_once('#')
            .ok_or_else(|| "Relay invites look like relay.example.com:39210#code.".to_string())?;
        let target = RelayTarget::parse(address)?;
        Self::new(target, room.to_string())
    }

    pub fn encode(&self) -> String {
        format!("{}#{}", self.target, self.room)
    }
}

pub fn normalize_room(value: &str) -> Result<String, String> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.len() != 32 || !normalized.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Relay invite codes are 32 hexadecimal characters.".to_string());
    }
    Ok(normalized)
}

pub fn generate_room() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    hex(&bytes)
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayRole {
    Host,
    Client,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum JoinRequest<'a> {
    Host { room: &'a str },
    Client { room: &'a str },
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
enum JoinReply {
    Waiting,
    Ready,
    Error { message: String },
}

fn read_reply_line(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut line = Vec::with_capacity(128);
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => return Err("The relay closed the connection.".to_string()),
            Ok(_) => {
                if byte[0] == b'\n' {
                    return Ok(line);
                }
                if byte[0] != b'\r' {
                    line.push(byte[0]);
                }
                if line.len() > MAX_REPLY_LINE {
                    return Err("The relay sent an oversized reply.".to_string());
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if is_timeout(&error) => {
                return Err("The relay did not answer in time.".to_string())
            }
            Err(error) => return Err(format!("The relay connection failed: {error}")),
        }
    }
}

fn is_timeout(error: &std::io::Error) -> bool {
    matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

/// Opens the rendezvous connection for one side of a relay session. Hosts
/// return once the relay has parked their room; clients return only after a
/// host has paired, at which point the socket becomes the encrypted byte pipe.
pub fn open_relay(target: &RelayTarget, role: RelayRole, room: &str) -> Result<TcpStream, String> {
    let room = normalize_room(room)?;
    let address = target.socket_address()?;
    let mut stream = TcpStream::connect_timeout(&address, RELAY_CONNECT_TIMEOUT)
        .map_err(|error| format!("Could not reach the relay at {target}: {error}"))?;
    let _ = stream.set_nodelay(true);

    let request = match role {
        RelayRole::Host => JoinRequest::Host { room: &room },
        RelayRole::Client => JoinRequest::Client { room: &room },
    };
    let mut payload = serde_json::to_vec(&request)
        .map_err(|error| format!("Could not prepare the relay request: {error}"))?;
    payload.push(b'\n');
    stream
        .write_all(&payload)
        .and_then(|_| stream.flush())
        .map_err(|error| format!("The relay closed the connection: {error}"))?;

    stream
        .set_read_timeout(Some(RELAY_JOIN_TIMEOUT))
        .map_err(|error| format!("Could not configure the relay connection: {error}"))?;
    let line = read_reply_line(&mut stream)?;
    let reply: JoinReply = serde_json::from_slice(&line)
        .map_err(|_| "The relay sent an unreadable reply.".to_string())?;

    match (role, reply) {
        (RelayRole::Host, JoinReply::Waiting) => Ok(stream),
        (RelayRole::Client, JoinReply::Ready) => Ok(stream),
        (_, JoinReply::Error { message }) => Err(message),
        _ => Err("The relay sent an unexpected reply.".to_string()),
    }
}

/// Waits for the relay to pair a client with a parked host room. Returns when
/// the channel is ready for the Noise handshake, or when `stopped` becomes true.
pub fn wait_for_relay_pairing(
    stream: &mut TcpStream,
    stopped: &dyn Fn() -> bool,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(RELAY_POLL))
        .map_err(|error| format!("Could not configure the relay connection: {error}"))?;
    let mut line = Vec::with_capacity(128);
    let mut byte = [0u8; 1];
    loop {
        if stopped() {
            return Err("The relay invite was closed.".to_string());
        }
        match stream.read(&mut byte) {
            Ok(0) => return Err("The relay closed the invite.".to_string()),
            Ok(_) => {
                if byte[0] == b'\n' {
                    if line.is_empty() {
                        continue;
                    }
                    let reply: JoinReply = serde_json::from_slice(&line)
                        .map_err(|_| "The relay sent an unreadable reply.".to_string())?;
                    line.clear();
                    match reply {
                        JoinReply::Ready => {
                            stream.set_read_timeout(None).map_err(|error| {
                                format!("Could not configure the relay connection: {error}")
                            })?;
                            return Ok(());
                        }
                        JoinReply::Error { message } => return Err(message),
                        JoinReply::Waiting => {}
                    }
                } else if byte[0] != b'\r' {
                    line.push(byte[0]);
                    if line.len() > MAX_REPLY_LINE {
                        return Err("The relay sent an oversized reply.".to_string());
                    }
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if is_timeout(&error) => {}
            Err(error) => return Err(format!("The relay connection ended: {error}")),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredHost {
    pub name: String,
    pub address: String,
    pub port: u16,
    pub platform: String,
    pub version: String,
}

/// Advertises this Mac over Bonjour while it hosts. The pairing code never
/// leaves the app; only the device name, platform, and version are published.
pub struct DiscoveryAdvertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

impl DiscoveryAdvertisement {
    pub fn start(port: u16) -> Result<Self, String> {
        let daemon =
            ServiceDaemon::new().map_err(|error| format!("Could not start Bonjour: {error}"))?;
        let device = crate::host::local_device_name();
        let id = local_device_id();
        let version = env!("CARGO_PKG_VERSION");
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &format!("parsecplus-{id}"),
            &local_hostname(),
            (),
            port,
            &[
                ("name", device.as_str()),
                ("platform", "macOS"),
                ("version", version),
                ("id", id.as_str()),
            ][..],
        )
        .map_err(|error| format!("Could not advertise this Mac: {error}"))?
        .enable_addr_auto();
        let fullname = info.get_fullname().to_string();
        daemon
            .register(info)
            .map_err(|error| format!("Could not advertise this Mac: {error}"))?;
        Ok(Self { daemon, fullname })
    }
}

impl Drop for DiscoveryAdvertisement {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

pub fn discover(timeout: Option<Duration>) -> Result<Vec<DiscoveredHost>, String> {
    let timeout = timeout
        .unwrap_or(DEFAULT_DISCOVERY_TIMEOUT)
        .clamp(Duration::from_millis(500), MAX_DISCOVERY_TIMEOUT);
    let daemon =
        ServiceDaemon::new().map_err(|error| format!("Could not start Bonjour: {error}"))?;
    let receiver = daemon
        .browse(SERVICE_TYPE)
        .map_err(|error| format!("Could not search the local network: {error}"))?;
    let deadline = Instant::now() + timeout;
    let mut seen = HashSet::new();
    let mut hosts = Vec::new();

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match receiver.recv_timeout(remaining.min(Duration::from_millis(250))) {
            Ok(ServiceEvent::ServiceResolved(resolved)) => {
                // Do not hide a host running on this same Mac: a local test
                // instance or a second copy is a legitimate nearby target.
                // Deduplicate only by the advertised full service name.
                if !seen.insert(resolved.get_fullname().to_string()) {
                    continue;
                }
                let Some(address) = pick_address(&resolved) else {
                    continue;
                };
                hosts.push(DiscoveredHost {
                    name: resolved
                        .get_property_val_str("name")
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or_else(|| resolved.get_hostname())
                        .to_string(),
                    address,
                    port: resolved.get_port(),
                    platform: resolved
                        .get_property_val_str("platform")
                        .unwrap_or("macOS")
                        .to_string(),
                    version: resolved
                        .get_property_val_str("version")
                        .unwrap_or("")
                        .to_string(),
                });
            }
            Ok(_) => {}
            Err(_) if receiver.is_disconnected() => break,
            Err(_) => {}
        }
    }

    let _ = daemon.stop_browse(SERVICE_TYPE);
    let _ = daemon.shutdown();
    hosts.sort_by_key(|host| host.name.to_lowercase());
    Ok(hosts)
}

fn pick_address(resolved: &mdns_sd::ResolvedService) -> Option<String> {
    let mut addresses: Vec<_> = resolved.get_addresses_v4().into_iter().collect();
    addresses.sort();
    if let Some(address) = addresses.first() {
        return Some(address.to_string());
    }
    let hostname = resolved.get_hostname().trim_end_matches('.');
    if hostname.is_empty() {
        None
    } else {
        Some(hostname.to_string())
    }
}

/// A stable, non-secret identifier used only to filter this Mac out of its own
/// discovery results. It is not a device credential.
pub fn local_device_id() -> String {
    let source = machine_hostname();
    let mut digest = Sha256::new();
    digest.update(b"parsecplus-device-v1\0");
    digest.update(source.as_bytes());
    hex(&digest.finalize()[..8])
}

fn machine_hostname() -> String {
    Command::new("hostname")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "parsecplus".to_string())
}

fn local_hostname() -> String {
    let raw = Command::new("hostname")
        .arg("-s")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("parsecplus-{}", &local_device_id()[..6]));
    let label: String = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(60)
        .collect();
    let label = label.trim_matches('-');
    if label.is_empty() {
        format!("parsecplus-{}.local.", &local_device_id()[..6])
    } else {
        format!("{label}.local.")
    }
}

#[tauri::command(async)]
pub fn native_discover_hosts(timeout_ms: Option<u64>) -> Result<Vec<DiscoveredHost>, String> {
    discover(timeout_ms.map(Duration::from_millis))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{
        self, handshake_initiator, handshake_responder, Control, SecureChannel,
    };
    use parsecplus_relay as relay;
    use std::{
        net::TcpListener,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        thread,
    };

    #[test]
    fn parses_relay_addresses() {
        assert_eq!(
            RelayTarget::parse(" relay.example.com ").unwrap(),
            RelayTarget {
                host: "relay.example.com".to_string(),
                port: DEFAULT_RELAY_PORT,
            }
        );
        assert_eq!(
            RelayTarget::parse("relay://relay.example.com:40000/").unwrap(),
            RelayTarget {
                host: "relay.example.com".to_string(),
                port: 40000,
            }
        );
        assert_eq!(
            RelayTarget::parse("[fd00::1]:39210").unwrap(),
            RelayTarget {
                host: "fd00::1".to_string(),
                port: 39210,
            }
        );
        assert_eq!(
            RelayTarget::parse("127.0.0.1").unwrap().port,
            DEFAULT_RELAY_PORT
        );
        assert!(RelayTarget::parse("").is_err());
        assert!(RelayTarget::parse("relay.example.com:0").is_err());
        assert!(RelayTarget::parse("relay.example.com:not-a-port").is_err());
        assert!(RelayTarget::parse("relay.example.com#room").is_err());
    }

    #[test]
    fn round_trips_relay_invites() {
        let invite =
            RelayInvite::parse("relay.example.com:40000#AABBCCDDEEFF00112233445566778899").unwrap();
        assert_eq!(invite.room, "aabbccddeeff00112233445566778899");
        assert_eq!(
            invite.encode(),
            "relay.example.com:40000#aabbccddeeff00112233445566778899"
        );
        assert!(RelayInvite::parse("relay.example.com").is_err());
        assert!(RelayInvite::parse("relay.example.com#nope").is_err());
    }

    #[test]
    fn generates_random_room_tokens() {
        let first = generate_room();
        let second = generate_room();
        assert_eq!(first.len(), 32);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn noise_session_round_trips_through_the_relay() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("relay listener");
        let address = listener.local_addr().expect("relay address");
        let relay_stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&relay_stop);
        let relay_thread = thread::spawn(move || relay::serve(listener, thread_stop));

        let target = RelayTarget {
            host: address.ip().to_string(),
            port: address.port(),
        };
        let room = generate_room();

        let host_target = target.clone();
        let host_room = room.clone();
        let (parked_tx, parked_rx) = std::sync::mpsc::channel();
        let host_thread = thread::spawn(move || {
            let mut stream =
                open_relay(&host_target, RelayRole::Host, &host_room).expect("host relay");
            let _ = parked_tx.send(());
            wait_for_relay_pairing(&mut stream, &|| false).expect("pairing");
            let noise = handshake_responder(&mut stream, "ABCD-2345").expect("responder");
            let mut channel = SecureChannel::new(stream, noise).expect("secure channel");
            let hello = channel
                .recv_timeout(Duration::from_secs(3))
                .expect("receive hello")
                .expect("hello message");
            let hello = transport::parse_control::<Control>(&hello)
                .expect("hello parsing")
                .expect("hello control");
            assert!(matches!(hello, Control::Hello { .. }));
            channel
                .send_control(&Control::Welcome {
                    width: 1920,
                    height: 1080,
                    allow_input: false,
                    host_name: "Relay test host".to_string(),
                    displays: Vec::new(),
                })
                .expect("welcome");
            let message = channel
                .recv_timeout(Duration::from_secs(3))
                .expect("receive bye")
                .expect("bye message");
            let control = transport::parse_control::<Control>(&message)
                .expect("bye parsing")
                .expect("bye control");
            assert!(matches!(control, Control::Bye));
        });

        parked_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("host parked its invite");
        let mut client = open_relay(&target, RelayRole::Client, &room).expect("client relay");
        let noise = handshake_initiator(&mut client, "abcd-2345").expect("initiator");
        let mut channel = SecureChannel::new(client, noise).expect("secure channel");
        channel
            .send_control(&Control::Hello {
                app_version: "test".to_string(),
                device_name: "Relay test client".to_string(),
                owner_name: String::new(),
            })
            .expect("hello");
        let welcome = channel
            .recv_timeout(Duration::from_secs(3))
            .expect("receive welcome")
            .expect("welcome message");
        let welcome = transport::parse_control::<Control>(&welcome)
            .expect("welcome parsing")
            .expect("welcome control");
        match welcome {
            Control::Welcome {
                width,
                height,
                host_name,
                ..
            } => {
                assert_eq!((width, height), (1920, 1080));
                assert_eq!(host_name, "Relay test host");
            }
            other => panic!("unexpected welcome: {other:?}"),
        }
        channel.send_control(&Control::Bye).expect("bye");
        channel.shutdown();

        host_thread.join().expect("host thread");
        relay_stop.store(true, Ordering::SeqCst);
        relay_thread.join().expect("relay thread");
    }

    #[test]
    #[ignore = "requires local network multicast"]
    fn advertises_this_mac_over_bonjour() {
        let advertisement = DiscoveryAdvertisement::start(39_200).expect("advertise");
        let daemon = ServiceDaemon::new().expect("browse daemon");
        let receiver = daemon.browse(SERVICE_TYPE).expect("browse");
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut found = false;
        while Instant::now() < deadline {
            if let Ok(ServiceEvent::ServiceResolved(resolved)) =
                receiver.recv_timeout(Duration::from_millis(250))
            {
                if resolved.get_fullname() == advertisement.fullname {
                    assert_eq!(resolved.get_port(), 39_200);
                    assert_eq!(resolved.get_property_val_str("platform"), Some("macOS"));
                    assert!(!resolved.get_addresses_v4().is_empty());
                    found = true;
                    break;
                }
            }
        }
        let _ = daemon.stop_browse(SERVICE_TYPE);
        let _ = daemon.shutdown();
        drop(advertisement);
        assert!(found, "the advertised service was not resolved");
    }

    #[test]
    fn relay_rejects_an_unknown_invite() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("relay listener");
        let address = listener.local_addr().expect("relay address");
        let relay_stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&relay_stop);
        let relay_thread = thread::spawn(move || relay::serve(listener, thread_stop));

        let target = RelayTarget {
            host: address.ip().to_string(),
            port: address.port(),
        };
        let error =
            open_relay(&target, RelayRole::Client, &generate_room()).expect_err("unknown invite");
        assert!(error.contains("No host"));

        relay_stop.store(true, Ordering::SeqCst);
        relay_thread.join().expect("relay thread");
    }
}
