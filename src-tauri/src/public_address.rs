//! Finding the address a peer on another network should connect to.
//!
//! A host that wants a friend outside its LAN to join has to tell them which
//! public IP and port to aim at, but the machine only knows its local address.
//! The obvious answer is a "what is my IP" HTTP service, which leaks the
//! request to a third party, needs a TLS client, and answers the wrong
//! question: TCP egress and UDP ingress can differ, and what the media path
//! needs is the address:port a *UDP* peer would actually reach.
//!
//! A STUN binding request (RFC 5389) answers exactly that. It is a single
//! UDP datagram sent from the same socket the media path uses, and the reply
//! names the source address:port that server saw. Running it from the fixed
//! media port therefore doubles as a port-forward check: if the mapped port
//! is not the one the user configured on the router, the forward is wrong.
//!
//! The module stays deliberately small and std-only. A binding request is a
//! twenty-byte header and the reply is a header plus length-prefixed
//! attributes, so a full STUN dependency would buy little. It never mutates
//! machine state: one ephemeral (or caller-chosen) UDP socket, one request,
//! one reply, and a clear sentence for the UI when any step fails.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// RFC 5389 magic cookie, also the key for XOR-MAPPED-ADDRESS ports.
const MAGIC_COOKIE: u32 = 0x2112_A442;
/// Binding request, no attributes.
const BINDING_REQUEST: u16 = 0x0001;
/// Binding success response.
const BINDING_SUCCESS: u16 = 0x0101;
/// MAPPED-ADDRESS (RFC 3489 style, no obfuscation).
const ATTR_MAPPED_ADDRESS: u16 = 0x0001;
/// XOR-MAPPED-ADDRESS (RFC 5389 section 15.2).
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
const HEADER_LEN: usize = 20;
const TRANSACTION_ID_LEN: usize = 12;
/// Address family byte in a mapped-address attribute.
const FAMILY_IPV4: u8 = 0x01;
const FAMILY_IPV6: u8 = 0x02;
/// IANA-assigned STUN port.
const DEFAULT_STUN_PORT: u16 = 3478;
/// Retransmissions per probe. A binding request is idempotent, so three sends
/// with the same transaction id are safe and absorb one lost datagram.
const MAX_SENDS: usize = 3;
/// Stray datagrams tolerated on one attempt before moving to the next send.
const MAX_PACKETS_PER_ATTEMPT: usize = 8;
const RECEIVE_BUFFER: usize = 2_048;

/// A successful binding probe.
///
/// `address` is what the STUN server saw and what a remote peer must dial.
/// `local_address` is the socket the probe used; on the media path its port is
/// the one to forward on the router.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicAddress {
    /// The mapped (public) address:port reported by the STUN server.
    pub address: SocketAddr,
    /// The locally bound socket that sent the request (`0.0.0.0:port`).
    pub local_address: SocketAddr,
}

