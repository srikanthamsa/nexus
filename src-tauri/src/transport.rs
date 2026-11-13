use crate::gamepad::GamepadState;
use crate::input::RemoteInput;
use crate::rtp_video::RtpVideoPacket;
use rand_core::{OsRng, RngCore};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use snow::{params::NoiseParams, Builder, TransportState};
use std::{
    io::{self, ErrorKind, Read, Write},
    net::TcpStream,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

pub const DEFAULT_PORT: u16 = 39200;

const PROLOGUE: &[u8] = b"parsecplus-direct-v1";
const MAX_NOISE_MESSAGE: usize = 65_535;
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MESSAGE_CONTROL: u8 = 1;
pub(crate) const MESSAGE_FRAME: u8 = 2;
const FRAME_KEYFRAME: u8 = 1;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// SCTP data-channel messages are limited to 16 KiB, and every chunk carries a
/// 34-byte header, so chunks stay below the limit on both transports.
pub const FRAME_CHUNK_BYTES: usize = 12_288;

const MAX_PENDING_FRAMES: usize = 8;
const MAX_PENDING_BYTES: usize = 32 * 1024 * 1024;

fn noise_params() -> &'static NoiseParams {
    static PARAMS: OnceLock<NoiseParams> = OnceLock::new();
    PARAMS.get_or_init(|| {
        "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s"
            .parse()
            .expect("valid Noise parameters")
    })
}

/// Uppercases a pairing code and removes separators. Pairing codes use an
/// alphabet that omits ambiguous characters.
pub fn normalize_pair_code(code: &str) -> Result<String, String> {
    let normalized: String = code
        .chars()
        .filter(|character| !character.is_whitespace() && *character != '-')
        .flat_map(|character| character.to_uppercase())
        .collect();
    if normalized.len() != 8 {
        return Err("Pairing codes have eight characters.".to_string());
    }
    if !normalized
        .bytes()
        .all(|byte| b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789".contains(&byte))
    {
        return Err("That pairing code contains unsupported characters.".to_string());
    }
    Ok(normalized)
}

/// Generates a single-use pairing code from the OS CSPRNG. The alphabet has 32
/// characters, so the byte-to-character mapping is unbiased.
pub fn generate_pair_code() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0u8; 8];
    OsRng.fill_bytes(&mut bytes);
    let mut code = String::with_capacity(9);
    for (index, byte) in bytes.iter().enumerate() {
        if index == 4 {
            code.push('-');
        }
        code.push(ALPHABET[(*byte as usize) % ALPHABET.len()] as char);
    }
    code
}

/// One display the host can stream, advertised to a client in `Welcome`.
///
/// The index is ScreenCaptureKit's shareable-display order, the same value a
/// `RequestDisplay` carries back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayOption {
    pub index: usize,
    pub label: String,
    pub width: u32,
    pub height: u32,
}

/// Control messages travel inside the encrypted channel alongside video chunks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Control {
    Hello {
        app_version: String,
        #[serde(default)]
        device_name: String,
        #[serde(default)]
        owner_name: String,
    },
    Welcome {
        width: u32,
        height: u32,
        allow_input: bool,
        host_name: String,
        /// Displays the host can switch the live capture to. Empty when the
        /// peer predates display switching.
        #[serde(default)]
        displays: Vec<DisplayOption>,
    },
    Input {
        event: RemoteInput,
    },
    /// A controller snapshot from the client. Kept separate from `Input` so
    /// the host can gate it on the per-viewer gamepad switch and route it to
    /// the virtual HID backend.
    Gamepad {
        state: GamepadState,
    },
    InputState {
        enabled: bool,
    },
    /// A plain-text clipboard update, only forwarded when the host enabled
    /// clipboard sharing for the session.
    Clipboard {
        text: String,
    },
    RequestKeyframe,
    /// Client → host: switch the live capture to another shareable display.
    RequestDisplay {
        index: usize,
    },
    /// Host → client: the switch succeeded and the stream now reports these
    /// dimensions.
    DisplayChanged {
        index: usize,
        width: u32,
        height: u32,
    },
    StreamFeedback {
        received: u32,
        jitter_ms: f64,
        rtt_ms: f64,
        dropped: u32,
    },
    StreamRate {
        bitrate_mbps: f64,
        ceiling_mbps: f64,
        adaptive: bool,
    },
    Ping {
        sent_ms: u64,
    },
    Pong {
        sent_ms: u64,
    },
    Error {
        message: String,
    },
    Bye,
    /// Signaling messages travel over the encrypted TCP channel and carry a
    /// serialized `RTCSessionDescription`.
    TransportOffer {
        sdp: String,
    },
    TransportAnswer {
        sdp: String,
    },
    TransportFallback {
        reason: String,
    },
    TransportChoice {
        webrtc: bool,
    },
}

/// The pairing code is mixed into the Noise XX handshake as a third-message
/// PSK, so a peer without the code cannot finish the handshake or read frames.
pub fn pairing_psk(code: &str) -> Result<[u8; 32], String> {
    let normalized = normalize_pair_code(code)?;
    let mut digest = Sha256::new();
    digest.update(b"parsecplus-pairing-v1\0");
    digest.update(normalized.as_bytes());
    Ok(digest.finalize().into())
}

