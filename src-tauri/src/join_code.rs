//! A single string a friend can paste to start a direct session.
//!
//! Telling someone "install Tailscale, join this tailnet, then type my IP, my
//! port, and this eight-character pairing code" is exactly the friction this
//! build is trying to remove. A join code compresses the three things a client
//! really needs — the host's IPv4 address, its media port, and the single-use
//! pairing code — into one short token that fits in any chat window:
//!
//! ```text
//! PC1-<base64 of 12 bytes>
//! ```
//!
//! The payload is deliberately fixed-width: one version byte, the four IPv4
//! octets, the port big-endian, and the eight pairing-code characters packed
//! five bits each (the alphabet omits I, O, 0 and 1 so a code can be read
//! aloud). Twelve bytes encode to sixteen base64 characters, so the whole
//! token stays at twenty characters and never needs line wrapping.
//!
//! The prefix carries the version, which means a future format can be rejected
//! by this build with a clear message instead of decoding into nonsense. The
//! pairing code inside is still verified by the existing Noise handshake, so a
//! shared join code leaks nothing beyond what the user read out themselves.

use std::net::Ipv4Addr;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;

/// Marks the format inside a shared string, including its version.
const PREFIX: &str = "PC1-";
const VERSION: u8 = 1;
/// version + IPv4 + port + five packed pairing-code bytes.
const PAYLOAD_LEN: usize = 12;
const CODE_LEN: usize = 8;
/// Eight five-bit values fit in five bytes.
const CODE_BYTES: usize = 5;

/// Pairing-code alphabet: uppercase letters without I/O, plus digits 2-9.
/// Thirty-two symbols is what makes five bits per character exact.
const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// The contents of a join code, restored to plain values.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinCode {
    pub address: String,
    pub port: u16,
    pub code: String,
}

/// Packs a host address, media port and pairing code into one shareable token.
///
/// The pairing code may be typed with or without its dash and in any case; the
/// token always carries the canonical `XXXX-XXXX` form.
pub fn encode(address: &str, port: u16, code: &str) -> Result<String, String> {
    let trimmed = address.trim();
    let ip: Ipv4Addr = trimmed.parse().map_err(|_| {
        format!(
            "\"{trimmed}\" is not an IPv4 address. A join code needs an address such as \
             192.168.195.252."
        )
    })?;
    validate_shareable(ip)?;
    if port == 0 {
        return Err(
            "Port 0 cannot be shared; start hosting first so the app knows its media port."
                .to_string(),
        );
    }

    let normalized = normalize_pairing_code(code)?;
    let mut payload = [0u8; PAYLOAD_LEN];
    payload[0] = VERSION;
    payload[1..5].copy_from_slice(&ip.octets());
    payload[5..7].copy_from_slice(&port.to_be_bytes());
    payload[7..12].copy_from_slice(&pack_pairing_code(&normalized)?);

    Ok(format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(payload)))
}

/// Restores a join code, rejecting anything this build cannot use.
pub fn decode(value: &str) -> Result<JoinCode, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("The join code is empty.".to_string());
    }
    let Some(prefix) = trimmed.get(..PREFIX.len()) else {
        return Err(format!(
            "This is not a Nexus join code: it should start with {PREFIX}."
        ));
    };
    if !prefix.eq_ignore_ascii_case(PREFIX) {
        return Err(format!(
            "This is not a Nexus join code: it should start with {PREFIX}."
        ));
    }

    let payload_text = &trimmed[PREFIX.len()..];
    if payload_text.is_empty() {
        return Err(format!("The join code has no data after {PREFIX}."));
    }
    let payload = URL_SAFE_NO_PAD
        .decode(payload_text)
        .map_err(|error| format!("The join code is not valid base64: {error}."))?;
    if payload.len() != PAYLOAD_LEN {
        return Err(format!(
            "The join code carries {} bytes of data; a {PREFIX} code needs {PAYLOAD_LEN}.",
            payload.len()
        ));
    }
    if payload[0] != VERSION {
        return Err(format!(
            "This join code uses version {}; this build understands version {VERSION}.",
            payload[0]
        ));
    }

    let ip = Ipv4Addr::new(payload[1], payload[2], payload[3], payload[4]);
    validate_shareable(ip)?;
    let port = u16::from_be_bytes([payload[5], payload[6]]);
    if port == 0 {
        return Err("The join code carries port 0, which cannot be used.".to_string());
    }

    Ok(JoinCode {
        address: ip.to_string(),
        port,
        code: unpack_pairing_code(&payload[7..12])?,
    })
}

/// Rejects addresses that can never reach another machine.
fn validate_shareable(ip: Ipv4Addr) -> Result<(), String> {
    if ip.is_unspecified() {
        return Err(
            "0.0.0.0 is not reachable by a friend; share the Tailscale or LAN address instead."
                .to_string(),
        );
    }
    if ip.is_loopback() {
        return Err(
            "127.x is loopback and only works on this Mac; share the Tailscale or LAN address \
             instead."
                .to_string(),
        );
    }
    if ip.is_broadcast() {
        return Err("255.255.255.255 is broadcast and cannot be a host address.".to_string());
    }
    Ok(())
}