/// Asks `stun_url` which public address:port it sees for a UDP socket.
///
/// `local_port` is the local UDP port to probe from: 0 picks an ephemeral
/// port, and the media port makes the reply a router port-forward check.
/// `timeout` bounds the whole exchange, including the retransmissions.
pub fn discover(
    stun_url: &str,
    local_port: u16,
    timeout: Duration,
) -> Result<PublicAddress, String> {
    let (host, port) = parse_stun_url(stun_url)?;

    let mut servers: Vec<SocketAddr> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|error| format!("Could not resolve the STUN server {host}: {error}"))?
        .collect();
    if servers.is_empty() {
        return Err(format!(
            "Could not resolve the STUN server {host} to an IP address."
        ));
    }
    // IPv4 first: the media path prefers it and the probe socket is IPv4.
    servers.sort_by_key(|server| server.is_ipv6());
    if !servers.iter().any(SocketAddr::is_ipv4) {
        return Err(format!(
            "The STUN server {host} only resolved to IPv6 addresses; the probe needs an IPv4 server."
        ));
    }

    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, local_port)).map_err(|error| {
        format!(
            "Could not open local UDP port {local_port} for the address probe: {error}. \
             That port may already be in use by an active hosting session."
        )
    })?;
    let local_address = socket
        .local_addr()
        .map_err(|error| format!("Could not read the probe socket's local address: {error}"))?;

    let deadline = Instant::now() + timeout;
    let transaction_id = transaction_id();
    let request = binding_request(&transaction_id);
    let mut buffer = [0u8; RECEIVE_BUFFER];
    let mut sends = 0usize;
    // Spread the retransmit budget over the resolved addresses, IPv4 first.
    let per_server = MAX_SENDS.div_ceil(servers.len()).max(1);
    let mut last_error = format!(
        "The STUN server did not answer within {} ms.",
        timeout.as_millis()
    );

    for server in &servers {
        let mut attempts = 0usize;
        while attempts < per_server && sends < MAX_SENDS {
            let now = Instant::now();
            if now >= deadline {
                return Err(last_error);
            }
            let remaining = deadline.saturating_duration_since(now);
            let sends_left = (MAX_SENDS - sends) as u32;
            // Give each remaining send its share of the time left, so one
            // silent server cannot consume the whole budget.
            let slice = (remaining / sends_left)
                .max(Duration::from_millis(1))
                .min(remaining);
            if let Err(error) = socket.set_read_timeout(Some(slice)) {
                return Err(format!(
                    "Could not configure the address probe's UDP timeout: {error}"
                ));
            }

            attempts += 1;
            sends += 1;
            if let Err(error) = socket.send_to(&request, server) {
                last_error = format!("Could not send the STUN request to {server}: {error}");
                continue;
            }

            let attempt_deadline = Instant::now() + slice;
            let mut received = 0usize;
            loop {
                let now = Instant::now();
                if now >= deadline {
                    return Err(last_error);
                }
                if now >= attempt_deadline || received >= MAX_PACKETS_PER_ATTEMPT {
                    break;
                }
                match socket.recv_from(&mut buffer) {
                    Ok((length, _from)) => {
                        received += 1;
                        match parse_binding_response(&buffer[..length], &transaction_id) {
                            Ok(Some(address)) => {
                                return Ok(PublicAddress {
                                    address,
                                    local_address,
                                });
                            }
                            // Not a binding response for this request (a stray
                            // datagram, or another sender on the same port).
                            Ok(None) => {}
                            // It *was* our reply but unusable: retransmitting
                            // is the only remaining move.
                            Err(error) => {
                                last_error = error;
                                break;
                            }
                        }
                    }
                    Err(error) => match error.kind() {
                        ErrorKind::WouldBlock | ErrorKind::TimedOut => break,
                        ErrorKind::ConnectionRefused | ErrorKind::ConnectionReset => {
                            last_error = format!(
                                "The STUN server {server} refused the UDP probe ({error})."
                            );
                            break;
                        }
                        _ => {
                            last_error = format!("Could not read the STUN response: {error}");
                            break;
                        }
                    },
                }
            }
        }
    }

    Err(last_error)
}

