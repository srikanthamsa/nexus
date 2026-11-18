//! OpenJoystickDriver (OJD) endpoint client.
//!
//! OJD is a separately signed macOS app that holds the restricted virtual-HID
//! entitlement. Rather than creating HID devices ourselves, Nexus connects
//! to its local AF_UNIX endpoint and publishes one virtual pad per viewer.
//! This is the native-controller path that works without SIP/AMFI changes.
//!
//! The endpoint protocol is versioned (`openjoystickdriver/v1alpha2`) and
//! authenticated with a token proof: HMAC-SHA256 keyed by SHA-256(token) over a
//! challenge nonce. The token is never transmitted or logged.

use std::{
    env,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::{Duration, Instant},
};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::gamepad::GamepadState;

const API_VERSION: &str = "openjoystickdriver/v1alpha2";
const TOKEN_NAME_FALLBACK: &str = "sunshine";
const KEEPALIVE_AFTER: Duration = Duration::from_millis(750);

/// Everything needed to open an OJD feed.
#[derive(Debug, Clone)]
pub struct OjdConfig {
    pub socket_path: String,
    pub token: String,
    pub token_name: String,
}

/// Discover the OJD endpoint and a control token.
///
/// Priority:
/// 1. `OJD_SOCKET_PATH` / `OJD_TOKEN` / `OJD_TOKEN_NAME` environment variables
///    (useful for tests and advanced setups).
/// 2. The endpoint socket under `$TMPDIR`.
/// 3. The `ojd_socket_path` / `ojd_token` keys in Sunshine's config, because the
///    Sunshine fork already creates a token for this endpoint.
///
/// Returning `None` simply leaves the native HID backend in charge.
pub fn discover() -> Option<OjdConfig> {
    let sunshine = read_sunshine_config();
    let socket_path = env::var("OJD_SOCKET_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            let candidate = env::temp_dir().join("com.openjoystickdriver.endpoint.sock");
            candidate
                .exists()
                .then(|| candidate.to_string_lossy().into_owned())
        })
        .or_else(|| sunshine.as_ref().and_then(|config| config.socket.clone()))?;
    let token = env::var("OJD_TOKEN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| sunshine.as_ref().and_then(|config| config.token.clone()))?;
    let token_name = env::var("OJD_TOKEN_NAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| TOKEN_NAME_FALLBACK.to_string());
    Some(OjdConfig {
        socket_path,
        token,
        token_name,
    })
}

#[derive(Default)]
struct SunshineOjd {
    socket: Option<String>,
    token: Option<String>,
}

/// Read only the OJD keys from Sunshine's config; every other line is ignored.
fn read_sunshine_config() -> Option<SunshineOjd> {
    let home = env::var_os("HOME")?;
    let path = PathBuf::from(home).join(".config/sunshine/sunshine.conf");
    let text = std::fs::read_to_string(path).ok()?;
    let mut config = SunshineOjd::default();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || !line.contains('=') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "ojd_socket_path" => config.socket = Some(value.to_string()),
            "ojd_token" => config.token = Some(value.to_string()),
            _ => {}
        }
    }
    if config.socket.is_none() || config.token.is_none() {
        return None;
    }
    Some(config)
}

/// One OJD virtual gamepad feed.
pub struct OjdPad {
    _reader: BufReader<UnixStream>,
    writer: UnixStream,
    last_sent: Instant,
}

impl OjdPad {
    /// Open a feed and create the virtual pad for `slot` (0..=3; OJD serves four).
    pub fn connect(config: &OjdConfig, slot: u8) -> Result<Self, String> {
        if slot == 0 || slot > 4 {
            return Err(
                "OpenJoystickDriver supports four virtual gamepads; this viewer cannot get one."
                    .to_string(),
            );
        }
        let stream = UnixStream::connect(&config.socket_path).map_err(|error| {
            format!(
                "Could not connect to the OpenJoystickDriver endpoint at {}: {error}",
                config.socket_path
            )
        })?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|error| format!("Could not configure the OJD endpoint timeout: {error}"))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(|error| {
                format!("Could not configure the OJD endpoint write timeout: {error}")
            })?;
        let writer = stream
            .try_clone()
            .map_err(|error| format!("Could not clone the OJD endpoint socket: {error}"))?;
        let mut reader = BufReader::new(stream);