fn noise_error(error: snow::Error) -> io::Error {
    io::Error::new(
        ErrorKind::InvalidData,
        format!("Encrypted channel error: {error}"),
    )
}

fn read_handshake_message(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut length = [0u8; 2];
    stream.read_exact(&mut length)?;
    let size = u16::from_be_bytes(length) as usize;
    if size == 0 || size > MAX_NOISE_MESSAGE {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "Invalid handshake message size.",
        ));
    }
    let mut message = vec![0u8; size];
    stream.read_exact(&mut message)?;
    Ok(message)
}

fn write_handshake_message(stream: &mut TcpStream, message: &[u8]) -> io::Result<()> {
    if message.is_empty() || message.len() > MAX_NOISE_MESSAGE {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "Invalid handshake message size.",
        ));
    }
    stream.write_all(&(message.len() as u16).to_be_bytes())?;
    stream.write_all(message)?;
    stream.flush()
}

fn build_handshake(code: &str, initiator: bool) -> io::Result<snow::HandshakeState> {
    let psk = pairing_psk(code).map_err(|error| io::Error::new(ErrorKind::InvalidInput, error))?;
    let base = Builder::new(noise_params().clone()).prologue(PROLOGUE);
    let keypair = base.generate_keypair().map_err(noise_error)?;
    let builder = base.local_private_key(&keypair.private).psk(3, &psk);
    if initiator {
        builder.build_initiator().map_err(noise_error)
    } else {
        builder.build_responder().map_err(noise_error)
    }
}

fn prepare_handshake_stream(stream: &TcpStream) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))
}

/// Runs the initiator half of the authenticated Noise handshake.
pub fn handshake_initiator(stream: &mut TcpStream, code: &str) -> io::Result<TransportState> {
    prepare_handshake_stream(stream)?;
    let mut noise = build_handshake(code, true)?;
    let mut buffer = vec![0u8; MAX_NOISE_MESSAGE];

    let length = noise.write_message(&[], &mut buffer).map_err(noise_error)?;
    write_handshake_message(stream, &buffer[..length])?;

    let message = read_handshake_message(stream)?;
    noise
        .read_message(&message, &mut buffer)
        .map_err(noise_error)?;

    let length = noise.write_message(&[], &mut buffer).map_err(noise_error)?;
    write_handshake_message(stream, &buffer[..length])?;

    noise.into_transport_mode().map_err(noise_error)
}

/// Runs the responder half of the authenticated Noise handshake.
pub fn handshake_responder(stream: &mut TcpStream, code: &str) -> io::Result<TransportState> {
    prepare_handshake_stream(stream)?;
    let mut noise = build_handshake(code, false)?;
    let mut buffer = vec![0u8; MAX_NOISE_MESSAGE];

    let message = read_handshake_message(stream)?;
    noise
        .read_message(&message, &mut buffer)
        .map_err(noise_error)?;

    let length = noise.write_message(&[], &mut buffer).map_err(noise_error)?;
    write_handshake_message(stream, &buffer[..length])?;

    let message = read_handshake_message(stream)?;
    noise
        .read_message(&message, &mut buffer)
        .map_err(noise_error)?;

    noise.into_transport_mode().map_err(noise_error)
}

/// An encrypted, length-framed TCP channel. Reads are non-blocking so a session
/// can interleave outgoing video with incoming input on one transport state.
pub struct SecureChannel {
    stream: TcpStream,
    noise: TransportState,
    incoming: Vec<u8>,
    expected: Option<usize>,
}