/// Uppercases a typed pairing code and removes its separators.
fn normalize_pairing_code(code: &str) -> Result<String, String> {
    let normalized: String = code
        .chars()
        .filter(|character| !character.is_whitespace() && *character != '-')
        .collect::<String>()
        .to_uppercase();
    if normalized.chars().count() != CODE_LEN {
        return Err(format!(
            "A pairing code has {CODE_LEN} characters; \"{}\" has {}.",
            code.trim(),
            normalized.chars().count()
        ));
    }
    if let Some(bad) = normalized
        .chars()
        .find(|character| !character.is_ascii() || code_value(*character as u8).is_none())
    {
        return Err(format!(
            "\"{bad}\" is not allowed in a pairing code (the alphabet omits I, O, 0 and 1)."
        ));
    }
    Ok(normalized)
}

/// Packs eight five-bit indices into five bytes, first character most
/// significant.
fn pack_pairing_code(code: &str) -> Result<[u8; CODE_BYTES], String> {
    let mut packed: u64 = 0;
    for byte in code.bytes() {
        let value = code_value(byte)
            .ok_or_else(|| "The pairing code contains an unsupported character.".to_string())?;
        packed = (packed << 5) | u64::from(value);
    }
    Ok([
        (packed >> 32) as u8,
        (packed >> 24) as u8,
        (packed >> 16) as u8,
        (packed >> 8) as u8,
        packed as u8,
    ])
}

/// Unpacks five bytes back into the canonical `XXXX-XXXX` pairing code.
fn unpack_pairing_code(bytes: &[u8]) -> Result<String, String> {
    let mut packed: u64 = 0;
    for byte in bytes {
        packed = (packed << 8) | u64::from(*byte);
    }
    let mut characters = String::with_capacity(CODE_LEN);
    for index in 0..CODE_LEN {
        let shift = 5 * (CODE_LEN - 1 - index);
        let value = ((packed >> shift) & 0x1f) as usize;
        let Some(&character) = ALPHABET.get(value) else {
            return Err("The join code's pairing-code bits are out of range.".to_string());
        };
        characters.push(character as char);
    }
    Ok(format_pairing_code(&characters))
}

fn code_value(byte: u8) -> Option<u8> {
    ALPHABET
        .iter()
        .position(|candidate| *candidate == byte)
        .map(|index| index as u8)
}