/// Splits a STUN URL into a host and a port.
///
/// Accepts `stun:host`, `stun:host:port`, `stun://host:port` and bracketed
/// IPv6 (`stun:[2001:db8::1]:3478`), with optional surrounding whitespace.
/// Only `stun:` is accepted: TURN (`turn:`/`turns:`) is a different protocol
/// this build does not configure yet.
pub fn parse_stun_url(stun_url: &str) -> Result<(String, u16), String> {
    let trimmed = stun_url.trim();
    if trimmed.is_empty() {
        return Err(
            "A STUN server URL is required (for example stun:stun.l.google.com:19302).".to_string(),
        );
    }
    let (scheme, rest) = trimmed
        .split_once(':')
        .ok_or_else(|| format!("The STUN server URL \"{trimmed}\" is missing its stun: scheme."))?;
    if !scheme.eq_ignore_ascii_case("stun") {
        return Err(format!(
            "Unsupported server URL scheme \"{scheme}:\". Only stun: is supported; TURN servers are not configured yet."
        ));
    }
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    if rest.is_empty() {
        return Err("The STUN server URL has no host after stun:.".to_string());
    }
    if rest.contains('/') {
        return Err(
            "The STUN server URL must be just a host and an optional port (no path).".to_string(),
        );
    }
    if rest.contains('?') || rest.contains('#') {
        return Err(
            "The STUN server URL must be just a host and an optional port (no query or fragment)."
                .to_string(),
        );
    }

    let (host, port_text) = if let Some(bracketed) = rest.strip_prefix('[') {
        let (host, after) = bracketed.split_once(']').ok_or_else(|| {
            "The STUN server URL has an unterminated IPv6 address; use stun:[addr]:port."
                .to_string()
        })?;
        if after.is_empty() {
            (host.to_string(), None)
        } else {
            let port = after.strip_prefix(':').ok_or_else(|| {
                "The STUN server URL has text after the bracketed IPv6 address.".to_string()
            })?;
            (host.to_string(), Some(port.to_string()))
        }
    } else if let Some((host, port)) = rest.rsplit_once(':') {
        if host.contains(':') {
            return Err(format!(
                "IPv6 STUN addresses must be wrapped in brackets, for example stun:[::1]:{DEFAULT_STUN_PORT}."
            ));
        }
        (host.to_string(), Some(port.to_string()))
    } else {
        (rest.to_string(), None)
    };

    if host.is_empty() {
        return Err("The STUN server URL has an empty host.".to_string());
    }
    if host.chars().any(char::is_whitespace) {
        return Err("The STUN server host must not contain spaces.".to_string());
    }

    let port = match port_text {
        Some(text) => text.parse::<u16>().map_err(|_| {
            format!("The STUN server port \"{text}\" is not a number between 1 and 65535.")
        })?,
        None => DEFAULT_STUN_PORT,
    };
    if port == 0 {
        return Err(
            "The STUN server port 0 is not usable; choose a port between 1 and 65535.".to_string(),
        );
    }

    Ok((host, port))
}

/// Builds the twenty-byte binding request that `discover` retransmits.
fn binding_request(transaction_id: &[u8; TRANSACTION_ID_LEN]) -> [u8; HEADER_LEN] {
    let mut request = [0u8; HEADER_LEN];
    request[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    // Message length 0: the request carries no attributes.
    request[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    request[8..20].copy_from_slice(transaction_id);
    request
}

/// Twelve bytes a STUN server cannot predict from the request itself.
///
/// `RandomState` is seeded from the OS, so hashing a clock reading and a
/// counter with fresh keys per chunk gives a distinct transaction id without
/// pulling `rand` into this std-only module.
fn transaction_id() -> [u8; TRANSACTION_ID_LEN] {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);

    let mut id = [0u8; TRANSACTION_ID_LEN];
    for (index, chunk) in id.chunks_mut(8).enumerate() {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(nanos);
        hasher.write_u64(counter);
        hasher.write_u8(index as u8);
        let value = hasher.finish().to_le_bytes();
        chunk.copy_from_slice(&value[..chunk.len()]);
    }
    id
}

/// Validates a datagram as this request's binding success and returns the
/// mapped address, if it carries one.
///
/// `Ok(None)` means "not ours" and is safe to ignore (a stray datagram or
/// another sender). `Err` means the header matched this request but the body
/// cannot be used, so the caller should report or retransmit.
fn parse_binding_response(
    packet: &[u8],
    transaction_id: &[u8; TRANSACTION_ID_LEN],
) -> Result<Option<SocketAddr>, String> {
    if packet.len() < HEADER_LEN {
        return Ok(None);
    }
    let message_type = u16::from_be_bytes([packet[0], packet[1]]);
    if message_type != BINDING_SUCCESS {
        return Ok(None);
    }
    let cookie = u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]);
    if cookie != MAGIC_COOKIE {
        return Ok(None);
    }
    if packet[8..20] != transaction_id[..] {
        return Ok(None);
    }

    let length = u16::from_be_bytes([packet[2], packet[3]]) as usize;
    if !length.is_multiple_of(4) {
        return Err(malformed(
            "the attribute area is not a whole number of four-byte words",
        ));
    }
    let end = HEADER_LEN
        .checked_add(length)
        .ok_or_else(|| malformed("the message length overflows"))?;
    if end > packet.len() {
        return Err(malformed("it declares more attributes than it sent"));
    }

    let mut xor_mapped = None;
    let mut mapped = None;
    let mut offset = HEADER_LEN;
    while offset + 4 <= end {
        let attribute_type = u16::from_be_bytes([packet[offset], packet[offset + 1]]);
        let attribute_length =
            u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]) as usize;
        let value_start = offset + 4;
        let value_end = value_start + attribute_length;
        if value_end > end {
            // Truncated attribute: stop scanning rather than trusting the rest
            // of the datagram. Anything decoded already still counts.
            break;
        }
        let value = &packet[value_start..value_end];
        match attribute_type {
            ATTR_XOR_MAPPED_ADDRESS if xor_mapped.is_none() => {
                xor_mapped = decode_mapped_address(value, true, transaction_id);
            }
            ATTR_MAPPED_ADDRESS if mapped.is_none() => {
                mapped = decode_mapped_address(value, false, transaction_id);
            }
            _ => {}
        }
        // Attribute values are padded to a four-byte boundary.
        offset = value_start + ((attribute_length + 3) & !3);
    }

    match xor_mapped.or(mapped) {
        Some(address) => Ok(Some(address)),
        None => Err("The STUN server replied but reported no mapped address.".to_string()),
    }
}