        let challenge = read_reply(&mut reader, "Challenge")?;
        let nonce = challenge
            .get("nonce")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "OpenJoystickDriver did not send a challenge nonce.".to_string())?;

        let hello = json!({
            "apiVersion": API_VERSION,
            "kind": "Hello",
            "scopes": ["Control"],
            "tokenName": config.token_name,
            "proof": token_proof(&config.token, nonce),
        });
        write_json(&writer, &hello)?;

        let welcome = read_reply(&mut reader, "Welcome")?;
        if let Some(code) = welcome.get("code").and_then(Value::as_str) {
            return Err(ojd_error(
                code,
                welcome.get("message").and_then(Value::as_str),
            ));
        }
        let has_control = welcome
            .get("scopes")
            .and_then(Value::as_array)
            .is_some_and(|scopes| scopes.iter().any(|scope| scope == "Control"));
        if !has_control {
            return Err("OpenJoystickDriver did not grant the Control scope.".to_string());
        }

        write_json(
            &writer,
            &json!({
                "apiVersion": API_VERSION,
                "kind": "FeedRequest",
                "as": "SDLCompatible",
            }),
        )?;
        let session = read_reply(&mut reader, "FeedSession")?;
        if let Some(code) = session.get("code").and_then(Value::as_str) {
            return Err(ojd_error(
                code,
                session.get("message").and_then(Value::as_str),
            ));
        }

        Ok(Self {
            _reader: reader,
            writer,
            last_sent: Instant::now(),
        })
    }

    /// Publish an absolute frame. OJD treats omitted controls as neutral, so the
    /// frame always lists the current axes; empty button/d-pad arrays are omitted.
    pub fn send_state(&mut self, state: &GamepadState) -> Result<(), String> {
        write_json(&self.writer, &frame_message(state))?;
        self.last_sent = Instant::now();
        Ok(())
    }

    /// Keep the OJD feed alive while the viewer is not moving the pad. OJD
    /// disconnects a feed after two seconds without a line.
    pub fn keepalive_if_due(&mut self) -> Result<(), String> {
        if self.last_sent.elapsed() < KEEPALIVE_AFTER {
            return Ok(());
        }
        write_json(
            &self.writer,
            &json!({"apiVersion": API_VERSION, "kind": "KeepAlive"}),
        )?;
        self.last_sent = Instant::now();
        Ok(())
    }
}

/// Build the absolute frame OJD expects. D-pad directions belong in `dpad`,
/// not `buttons`; OJD silently ignores them there. Stick Y is inverted relative
/// to the W3C/Moonlight convention used by [`GamepadState`].
fn frame_message(state: &GamepadState) -> Value {
    let state = state.normalized();
    let mut buttons = Vec::new();
    for (index, name) in [
        (0usize, "South"),
        (1, "East"),
        (2, "West"),
        (3, "North"),
        (4, "LeftShoulder"),
        (5, "RightShoulder"),
        (8, "Back"),
        (9, "Start"),
        (10, "LeftStickClick"),
        (11, "RightStickClick"),
        (16, "Guide"),
    ] {
        if state.buttons.get(index).copied().unwrap_or(0.0) >= 0.5 {
            buttons.push(name);
        }
    }

    let mut dpad = Vec::new();
    for (index, name) in [
        (12usize, "DpadUp"),
        (13, "DpadDown"),
        (14, "DpadLeft"),
        (15, "DpadRight"),
    ] {
        if state.buttons.get(index).copied().unwrap_or(0.0) >= 0.5 {
            dpad.push(name);
        }
    }

    let trigger = |button: usize, axis: usize| -> f64 {
        state
            .buttons
            .get(button)
            .copied()
            .or_else(|| state.axes.get(axis).copied())
            .unwrap_or(0.0)
            .clamp(0.0, 1.0)
    };
    let axis = |index: usize| -> f64 {
        state
            .axes
            .get(index)
            .copied()
            .unwrap_or(0.0)
            .clamp(-1.0, 1.0)
    };

    let mut frame = json!({
        "apiVersion": API_VERSION,
        "kind": "Frame",
        "axes": {
            "LeftStickX": axis(0),
            // OJD's HID Y is positive for up; W3C Gamepad is positive down.
            "LeftStickY": -axis(1),
            "RightStickX": axis(2),
            "RightStickY": -axis(3),
            "LeftTrigger": trigger(6, 4),
            "RightTrigger": trigger(7, 5),
        },
    });
    if !buttons.is_empty() {
        frame["buttons"] = json!(buttons);
    }
    if !dpad.is_empty() {
        frame["dpad"] = json!(dpad);
    }
    frame
}

impl Drop for OjdPad {
    fn drop(&mut self) {
        // Drain is best-effort; OJD removes the pad as soon as the socket closes.
        let drain = json!({"apiVersion": API_VERSION, "kind": "Drain"});
        let _ = write_json(&self.writer, &drain);
    }
}

fn ojd_error(code: &str, message: Option<&str>) -> String {
    let reason = match code {
        "E1001" => "the OpenJoystickDriver endpoint is disabled",
        "E1002" => "the OpenJoystickDriver token is missing the Control scope",
        "E1009" | "E1010" | "E1011" => "this Nexus build uses an unsupported OJD API version",
        _ => "OpenJoystickDriver refused the virtual gamepad",
    };
    match message.filter(|value| !value.is_empty()) {
        Some(message) => format!("{reason} ({code}: {message})"),
        None => format!("{reason} ({code})"),
    }
}

fn read_reply(reader: &mut BufReader<UnixStream>, expected: &str) -> Result<Value, String> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|error| format!("OpenJoystickDriver did not reply with {expected}: {error}"))?;
    if line.trim().is_empty() {
        return Err(format!(
            "OpenJoystickDriver closed the endpoint before sending {expected}."
        ));
    }
    let value: Value = serde_json::from_str(line.trim())
        .map_err(|error| format!("OpenJoystickDriver sent an invalid {expected}: {error}"))?;
    if let Some(kind) = value.get("kind").and_then(Value::as_str) {
        if kind != expected && kind != "Status" {
            return Err(format!(
                "OpenJoystickDriver sent {kind} where {expected} was expected."
            ));
        }
    }
    Ok(value)
}