/// Insert the display dash: `ABCDEFGH` becomes `ABCD-EFGH`.
fn format_pairing_code(code: &str) -> String {
    let (head, tail) = code.split_at(4);
    format!("{head}-{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDRESS: &str = "192.168.195.252";
    const PORT: u16 = 39_200;
    const PAIRING_CODE: &str = "ABCD-EFGH";

    fn packed_code(code: &str) -> [u8; CODE_BYTES] {
        pack_pairing_code(&normalize_pairing_code(code).expect("normalize test code"))
            .expect("pack test code")
    }

    /// Builds a token around arbitrary payload bytes, for the rejection tests.
    fn crafted(version: u8, ip: [u8; 4], port: u16, code: [u8; CODE_BYTES]) -> String {
        let mut payload = [0u8; PAYLOAD_LEN];
        payload[0] = version;
        payload[1..5].copy_from_slice(&ip);
        payload[5..7].copy_from_slice(&port.to_be_bytes());
        payload[7..12].copy_from_slice(&code);
        format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(payload))
    }

    #[test]
    fn round_trips_a_join_code() {
        let encoded = encode(ADDRESS, PORT, PAIRING_CODE).expect("encode");
        assert!(encoded.starts_with("PC1-"), "{encoded}");
        assert_eq!(
            encoded.len(),
            20,
            "join codes stay one short line: {encoded}"
        );

        let decoded = decode(&encoded).expect("decode");
        assert_eq!(
            decoded,
            JoinCode {
                address: ADDRESS.to_string(),
                port: PORT,
                code: PAIRING_CODE.to_string(),
            }
        );
    }

    #[test]
    fn payload_layout_matches_the_contract() {
        let encoded = encode(ADDRESS, PORT, "ABCDEFGH").expect("encode");
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded.trim_start_matches(PREFIX))
            .expect("payload base64");
        assert_eq!(bytes.len(), PAYLOAD_LEN);
        assert_eq!(bytes[0], VERSION);
        assert_eq!(&bytes[1..5], &[192, 168, 195, 252]);
        assert_eq!(
            u16::from_be_bytes([bytes[5], bytes[6]]),
            PORT,
            "port is big-endian"
        );
        assert_eq!(&bytes[7..12], &packed_code("ABCDEFGH"));
    }

    #[test]
    fn normalizes_the_pairing_code_and_accepts_lowercase_prefix() {
        let spaced = encode(ADDRESS, PORT, " ab cd-ef gh ").expect("encode spaced");
        let compact = encode(ADDRESS, PORT, "abcdefgh").expect("encode lowercase");
        assert_eq!(
            spaced, compact,
            "the token is canonical regardless of typing"
        );
        assert_eq!(decode(&spaced).expect("decode").code, PAIRING_CODE);

        // The prefix is case-insensitive; the base64 body is not.
        let lowercase_prefix = format!("pc1-{}", spaced.trim_start_matches(PREFIX));
        let decoded = decode(&format!("  {lowercase_prefix}\n")).expect("decode lowercase prefix");
        assert_eq!(decoded.code, PAIRING_CODE);
    }

    #[test]
    fn every_alphabet_char_round_trips() {
        for code in ["ABCDEFGH", "JKMNPQRS", "TUVWXY23", "456789AB"] {
            let encoded = encode(ADDRESS, PORT, code).expect("encode alphabet run");
            assert_eq!(
                decode(&encoded).expect("decode alphabet run").code,
                format_pairing_code(code)
            );
        }
    }

    #[test]
    fn rejects_unshareable_addresses() {
        for address in [
            "0.0.0.0",
            "127.0.0.1",
            "127.1.2.3",
            "255.255.255.255",
            "not-an-ip",
            "",
        ] {
            let error = encode(address, PORT, PAIRING_CODE).expect_err("address must be rejected");
            assert!(
                !error.is_empty(),
                "a rejected address needs a message: {address}"
            );
        }
        let error = encode("0.0.0.0", PORT, PAIRING_CODE).expect_err("unspecified");
        assert!(error.contains("0.0.0.0"), "{error}");
        let error = encode("127.0.0.1", PORT, PAIRING_CODE).expect_err("loopback");
        assert!(error.contains("loopback"), "{error}");
    }

    #[test]
    fn rejects_port_zero() {
        let error = encode(ADDRESS, 0, PAIRING_CODE).expect_err("port zero");
        assert!(error.contains("Port 0"), "{error}");
    }

    #[test]
    fn rejects_bad_pairing_codes() {
        for code in [
            "ABCD-EFG",
            "ABCD-EFGHI",
            "ABCD-EFGI",
            "ABCD-EFGO",
            "ABCD-EFG0",
            "ABCD-EFG1",
            "ABCD EFGH I",
            "",
        ] {
            assert!(
                encode(ADDRESS, PORT, code).is_err(),
                "expected pairing code {code:?} to be rejected"
            );
        }
        let error = encode(ADDRESS, PORT, "ABCD-EFGI").expect_err("I is not in the alphabet");
        assert!(
            error.contains("I"),
            "the message should name the bad character: {error}"
        );
    }

    #[test]
    fn decode_rejects_malformed_tokens() {
        let valid = encode(ADDRESS, PORT, PAIRING_CODE).expect("encode");
        let cases = [
            String::new(),
            "   ".to_string(),
            "hello".to_string(),
            "PC1".to_string(),
            "PC1-".to_string(),
            "PC2-AAAA".to_string(),
            format!("PC1-{}", "****"),
            format!("PC1-{}", "éééé"),
            format!("PC1-{}", URL_SAFE_NO_PAD.encode([0u8; 11])),
            format!("PC1-{}", URL_SAFE_NO_PAD.encode([0u8; 13])),
            valid.repeat(2),
        ];
        for value in cases {
            assert!(decode(&value).is_err(), "expected {value:?} to be rejected");
        }
        let error = decode("PC1-****").expect_err("bad base64");
        assert!(error.contains("base64"), "{error}");
        let error = decode(&format!("PC1-{}", URL_SAFE_NO_PAD.encode([0u8; 11])))
            .expect_err("wrong length");
        assert!(error.contains("bytes"), "{error}");
    }

    #[test]
    fn decode_rejects_bad_version_ip_and_port() {
        let code = packed_code("ABCDEFGH");
        let version = crafted(2, [192, 168, 195, 252], PORT, code);
        let error = decode(&version).expect_err("unsupported version");
        assert!(error.contains("version"), "{error}");

        let loopback = crafted(VERSION, [127, 0, 0, 1], PORT, code);
        let error = decode(&loopback).expect_err("loopback address");
        assert!(error.contains("loopback"), "{error}");

        let zero_port = crafted(VERSION, [192, 168, 195, 252], 0, code);
        let error = decode(&zero_port).expect_err("port zero");
        assert!(error.contains("port 0"), "{error}");

        let unspecified = crafted(VERSION, [0, 0, 0, 0], PORT, code);
        assert!(decode(&unspecified).is_err(), "0.0.0.0 must be rejected");
    }

    #[test]
    fn decode_reports_non_join_code_input_clearly() {
        let error = decode("https://example.com").expect_err("not a code");
        assert!(error.contains("PC1-"), "{error}");
        let error = decode("PC1-").expect_err("empty payload");
        assert!(error.contains("no data"), "{error}");
    }
}