/// Decodes a MAPPED-ADDRESS / XOR-MAPPED-ADDRESS attribute value.
///
/// Returns `None` (skip the attribute) for an unknown family or a value that is
/// too short to hold the address it claims.
fn decode_mapped_address(
    value: &[u8],
    xor: bool,
    transaction_id: &[u8; TRANSACTION_ID_LEN],
) -> Option<SocketAddr> {
    if value.len() < 4 {
        return None;
    }
    let family = value[1];
    let mut port = u16::from_be_bytes([value[2], value[3]]);
    if xor {
        port ^= (MAGIC_COOKIE >> 16) as u16;
    }

    let address = match family {
        FAMILY_IPV4 => {
            if value.len() < 8 {
                return None;
            }
            let mut bytes = [0u8; 4];
            bytes.copy_from_slice(&value[4..8]);
            if xor {
                for (byte, mask) in bytes.iter_mut().zip(MAGIC_COOKIE.to_be_bytes()) {
                    *byte ^= mask;
                }
            }
            IpAddr::V4(Ipv4Addr::from(bytes))
        }
        FAMILY_IPV6 => {
            if value.len() < 20 {
                return None;
            }
            let mut bytes = [0u8; 16];
            bytes.copy_from_slice(&value[4..20]);
            if xor {
                let mut mask = [0u8; 16];
                mask[..4].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
                mask[4..].copy_from_slice(transaction_id);
                for (byte, mask) in bytes.iter_mut().zip(mask) {
                    *byte ^= mask;
                }
            }
            IpAddr::V6(Ipv6Addr::from(bytes))
        }
        _ => return None,
    };

    Some(SocketAddr::new(address, port))
}