fn write_json(writer: &UnixStream, value: &Value) -> Result<(), String> {
    let mut line = serde_json::to_vec(value)
        .map_err(|error| format!("Could not encode an OpenJoystickDriver message: {error}"))?;
    line.push(b'\n');
    let mut writer = writer;
    writer
        .write_all(&line)
        .map_err(|error| format!("Could not write to the OpenJoystickDriver endpoint: {error}"))?;
    writer
        .flush()
        .map_err(|error| format!("Could not flush the OpenJoystickDriver endpoint: {error}"))
}

fn token_proof(token: &str, nonce: &str) -> String {
    let key = Sha256::digest(token.as_bytes());
    let message = format!("OpenJoystickDriver endpoint hello 1\n{nonce}\n\n\n");
    hex(&hmac_sha256(&key, message.as_bytes()))
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut normalized = [0u8; 64];
    if key.len() > 64 {
        normalized[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(normalized.map(|byte| byte ^ 0x36));
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(normalized.map(|byte| byte ^ 0x5c));
    outer.update(inner);
    outer.finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_proof_is_stable_and_secret() {
        let proof = token_proof("ojd_test_token", "bm9uY2U");
        assert_eq!(proof.len(), 64);
        assert!(proof.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!proof.contains("ojd_test_token"));
        assert_ne!(proof, token_proof("ojd_test_token", "bm9uY2Uy"));
        assert_ne!(proof, token_proof("ojd_other_token", "bm9uY2U"));
    }

    #[test]
    #[ignore = "requires a running OpenJoystickDriver endpoint with a Control token"]
    fn live_endpoint_accepts_four_pads() {
        let config = discover().expect("OJD endpoint and token should be discoverable");
        let neutral = GamepadState {
            slot: 0,
            name: "four-pad test".to_string(),
            connected: true,
            axes: vec![0.0; 6],
            buttons: vec![0.0; 17],
        };
        let mut pads = Vec::new();
        for slot in 1..=4 {
            let mut pad = OjdPad::connect(&config, slot).expect("OJD should create a pad");
            pad.send_state(&neutral).expect("frame should be accepted");
            pads.push(pad);
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(pads.len(), 4);
    }

    #[test]
    #[ignore = "requires a running OpenJoystickDriver endpoint with a Control token"]
    fn live_endpoint_accepts_a_pad() {
        let config = discover().expect("OJD endpoint and token should be discoverable");
        let mut pad = OjdPad::connect(&config, 3).expect("OJD should create a pad");
        let state = GamepadState {
            slot: 3,
            name: "live test".to_string(),
            connected: true,
            axes: vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            buttons: vec![0.0; 17],
        };
        pad.send_state(&state).expect("frame should be accepted");
        std::thread::sleep(std::time::Duration::from_millis(300));
        pad.keepalive_if_due()
            .expect("keepalive should be accepted");
    }

    fn state(buttons: &[usize], axes: &[f64; 4]) -> GamepadState {
        let mut values = vec![0.0; 17];
        for index in buttons {
            values[*index] = 1.0;
        }
        GamepadState {
            slot: 1,
            name: "test".to_string(),
            connected: true,
            axes: axes.to_vec(),
            buttons: values,
        }
    }

    #[test]
    fn dpad_uses_its_own_field() {
        for (index, expected) in [
            (12usize, "DpadUp"),
            (13, "DpadDown"),
            (14, "DpadLeft"),
            (15, "DpadRight"),
        ] {
            let frame = frame_message(&state(&[index], &[0.0; 4]));
            assert_eq!(frame["dpad"], json!([expected]));
            assert!(frame.get("buttons").is_none(), "d-pad must not ride in buttons");
        }
    }

    #[test]
    fn pure_dpad_frame_has_no_buttons_key() {
        let frame = frame_message(&state(&[12], &[0.0; 4]));
        assert_eq!(frame["dpad"], json!(["DpadUp"]));
        assert!(frame.get("buttons").is_none());
    }

    #[test]
    fn stick_y_is_inverted_for_ojd() {
        // W3C Gamepad: -1 is up and +1 is down. OJD's HID Y is the reverse.
        let frame = frame_message(&state(&[], &[0.0, -1.0, 0.0, 1.0]));
        assert_eq!(frame["axes"]["LeftStickY"], 1.0);
        assert_eq!(frame["axes"]["RightStickY"], -1.0);
    }

    #[test]
    fn neutral_frame_omits_button_and_dpad_arrays() {
        let frame = frame_message(&state(&[], &[0.0; 4]));
        assert!(frame.get("buttons").is_none());
        assert!(frame.get("dpad").is_none());
        assert!(frame.get("axes").is_some());
    }

    #[test]
    fn hmac_matches_rfc_vector() {
        // RFC 4231 test case 2.
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }
}