impl SecureChannel {
    pub fn new(stream: TcpStream, noise: TransportState) -> io::Result<Self> {
        stream.set_read_timeout(None)?;
        stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            noise,
            incoming: Vec::with_capacity(64 * 1024),
            expected: None,
        })
    }

    fn write_all_encrypted(&mut self, ciphertext: &[u8]) -> io::Result<()> {
        let deadline = Instant::now() + WRITE_TIMEOUT;
        let mut framed = Vec::with_capacity(ciphertext.len() + 2);
        framed.extend_from_slice(&(ciphertext.len() as u16).to_be_bytes());
        framed.extend_from_slice(ciphertext);

        let mut sent = 0;
        while sent < framed.len() {
            match self.stream.write(&framed[sent..]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        ErrorKind::WriteZero,
                        "The remote device closed the connection.",
                    ))
                }
                Ok(written) => sent += written,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            ErrorKind::TimedOut,
                            "Timed out while sending to the remote device.",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub fn send(&mut self, payload: &[u8]) -> io::Result<()> {
        if payload.len() > MAX_NOISE_MESSAGE - 16 {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "Transport message is too large.",
            ));
        }
        let mut ciphertext = vec![0u8; payload.len() + 16];
        let length = self
            .noise
            .write_message(payload, &mut ciphertext)
            .map_err(noise_error)?;
        ciphertext.truncate(length);
        self.write_all_encrypted(&ciphertext)
    }

    pub fn send_control<T: Serialize>(&mut self, message: &T) -> io::Result<()> {
        let json = serde_json::to_vec(message)
            .map_err(|error| io::Error::new(ErrorKind::InvalidInput, error.to_string()))?;
        let mut payload = Vec::with_capacity(json.len() + 1);
        payload.push(MESSAGE_CONTROL);
        payload.extend_from_slice(&json);
        self.send(&payload)
    }

    /// Returns one decrypted transport message if a complete one is available.
    pub fn poll_recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            if let Some(expected) = self.expected {
                if self.incoming.len() >= expected + 2 {
                    let ciphertext = self.incoming[2..expected + 2].to_vec();
                    self.incoming.drain(..expected + 2);
                    self.expected = None;
                    let mut plaintext = vec![0u8; ciphertext.len()];
                    let length = self
                        .noise
                        .read_message(&ciphertext, &mut plaintext)
                        .map_err(noise_error)?;
                    plaintext.truncate(length);
                    return Ok(Some(plaintext));
                }
            } else if self.incoming.len() >= 2 {
                let expected = u16::from_be_bytes([self.incoming[0], self.incoming[1]]) as usize;
                if expected == 0 || expected > MAX_NOISE_MESSAGE {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        "Invalid transport message size.",
                    ));
                }
                self.expected = Some(expected);
                continue;
            }

            let mut buffer = [0u8; 16 * 1024];
            match self.stream.read(&mut buffer) {
                Ok(0) => {
                    return Err(io::Error::new(
                        ErrorKind::ConnectionAborted,
                        "The remote device closed the connection.",
                    ))
                }
                Ok(read) => {
                    self.incoming.extend_from_slice(&buffer[..read]);
                    if self.incoming.len() > MAX_NOISE_MESSAGE + 2 {
                        return Err(io::Error::new(
                            ErrorKind::InvalidData,
                            "Transport buffer exceeded its limit.",
                        ));
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(None),
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }

    /// Waits up to `timeout` for one message, sleeping in short increments.
    pub fn recv_timeout(&mut self, timeout: Duration) -> io::Result<Option<Vec<u8>>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(message) = self.poll_recv()? {
                return Ok(Some(message));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    pub fn shutdown(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

/// The transport a session ended up using after negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportKind {
    Tcp,
    WebRtc,
}

/// A live session transport. TCP carries both signaling and, when WebRTC
/// cannot connect, the stream itself; WebRTC carries control over an SCTP
/// data channel and video over an RTP media track, both over UDP.
pub enum SessionChannel {
    Tcp(SecureChannel),
    WebRtc(crate::webrtc_transport::WebRtcChannel),
}

impl SessionChannel {
    /// Whether this session negotiated the RTP media path.
    pub fn is_webrtc(&self) -> bool {
        matches!(self, SessionChannel::WebRtc(_))
    }

    /// Sends one already-framed TCP message. The WebRTC path carries video as
    /// RTP and control as a data channel, so it has no use for this.
    pub fn send(&mut self, payload: &[u8]) -> io::Result<()> {
        match self {
            SessionChannel::Tcp(channel) => channel.send(payload),
            SessionChannel::WebRtc(_) => Err(io::Error::new(
                ErrorKind::Unsupported,
                "The WebRTC transport sends video as RTP, not as framed messages.",
            )),
        }
    }

    /// Packetizes one encoded Annex-B access unit onto the RTP track.
    pub fn send_video(&mut self, annex_b: &[u8], duration: Duration) -> io::Result<()> {
        match self {
            SessionChannel::WebRtc(channel) => channel.send_video(annex_b, duration),
            SessionChannel::Tcp(_) => Err(io::Error::new(
                ErrorKind::Unsupported,
                "The TCP transport frames video in the host's message loop.",
            )),
        }
    }

    /// One inbound RTP video packet; the TCP path reports none.
    pub fn poll_video(&mut self) -> io::Result<Option<RtpVideoPacket>> {
        match self {
            SessionChannel::WebRtc(channel) => channel.poll_video(),
            SessionChannel::Tcp(_) => Ok(None),
        }
    }

    /// The host's live bandwidth estimate on the RTP path.
    pub fn link_estimate(&self) -> Option<Arc<crate::congestion::LinkEstimate>> {
        match self {
            SessionChannel::WebRtc(channel) => channel.link_estimate(),
            SessionChannel::Tcp(_) => None,
        }
    }

    pub fn send_control<T: Serialize>(&mut self, message: &T) -> io::Result<()> {
        match self {
            SessionChannel::Tcp(channel) => channel.send_control(message),
            SessionChannel::WebRtc(channel) => channel.send_control(message),
        }
    }

    pub fn poll_recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        match self {
            SessionChannel::Tcp(channel) => channel.poll_recv(),
            SessionChannel::WebRtc(channel) => channel.poll_recv(),
        }
    }

    pub fn recv_timeout(&mut self, timeout: Duration) -> io::Result<Option<Vec<u8>>> {
        match self {
            SessionChannel::Tcp(channel) => channel.recv_timeout(timeout),
            SessionChannel::WebRtc(channel) => channel.recv_timeout(timeout),
        }
    }

    pub fn shutdown(&self) {
        match self {
            SessionChannel::Tcp(channel) => channel.shutdown(),
            SessionChannel::WebRtc(channel) => channel.shutdown(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FrameChunk<'a> {
    pub frame_id: u32,
    pub total_bytes: usize,
    pub offset: usize,
    pub keyframe: bool,
    pub timestamp_ms: u64,
    pub width: u32,
    pub height: u32,
    pub encode_ms: f32,
    pub data: &'a [u8],
}

pub struct OutgoingFrame<'a> {
    pub frame_id: u32,
    pub keyframe: bool,
    pub timestamp_ms: u64,
    pub width: u32,
    pub height: u32,
    pub encode_ms: f32,
    pub data: &'a [u8],
}

pub fn frame_messages(frame: &OutgoingFrame<'_>) -> Result<Vec<Vec<u8>>, String> {
    if frame.data.is_empty() {
        return Err("Cannot send an empty video frame.".to_string());
    }
    if frame.data.len() > MAX_FRAME_BYTES {
        return Err("Video frame exceeds the transport limit.".to_string());
    }

    let total = frame.data.len();
    let header = 1 + 4 + 4 + 4 + 1 + 8 + 4 + 4 + 4;
    let chunk_size = FRAME_CHUNK_BYTES.min(MAX_NOISE_MESSAGE - 16 - header);
    let mut messages = Vec::with_capacity(total.div_ceil(chunk_size));

    for offset in (0..total).step_by(chunk_size) {
        let end = (offset + chunk_size).min(total);
        let chunk = &frame.data[offset..end];
        let mut message = Vec::with_capacity(header + chunk.len());
        message.push(MESSAGE_FRAME);
        message.extend_from_slice(&frame.frame_id.to_be_bytes());
        message.extend_from_slice(&(total as u32).to_be_bytes());
        message.extend_from_slice(&(offset as u32).to_be_bytes());
        message.push(if frame.keyframe { FRAME_KEYFRAME } else { 0 });
        message.extend_from_slice(&frame.timestamp_ms.to_be_bytes());
        message.extend_from_slice(&frame.width.to_be_bytes());
        message.extend_from_slice(&frame.height.to_be_bytes());
        message.extend_from_slice(&frame.encode_ms.to_bits().to_be_bytes());
        message.extend_from_slice(chunk);
        messages.push(message);
    }

    Ok(messages)
}

pub fn parse_frame_chunk(message: &[u8]) -> Result<Option<FrameChunk<'_>>, String> {
    if message.is_empty() {
        return Err("Empty transport message.".to_string());
    }
    if message[0] != MESSAGE_FRAME {
        return Ok(None);
    }
    const HEADER: usize = 1 + 4 + 4 + 4 + 1 + 8 + 4 + 4 + 4;
    if message.len() < HEADER {
        return Err("Truncated video frame message.".to_string());
    }
    let frame_id = u32::from_be_bytes(message[1..5].try_into().expect("slice length"));
    let total_bytes = u32::from_be_bytes(message[5..9].try_into().expect("slice length")) as usize;
    let offset = u32::from_be_bytes(message[9..13].try_into().expect("slice length")) as usize;
    let keyframe = message[13] & FRAME_KEYFRAME != 0;
    let timestamp_ms = u64::from_be_bytes(message[14..22].try_into().expect("slice length"));
    let width = u32::from_be_bytes(message[22..26].try_into().expect("slice length"));
    let height = u32::from_be_bytes(message[26..30].try_into().expect("slice length"));
    let encode_ms = f32::from_bits(u32::from_be_bytes(
        message[30..34].try_into().expect("slice length"),
    ));
    let data = &message[HEADER..];

    if total_bytes == 0 || total_bytes > MAX_FRAME_BYTES {
        return Err("Invalid video frame size.".to_string());
    }
    if offset
        .checked_add(data.len())
        .is_none_or(|end| end > total_bytes)
    {
        return Err("Invalid video frame chunk bounds.".to_string());
    }

    Ok(Some(FrameChunk {
        frame_id,
        total_bytes,
        offset,
        keyframe,
        timestamp_ms,
        width,
        height,
        encode_ms,
        data,
    }))
}

pub fn parse_control<T: DeserializeOwned>(message: &[u8]) -> Result<Option<T>, String> {
    if message.is_empty() || message[0] != MESSAGE_CONTROL {
        return Ok(None);
    }
    serde_json::from_slice(&message[1..])
        .map(Some)
        .map_err(|error| format!("Invalid control message: {error}"))
}

struct PartialFrame {
    total: usize,
    keyframe: bool,
    timestamp_ms: u64,
    width: u32,
    height: u32,
    encode_ms: f32,
    chunks: Vec<(usize, Vec<u8>)>,
    filled: usize,
}

impl PartialFrame {
    fn new(chunk: &FrameChunk<'_>) -> Self {
        Self {
            total: chunk.total_bytes,
            keyframe: chunk.keyframe,
            timestamp_ms: chunk.timestamp_ms,
            width: chunk.width,
            height: chunk.height,
            encode_ms: chunk.encode_ms,
            chunks: Vec::new(),
            filled: 0,
        }
    }

    fn add(&mut self, chunk: FrameChunk<'_>) {
        if self
            .chunks
            .iter()
            .any(|(offset, _)| *offset == chunk.offset)
        {
            return;
        }
        self.filled += chunk.data.len();
        self.chunks.push((chunk.offset, chunk.data.to_vec()));
    }

    fn complete(&self) -> bool {
        self.filled == self.total
    }

    fn finish(mut self, frame_id: u32) -> Option<AssembledFrame> {
        self.chunks.sort_unstable_by_key(|(offset, _)| *offset);
        let mut data = Vec::with_capacity(self.total);
        let mut cursor = 0usize;
        for (offset, bytes) in self.chunks {
            if offset != cursor {
                return None;
            }
            cursor += bytes.len();
            data.extend_from_slice(&bytes);
        }
        if cursor != self.total {
            return None;
        }
        Some(AssembledFrame {
            frame_id,
            keyframe: self.keyframe,
            timestamp_ms: self.timestamp_ms,
            width: self.width,
            height: self.height,
            encode_ms: self.encode_ms,
            data,
        })
    }
}

/// Reassembles frame chunks from an unordered transport. Frames are emitted in
/// increasing ID order; a missing frame is skipped only when the pending budget
/// is exhausted, so brief reordering costs nothing but permanent holes cannot
/// stall the stream.
#[derive(Default)]
pub struct FrameAssembler {
    pending: std::collections::BTreeMap<u32, PartialFrame>,
    pending_bytes: usize,
    expected: Option<u32>,
}

impl FrameAssembler {
    pub fn push(&mut self, chunk: FrameChunk<'_>) -> Result<Vec<AssembledFrame>, String> {
        if self
            .expected
            .is_some_and(|expected| chunk.frame_id < expected)
        {
            return Ok(Vec::new());
        }

        match self.pending.get_mut(&chunk.frame_id) {
            Some(frame) if frame.total == chunk.total_bytes => frame.add(chunk),
            Some(_) => {
                self.remove(chunk.frame_id);
                return Ok(Vec::new());
            }
            None => {
                let frame = PartialFrame::new(&chunk);
                self.pending_bytes += frame.total;
                self.pending.insert(chunk.frame_id, frame);
                if let Some(frame) = self.pending.get_mut(&chunk.frame_id) {
                    frame.add(chunk);
                }
            }
        }

        let mut frames = Vec::new();
        while self.pending.len() > MAX_PENDING_FRAMES || self.pending_bytes > MAX_PENDING_BYTES {
            if self.expected.is_none() {
                let Some(&lowest) = self.pending.keys().next() else {
                    break;
                };
                self.expected = Some(lowest);
            }
            let expected = self.expected.expect("expected frame id");
            if !self.pending.contains_key(&expected) {
                let Some(&lowest) = self.pending.keys().next() else {
                    break;
                };
                self.expected = Some(lowest);
            }
            frames.extend(self.flush_ready());
            self.drop_stale();
            if self.pending.len() <= MAX_PENDING_FRAMES && self.pending_bytes <= MAX_PENDING_BYTES {
                break;
            }
            let expected = self.expected.expect("expected frame id");
            if self.pending.contains_key(&expected) {
                self.remove(expected);
                self.expected = Some(expected.wrapping_add(1));
            } else {
                break;
            }
        }
        frames.extend(self.flush_ready());
        self.drop_stale();
        Ok(frames)
    }

    fn drop_stale(&mut self) {
        let Some(expected) = self.expected else {
            return;
        };
        while let Some((&lowest, _)) = self.pending.iter().next() {
            if lowest < expected {
                self.remove(lowest);
            } else {
                break;
            }
        }
    }

    fn flush_ready(&mut self) -> Vec<AssembledFrame> {
        let mut frames = Vec::new();
        loop {
            if self.expected.is_none() {
                // Start from the lowest pending id, not the lowest *complete*
                // frame: an unordered transport can deliver frame 2 whole
                // before frame 1, and emitting 2 first would break the
                // decoder's reference chain.
                let Some(&lowest) = self.pending.keys().next() else {
                    break;
                };
                self.expected = Some(lowest);
            }
            let expected = self.expected.expect("expected frame id");
            if !self
                .pending
                .get(&expected)
                .is_some_and(PartialFrame::complete)
            {
                break;
            }
            let frame = self.pending.remove(&expected).expect("complete frame");
            self.pending_bytes = self.pending_bytes.saturating_sub(frame.total);
            self.expected = Some(expected.wrapping_add(1));
            if let Some(frame) = frame.finish(expected) {
                frames.push(frame);
            }
        }
        frames
    }

    fn remove(&mut self, frame_id: u32) {
        if let Some(frame) = self.pending.remove(&frame_id) {
            self.pending_bytes = self.pending_bytes.saturating_sub(frame.total);
        }
    }
}

pub struct AssembledFrame {
    pub frame_id: u32,
    #[allow(dead_code)] // Decoded from the wire header; the renderer only needs dimensions.
    pub keyframe: bool,
    pub timestamp_ms: u64,
    pub width: u32,
    pub height: u32,
    pub encode_ms: f32,
    pub data: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, thread};

    fn sample_frame(size: usize) -> Vec<u8> {
        (0..size).map(|index| (index % 251) as u8).collect()
    }

    #[test]
    fn normalizes_and_rejects_pair_codes() {
        assert_eq!(normalize_pair_code(" abcd-2345 ").unwrap(), "ABCD2345");
        assert_eq!(normalize_pair_code("ABCD-2345").unwrap(), "ABCD2345");
        assert!(normalize_pair_code("ABCD-234").is_err());
        assert!(normalize_pair_code("ABCD-2340").is_err());
        assert!(normalize_pair_code("ABCD-234I").is_err());
    }

    #[test]
    fn derives_stable_pairing_keys() {
        let first = pairing_psk("ABCD-2345").unwrap();
        let second = pairing_psk("abcd 2345").unwrap();
        let other = pairing_psk("WXYZ-6789").unwrap();
        assert_eq!(first, second);
        assert_ne!(first, other);
    }

    #[test]
    fn chunks_and_reassembles_a_frame() {
        let data = sample_frame(FRAME_CHUNK_BYTES * 2 + 123);
        let messages = frame_messages(&OutgoingFrame {
            frame_id: 42,
            keyframe: true,
            timestamp_ms: 1_234,
            width: 1920,
            height: 1080,
            encode_ms: 2.5,
            data: &data,
        })
        .unwrap();
        assert!(messages.len() >= 3);

        let mut assembler = FrameAssembler::default();
        let mut frames = Vec::new();
        for message in messages {
            let chunk = parse_frame_chunk(&message).unwrap().expect("frame chunk");
            frames.extend(assembler.push(chunk).unwrap());
        }
        let frame = frames.into_iter().next().expect("completed frame");
        assert_eq!(frame.frame_id, 42);
        assert!(frame.keyframe);
        assert_eq!(frame.timestamp_ms, 1_234);
        assert_eq!((frame.width, frame.height), (1920, 1080));
        assert!((frame.encode_ms - 2.5).abs() < f32::EPSILON);
        assert_eq!(frame.data, data);
    }

    fn frame_message(frame_id: u32, data: &[u8]) -> Vec<u8> {
        frame_messages(&OutgoingFrame {
            frame_id,
            keyframe: frame_id == 1,
            timestamp_ms: u64::from(frame_id),
            width: 640,
            height: 360,
            encode_ms: 1.0,
            data,
        })
        .unwrap()
        .remove(0)
    }

    #[test]
    fn reassembles_out_of_order_chunks() {
        let data = sample_frame(FRAME_CHUNK_BYTES * 3 + 17);
        let mut messages = frame_messages(&OutgoingFrame {
            frame_id: 9,
            keyframe: false,
            timestamp_ms: 5,
            width: 640,
            height: 360,
            encode_ms: 1.0,
            data: &data,
        })
        .unwrap();
        messages.reverse();

        let mut assembler = FrameAssembler::default();
        let mut frames = Vec::new();
        for message in messages {
            let chunk = parse_frame_chunk(&message).unwrap().expect("frame chunk");
            frames.extend(assembler.push(chunk).unwrap());
        }
        let frame = frames.into_iter().next().expect("completed frame");
        assert_eq!(frame.frame_id, 9);
        assert_eq!(frame.data, data);
    }

    #[test]
    fn starts_from_the_lowest_pending_frame() {
        let first = sample_frame(FRAME_CHUNK_BYTES * 2 + 40);
        let second = sample_frame(1_100);
        let messages = frame_messages(&OutgoingFrame {
            frame_id: 1,
            keyframe: true,
            timestamp_ms: 1,
            width: 640,
            height: 360,
            encode_ms: 1.0,
            data: &first,
        })
        .unwrap();
        let mut assembler = FrameAssembler::default();

        let chunk = parse_frame_chunk(&messages[0]).unwrap().unwrap();
        assert!(assembler.push(chunk).unwrap().is_empty());

        let second_message = frame_message(2, &second);
        let chunk = parse_frame_chunk(&second_message).unwrap().unwrap();
        assert!(
            assembler.push(chunk).unwrap().is_empty(),
            "a complete later frame must wait for an earlier partial frame"
        );

        let mut frames = Vec::new();
        for message in &messages[1..] {
            let chunk = parse_frame_chunk(message).unwrap().unwrap();
            frames.extend(assembler.push(chunk).unwrap());
        }
        assert_eq!(
            frames
                .iter()
                .map(|frame| frame.frame_id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(frames[0].data, first);
    }

    #[test]
    fn holds_later_frames_until_the_earlier_one_completes() {
        let first = sample_frame(2_000);
        let second = sample_frame(2_100);
        let third = sample_frame(2_200);
        let mut assembler = FrameAssembler::default();

        let first_message = frame_message(1, &first);
        let chunk = parse_frame_chunk(&first_message).unwrap().unwrap();
        let ids: Vec<u32> = assembler
            .push(chunk)
            .unwrap()
            .into_iter()
            .map(|frame| frame.frame_id)
            .collect();
        assert_eq!(ids, vec![1]);

        let third_message = frame_message(3, &third);
        let chunk = parse_frame_chunk(&third_message).unwrap().unwrap();
        assert!(assembler.push(chunk).unwrap().is_empty());

        let second_message = frame_message(2, &second);
        let chunk = parse_frame_chunk(&second_message).unwrap().unwrap();
        let frames = assembler.push(chunk).unwrap();
        assert_eq!(
            frames
                .iter()
                .map(|frame| frame.frame_id)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(frames[0].data, second);
        assert_eq!(frames[1].data, third);
    }

    #[test]
    fn skips_a_missing_frame_when_the_reorder_budget_runs_out() {
        let data = sample_frame(1_500);
        let mut assembler = FrameAssembler::default();

        let first_message = frame_message(1, &data);
        let chunk = parse_frame_chunk(&first_message).unwrap().unwrap();
        let emitted = assembler.push(chunk).unwrap();
        assert_eq!(emitted.len(), 1);

        let mut frames = Vec::new();
        for frame_id in 3..=(MAX_PENDING_FRAMES as u32 + 3) {
            let message = frame_message(frame_id, &data);
            let chunk = parse_frame_chunk(&message).unwrap().unwrap();
            frames.extend(assembler.push(chunk).unwrap());
        }
        let ids: Vec<u32> = frames.iter().map(|frame| frame.frame_id).collect();
        assert_eq!(ids.first(), Some(&3));
        assert!(ids.contains(&(MAX_PENDING_FRAMES as u32 + 3)));
        assert!(!ids.contains(&2));
    }

    #[test]
    fn rejects_malformed_frame_chunks() {
        let data = sample_frame(16);
        let message = frame_messages(&OutgoingFrame {
            frame_id: 1,
            keyframe: false,
            timestamp_ms: 0,
            width: 320,
            height: 240,
            encode_ms: 1.0,
            data: &data,
        })
        .unwrap()
        .remove(0);
        assert!(parse_frame_chunk(&message[..20]).is_err());

        let mut bad = message.clone();
        bad[9..13].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(parse_frame_chunk(&bad).is_err());
    }

    #[test]
    fn completes_authenticated_handshake_and_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let code = "ABCD-2345".to_string();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let noise = handshake_responder(&mut stream, &code).expect("responder");
            let mut channel = SecureChannel::new(stream, noise).expect("channel");
            let message = channel
                .recv_timeout(Duration::from_secs(2))
                .expect("receive")
                .expect("message");
            let control: serde_json::Value = parse_control(&message).unwrap().expect("control");
            assert_eq!(control["type"], "hello");
            channel
                .send_control(&serde_json::json!({ "type": "welcome" }))
                .expect("send welcome");

            let message = channel
                .recv_timeout(Duration::from_secs(2))
                .expect("receive")
                .expect("message");
            let feedback: Control = parse_control(&message).unwrap().expect("feedback");
            match feedback {
                Control::StreamFeedback {
                    received,
                    jitter_ms,
                    rtt_ms,
                    dropped,
                } => {
                    assert_eq!(received, 24);
                    assert!((jitter_ms - 8.5).abs() < f64::EPSILON);
                    assert!((rtt_ms - 21.0).abs() < f64::EPSILON);
                    assert_eq!(dropped, 1);
                }
                other => panic!("unexpected feedback: {other:?}"),
            }
            channel
                .send_control(&Control::StreamRate {
                    bitrate_mbps: 18.4,
                    ceiling_mbps: 35.0,
                    adaptive: true,
                })
                .expect("send rate");
        });

        let mut stream = TcpStream::connect(address).expect("connect");
        let noise = handshake_initiator(&mut stream, "abcd-2345").expect("initiator");
        let mut channel = SecureChannel::new(stream, noise).expect("channel");
        channel
            .send_control(&Control::Hello {
                app_version: "test".to_string(),
                device_name: "device".to_string(),
                owner_name: "owner".to_string(),
            })
            .expect("send hello");
        let message = channel
            .recv_timeout(Duration::from_secs(2))
            .expect("receive")
            .expect("message");
        let control: serde_json::Value = parse_control(&message).unwrap().expect("control");
        assert_eq!(control["type"], "welcome");

        channel
            .send_control(&Control::StreamFeedback {
                received: 24,
                jitter_ms: 8.5,
                rtt_ms: 21.0,
                dropped: 1,
            })
            .expect("send feedback");
        let message = channel
            .recv_timeout(Duration::from_secs(2))
            .expect("receive")
            .expect("message");
        let rate: Control = parse_control(&message).unwrap().expect("rate");
        match rate {
            Control::StreamRate {
                bitrate_mbps,
                ceiling_mbps,
                adaptive,
            } => {
                assert!((bitrate_mbps - 18.4).abs() < f64::EPSILON);
                assert!((ceiling_mbps - 35.0).abs() < f64::EPSILON);
                assert!(adaptive);
            }
            other => panic!("unexpected rate: {other:?}"),
        }
        server.join().expect("server thread");
    }

    #[test]
    fn rejects_a_wrong_pairing_code() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            assert!(handshake_responder(&mut stream, "ABCD-2345").is_err());
        });

        let mut stream = TcpStream::connect(address).expect("connect");
        if let Ok(noise) = handshake_initiator(&mut stream, "WXYZ-6789") {
            // The initiator only discovers a PSK mismatch when the responder's
            // first encrypted message fails to decrypt.
            let mut channel = SecureChannel::new(stream, noise).expect("channel");
            assert!(channel.recv_timeout(Duration::from_secs(2)).is_err());
        }
        server.join().expect("server thread");
    }

    #[test]
    fn round_trips_display_switch_controls() {
        let encoded =
            serde_json::to_vec(&Control::RequestDisplay { index: 3 }).expect("encode request");
        match serde_json::from_slice::<Control>(&encoded).expect("decode request") {
            Control::RequestDisplay { index } => assert_eq!(index, 3),
            other => panic!("unexpected request: {other:?}"),
        }

        let encoded = serde_json::to_vec(&Control::DisplayChanged {
            index: 3,
            width: 2880,
            height: 1712,
        })
        .expect("encode changed");
        match serde_json::from_slice::<Control>(&encoded).expect("decode changed") {
            Control::DisplayChanged {
                index,
                width,
                height,
            } => assert_eq!((index, width, height), (3, 2880, 1712)),
            other => panic!("unexpected changed: {other:?}"),
        }
    }

    #[test]
    fn round_trips_gamepad_controls() {
        let state = crate::gamepad::GamepadState {
            slot: 1,
            name: "Xbox Wireless Controller".to_string(),
            connected: true,
            axes: vec![0.0, -1.0, 0.5, 0.25],
            buttons: vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.75, 0.0],
        };
        let encoded = serde_json::to_vec(&Control::Gamepad { state }).expect("encode gamepad");
        let json = String::from_utf8(encoded.clone()).expect("utf8");
        assert!(json.contains("\"type\":\"gamepad\""));
        assert!(json.contains("\"connected\":true"));
        match serde_json::from_slice::<Control>(&encoded).expect("decode gamepad") {
            Control::Gamepad { state } => {
                assert_eq!(state.slot, 1);
                assert_eq!(state.axes, vec![0.0, -1.0, 0.5, 0.25]);
                assert_eq!(state.buttons[6], 0.75);
            }
            other => panic!("unexpected gamepad: {other:?}"),
        }
    }

    #[test]
    fn advertises_displays_and_defaults_them_for_old_peers() {
        let displays = vec![DisplayOption {
            index: 1,
            label: "display 1".to_string(),
            width: 1710,
            height: 1107,
        }];
        let welcome = Control::Welcome {
            width: 1710,
            height: 1107,
            allow_input: true,
            host_name: "Host".to_string(),
            displays: displays.clone(),
        };
        let encoded = serde_json::to_vec(&welcome).expect("encode welcome");
        match serde_json::from_slice::<Control>(&encoded).expect("decode welcome") {
            Control::Welcome {
                displays: decoded, ..
            } => assert_eq!(decoded, displays),
            other => panic!("unexpected welcome: {other:?}"),
        }

        // A host that predates display switching sends no `displays` field.
        let old =
            br#"{"type":"welcome","width":1920,"height":1080,"allowInput":true,"hostName":"Old"}"#;
        match serde_json::from_slice::<Control>(old).expect("decode old welcome") {
            Control::Welcome { displays, .. } => assert!(displays.is_empty()),
            other => panic!("unexpected old welcome: {other:?}"),
        }
    }

    #[test]
    fn display_controls_use_camel_case_tags_and_fields() {
        let value = serde_json::to_value(Control::RequestDisplay { index: 2 }).expect("encode");
        assert_eq!(value["type"], "requestDisplay");

        let value = serde_json::to_value(Control::DisplayChanged {
            index: 2,
            width: 2560,
            height: 1440,
        })
        .expect("encode");
        assert_eq!(value["type"], "displayChanged");
        assert_eq!(value["index"], 2);
        assert_eq!(value["width"], 2560);
        assert_eq!(value["height"], 1440);

        let value = serde_json::to_value(DisplayOption {
            index: 2,
            label: "display 2".to_string(),
            width: 2560,
            height: 1440,
        })
        .expect("encode option");
        assert_eq!(value["index"], 2);
        assert_eq!(value["label"], "display 2");
        assert_eq!(value["width"], 2560);
        assert_eq!(value["height"], 1440);
    }
}