fn malformed(detail: &str) -> String {
    format!("The STUN server sent a malformed response: {detail}.")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRANSACTION_ID: [u8; TRANSACTION_ID_LEN] = [
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
    ];

    fn packet_with_body(transaction_id: &[u8; TRANSACTION_ID_LEN], body: &[u8]) -> Vec<u8> {
        let mut packet = Vec::with_capacity(HEADER_LEN + body.len());
        packet.extend_from_slice(&BINDING_SUCCESS.to_be_bytes());
        packet.extend_from_slice(&(body.len() as u16).to_be_bytes());
        packet.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        packet.extend_from_slice(transaction_id);
        packet.extend_from_slice(body);
        packet
    }

    fn encode_attributes(entries: &[(u16, Vec<u8>)]) -> Vec<u8> {
        let mut body = Vec::new();
        for (attribute_type, value) in entries {
            body.extend_from_slice(&attribute_type.to_be_bytes());
            body.extend_from_slice(&(value.len() as u16).to_be_bytes());
            body.extend_from_slice(value);
            while body.len() % 4 != 0 {
                body.push(0);
            }
        }
        body
    }

    fn response(entries: &[(u16, Vec<u8>)]) -> Vec<u8> {
        packet_with_body(&TRANSACTION_ID, &encode_attributes(entries))
    }

    fn xor_value(port: u16, address: IpAddr) -> Vec<u8> {
        let mut value = vec![
            0u8,
            match address {
                IpAddr::V4(_) => FAMILY_IPV4,
                IpAddr::V6(_) => FAMILY_IPV6,
            },
        ];
        value.extend_from_slice(&(port ^ (MAGIC_COOKIE >> 16) as u16).to_be_bytes());
        match address {
            IpAddr::V4(ip) => {
                let mut bytes = ip.octets();
                for (byte, mask) in bytes.iter_mut().zip(MAGIC_COOKIE.to_be_bytes()) {
                    *byte ^= mask;
                }
                value.extend_from_slice(&bytes);
            }
            IpAddr::V6(ip) => {
                let mut mask = [0u8; 16];
                mask[..4].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
                mask[4..].copy_from_slice(&TRANSACTION_ID);
                let mut bytes = ip.octets();
                for (byte, mask) in bytes.iter_mut().zip(mask) {
                    *byte ^= mask;
                }
                value.extend_from_slice(&bytes);
            }
        }
        value
    }

    fn plain_value(port: u16, address: IpAddr) -> Vec<u8> {
        let mut value = vec![
            0u8,
            match address {
                IpAddr::V4(_) => FAMILY_IPV4,
                IpAddr::V6(_) => FAMILY_IPV6,
            },
        ];
        value.extend_from_slice(&port.to_be_bytes());
        match address {
            IpAddr::V4(ip) => value.extend_from_slice(&ip.octets()),
            IpAddr::V6(ip) => value.extend_from_slice(&ip.octets()),
        }
        value
    }

    #[test]
    fn accepts_the_documented_url_forms() {
        assert_eq!(
            parse_stun_url("stun:stun.l.google.com").expect("plain host"),
            ("stun.l.google.com".to_string(), 3478)
        );
        assert_eq!(
            parse_stun_url(" stun:stun.l.google.com:19302 ").expect("whitespace"),
            ("stun.l.google.com".to_string(), 19302)
        );
        assert_eq!(
            parse_stun_url("stun://stun.l.google.com:19302").expect("slashes"),
            ("stun.l.google.com".to_string(), 19302)
        );
        assert_eq!(
            parse_stun_url("STUN:example.com:3478").expect("uppercase scheme"),
            ("example.com".to_string(), 3478)
        );
        assert_eq!(
            parse_stun_url("stun://example.com").expect("slashes, default port"),
            ("example.com".to_string(), 3478)
        );
        assert_eq!(
            parse_stun_url("stun:[2001:db8::1]:3478").expect("bracketed IPv6"),
            ("2001:db8::1".to_string(), 3478)
        );
        assert_eq!(
            parse_stun_url("stun:[2001:db8::1]").expect("bracketed IPv6, default port"),
            ("2001:db8::1".to_string(), 3478)
        );
    }

    #[test]
    fn rejects_unsupported_or_incomplete_urls() {
        let rejected = [
            "",
            "   ",
            "stun:",
            "stun://",
            "example.com",
            "stun::3478",
            "turn:example.com:3478",
            "turns:example.com:3478",
            "http://example.com",
            "stun:example.com:notaport",
            "stun:example.com:70000",
            "stun:example.com:0",
            "stun:example.com:3478/path",
            "stun:example.com:3478?x=1",
            "stun:example.com:3478#frag",
            "stun:2001:db8::1",
            "stun:[2001:db8::1",
            "stun:exa mple.com",
        ];
        for url in rejected {
            assert!(
                parse_stun_url(url).is_err(),
                "expected {url:?} to be rejected"
            );
        }
        // The TURN rejection should point at the scheme, not the port.
        let error = parse_stun_url("turns:example.com:3478").expect_err("turns must fail");
        assert!(error.contains("Only stun"), "{error}");
    }

    #[test]
    fn binding_request_is_a_binding_request() {
        let transaction_id = TRANSACTION_ID;
        let request = binding_request(&transaction_id);
        assert_eq!(request.len(), HEADER_LEN);
        assert_eq!(
            u16::from_be_bytes([request[0], request[1]]),
            BINDING_REQUEST
        );
        assert_eq!(u16::from_be_bytes([request[2], request[3]]), 0);
        assert_eq!(
            u32::from_be_bytes([request[4], request[5], request[6], request[7]]),
            MAGIC_COOKIE
        );
        assert_eq!(&request[8..20], &transaction_id[..]);
    }

    #[test]
    fn transaction_ids_differ() {
        assert_ne!(transaction_id(), transaction_id());
    }

    #[test]
    fn decodes_an_xor_mapped_ipv4_address() {
        let expected: SocketAddr = "203.0.113.7:12345".parse().expect("expected address");
        let packet = response(&[(
            ATTR_XOR_MAPPED_ADDRESS,
            xor_value(expected.port(), expected.ip()),
        )]);
        assert_eq!(
            parse_binding_response(&packet, &TRANSACTION_ID).expect("valid response"),
            Some(expected)
        );
    }

    #[test]
    fn decodes_an_xor_mapped_ipv6_address() {
        let expected: SocketAddr = "[2001:db8::1]:3478".parse().expect("expected address");
        let packet = response(&[(
            ATTR_XOR_MAPPED_ADDRESS,
            xor_value(expected.port(), expected.ip()),
        )]);
        assert_eq!(
            parse_binding_response(&packet, &TRANSACTION_ID).expect("valid response"),
            Some(expected)
        );
    }

    #[test]
    fn falls_back_to_mapped_address() {
        let expected: SocketAddr = "198.51.100.9:41300".parse().expect("expected address");
        let packet = response(&[(
            ATTR_MAPPED_ADDRESS,
            plain_value(expected.port(), expected.ip()),
        )]);
        assert_eq!(
            parse_binding_response(&packet, &TRANSACTION_ID).expect("valid response"),
            Some(expected)
        );
    }

    #[test]
    fn prefers_xor_mapped_over_mapped_and_skips_unknown_attributes() {
        let mapped: SocketAddr = "198.51.100.9:41300".parse().expect("mapped");
        let xor_mapped: SocketAddr = "203.0.113.7:12345".parse().expect("xor mapped");
        let packet = response(&[
            (0x8022, b"parsecplus-test".to_vec()), // SOFTWARE, ignored
            (ATTR_MAPPED_ADDRESS, plain_value(mapped.port(), mapped.ip())),
            (
                ATTR_XOR_MAPPED_ADDRESS,
                xor_value(xor_mapped.port(), xor_mapped.ip()),
            ),
        ]);
        assert_eq!(
            parse_binding_response(&packet, &TRANSACTION_ID).expect("valid response"),
            Some(xor_mapped)
        );
    }

    #[test]
    fn rejects_datagrams_that_are_not_our_binding_success() {
        let mapped: SocketAddr = "203.0.113.7:12345".parse().expect("mapped");
        let valid = response(&[(
            ATTR_XOR_MAPPED_ADDRESS,
            xor_value(mapped.port(), mapped.ip()),
        )]);

        // Too short to be a STUN message.
        assert_eq!(
            parse_binding_response(&valid[..10], &TRANSACTION_ID).expect("short"),
            None
        );
        // A successful response to a different transaction.
        let mut other = TRANSACTION_ID;
        other[0] ^= 0xff;
        assert_eq!(
            parse_binding_response(&valid, &other).expect("other transaction"),
            None
        );
        // Not a binding success (e.g. a request or an error response).
        let mut wrong_type = valid.clone();
        wrong_type[1] = 0x11;
        assert_eq!(
            parse_binding_response(&wrong_type, &TRANSACTION_ID).expect("wrong type"),
            None
        );
        // Wrong magic cookie.
        let mut wrong_cookie = valid.clone();
        wrong_cookie[4] ^= 0xff;
        assert_eq!(
            parse_binding_response(&wrong_cookie, &TRANSACTION_ID).expect("wrong cookie"),
            None
        );
    }

    #[test]
    fn rejects_a_length_that_disagrees_with_the_datagram() {
        let mapped: SocketAddr = "203.0.113.7:12345".parse().expect("mapped");
        let mut packet = response(&[(
            ATTR_XOR_MAPPED_ADDRESS,
            xor_value(mapped.port(), mapped.ip()),
        )]);
        let inflated = u16::from_be_bytes([packet[2], packet[3]]) + 4;
        packet[2..4].copy_from_slice(&inflated.to_be_bytes());
        let error = parse_binding_response(&packet, &TRANSACTION_ID).expect_err("inflated length");
        assert!(error.contains("malformed"), "{error}");

        let mut unaligned = response(&[(
            ATTR_XOR_MAPPED_ADDRESS,
            xor_value(mapped.port(), mapped.ip()),
        )]);
        unaligned[2..4].copy_from_slice(&3u16.to_be_bytes());
        let error = parse_binding_response(&unaligned, &TRANSACTION_ID).expect_err("unaligned");
        assert!(error.contains("malformed"), "{error}");
    }

    #[test]
    fn survives_truncated_or_short_attributes() {
        // An attribute that claims more value bytes than the datagram holds.
        let truncated = packet_with_body(
            &TRANSACTION_ID,
            &[
                0x00, 0x20, 0x00, 0x10, // XOR-MAPPED-ADDRESS, length 16
                0x00, 0x01, 0x00, 0x00, // only four value bytes follow
            ],
        );
        let error = parse_binding_response(&truncated, &TRANSACTION_ID).expect_err("truncated");
        assert!(error.contains("no mapped address"), "{error}");

        // A structurally valid but too-short XOR-MAPPED-ADDRESS value.
        let short = response(&[(ATTR_XOR_MAPPED_ADDRESS, vec![0x00, FAMILY_IPV4, 0x00, 0x00])]);
        let error = parse_binding_response(&short, &TRANSACTION_ID).expect_err("short value");
        assert!(error.contains("no mapped address"), "{error}");

        // A reply with no mapped-address attribute at all.
        let empty = response(&[(0x8022, b"stun".to_vec())]);
        let error = parse_binding_response(&empty, &TRANSACTION_ID).expect_err("no attribute");
        assert!(error.contains("no mapped address"), "{error}");
    }

    #[test]
    fn discover_rejects_bad_urls_before_touching_the_network() {
        let error = discover("turn:example.com:3478", 0, Duration::from_millis(10))
            .expect_err("TURN is not supported");
        assert!(error.contains("Only stun"), "{error}");
    }

    #[test]
    fn discover_reports_a_busy_local_port_clearly() {
        // Hold an ephemeral port so the probe's bind must fail, then confirm the
        // message tells the user hosting may already own the port.
        let holder = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).expect("bind a probe port");
        let port = holder.local_addr().expect("holder address").port();
        let error = discover("stun:127.0.0.1", port, Duration::from_millis(20))
            .expect_err("a busy port must fail");
        assert!(error.contains("in use"), "{error}");
    }

    #[test]
    fn public_address_reports_both_sockets() {
        let value = PublicAddress {
            address: "203.0.113.7:12345".parse().expect("mapped"),
            local_address: "0.0.0.0:41000".parse().expect("local"),
        };
        let expected: SocketAddr = "203.0.113.7:12345".parse().expect("mapped");
        assert_eq!(value.address, expected);
        assert!(value.local_address.ip().is_unspecified());
        assert_eq!(value.local_address.port(), 41000);
    }
}
