#[cfg(target_os = "macos")]
use crate::clipboard;
use crate::{
    adaptive::LinkMonitor,
    capture::now_ms,
    input::RemoteInput,
    reachability::{self, RelayInvite, RelayRole},
    rtp_video::{AssemblerOutput, RtpAssembler},
    transport::{
        self, Control, DisplayOption, FrameAssembler, SecureChannel, SessionChannel, TransportKind,
        DEFAULT_PORT,
    },
    video::{VideoSize, VideoSurface},
    webrtc_transport::{IceSettings, WebRtcChannel, WebRtcEndpoint},
};
use macos_media_toolkit::videotoolbox::{DecodeError, DecoderConfig, HevcDecoder};
use serde::Serialize;
use std::{
    io::ErrorKind,
    net::{TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager, State};

pub const CLIENT_VIDEO_EVENT: &str = "native-client:video";
pub const CLIENT_STATS_EVENT: &str = "native-client:stats";
pub const CLIENT_STOPPED_EVENT: &str = "native-client:stopped";
pub const CLIENT_INPUT_EVENT: &str = "native-client:input";
pub const CLIENT_DISPLAY_EVENT: &str = "native-client:display";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const WELCOME_TIMEOUT: Duration = Duration::from_secs(150);
const ANSWER_TIMEOUT: Duration = Duration::from_secs(12);
const WEBRTC_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const SESSION_READY_TIMEOUT: Duration = Duration::from_secs(180);
const RECEIVE_POLL: Duration = Duration::from_millis(4);
const MAX_MESSAGE_BURST: usize = 512;
const MAX_COMMAND_BURST: usize = 512;
const KEYFRAME_REQUEST_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientSummary {
    pub peer: String,
    pub host_name: String,
    pub width: u32,
    pub height: u32,
    pub allow_input: bool,
    pub connected_at: u64,
    pub path: String,
    pub transport: TransportKind,
    /// Displays the host can switch the live session to, from `Welcome`.
    pub displays: Vec<DisplayOption>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientStatus {
    pub connected: bool,
    pub summary: Option<ClientSummary>,
    pub allow_input: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatsPayload {
    sequence: u64,
    fps: f64,
    mbps: f64,
    decode_ms: f64,
    encode_ms: f64,
    rtt_ms: f64,
    frames: u64,
    width: u32,
    height: u32,
    uptime_secs: u64,
    stream_mbps: f64,
    ceiling_mbps: f64,
    adaptive: bool,
    jitter_ms: f64,
    dropped: u64,
    /// Candidate kinds that produced the RTP path, e.g. `srflx → host`.
    ice_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoppedPayload {
    reason: String,
    failed: bool,
    frames: u64,
    uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InputPayload {
    enabled: bool,
}

/// Event payload for `native-client:display`, sent when the host switches the
/// live capture to another of its displays.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DisplayPayload {
    index: usize,
    width: u32,
    height: u32,
}

enum ClientCommand {
    Input(RemoteInput),
    RequestKeyframe,
    RequestDisplay(usize),
    Disconnect,
}

struct ActiveClient {
    stop: Arc<AtomicBool>,
    sender: mpsc::Sender<ClientCommand>,
    thread: Option<JoinHandle<()>>,
    connected: Arc<AtomicBool>,
    input_enabled: Arc<AtomicBool>,
    /// Shared with the session thread so a display switch updates the live
    /// session size reported by `native_client_status`.
    summary: Arc<Mutex<Option<ClientSummary>>>,
    last_error: Mutex<Option<String>>,
}

#[derive(Default)]
pub struct NativeClient {
    active: Mutex<Option<ActiveClient>>,
}

impl NativeClient {
    fn status(&self) -> ClientStatus {
        let active = self.active.lock().expect("client state poisoned");
        match active.as_ref() {
            Some(client) if client.connected.load(Ordering::SeqCst) => ClientStatus {
                connected: true,
                summary: client
                    .summary
                    .lock()
                    .expect("client summary poisoned")
                    .clone(),
                allow_input: client.input_enabled.load(Ordering::SeqCst),
                last_error: client
                    .last_error
                    .lock()
                    .expect("client error poisoned")
                    .clone(),
            },
            Some(client) => ClientStatus {
                connected: false,
                summary: None,
                allow_input: false,
                last_error: client
                    .last_error
                    .lock()
                    .expect("client error poisoned")
                    .clone(),
            },
            None => ClientStatus {
                connected: false,
                summary: None,
                allow_input: false,
                last_error: None,
            },
        }
    }

    fn take(&self) -> Option<ActiveClient> {
        self.active.lock().expect("client state poisoned").take()
    }

    /// Asks the live session to switch the host's capture to `index`.
    fn request_display(&self, index: usize) -> Result<(), String> {
        let active = self.active.lock().expect("client state poisoned");
        let Some(active) = active.as_ref() else {
            return Err("The remote session is not connected.".to_string());
        };
        active
            .sender
            .send(ClientCommand::RequestDisplay(index))
            .map_err(|_| "The remote session has ended.".to_string())
    }

    pub fn shutdown(&self) {
        if let Some(client) = self.take() {
            client.stop.store(true, Ordering::SeqCst);
            let _ = client.sender.send(ClientCommand::Disconnect);
            if let Some(handle) = client.thread {
                let _ = handle.join();
            }
        }
    }
}

fn split_address(address: &str, port: Option<u16>) -> (String, u16) {
    if let Some(port) = port {
        return (address.trim().to_string(), port);
    }
    let address = address.trim();
    match address.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => match port.parse::<u16>() {
            Ok(port) => (host.to_string(), port),
            Err(_) => (address.to_string(), DEFAULT_PORT),
        },
        _ => (address.to_string(), DEFAULT_PORT),
    }
}

#[tauri::command(async)]
pub fn native_client_connect(
    app: AppHandle,
    client: State<'_, NativeClient>,
    video: State<'_, VideoSurface>,
    address: String,
    port: Option<u16>,
    code: String,
    owner_name: Option<String>,
    relay_invite: Option<String>,
    ice: Option<IceSettings>,
) -> Result<ClientSummary, String> {
    video.reset();
    let normalized = transport::normalize_pair_code(&code)?;
    let ice = ice.unwrap_or_default().normalized()?;
    let relay = relay_invite
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(RelayInvite::parse)
        .transpose()?;
    let (host, port, path) = match relay.as_ref() {
        Some(invite) => (
            invite.target.host.clone(),
            invite.target.port,
            "relay".to_string(),
        ),
        None => {
            let (host, port) = split_address(&address, port);
            if host.is_empty() {
                return Err("Enter the host's address to connect.".to_string());
            }
            (host, port, "direct".to_string())
        }
    };

    client.shutdown();

    let stop = Arc::new(AtomicBool::new(false));
    let connected = Arc::new(AtomicBool::new(true));
    let input_enabled = Arc::new(AtomicBool::new(false));
    let (command_tx, command_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    let thread_stop = Arc::clone(&stop);
    let thread_connected = Arc::clone(&connected);
    let thread_input = Arc::clone(&input_enabled);
    // Shared with the session thread so `DisplayChanged` can update the live
    // summary (size) that `native_client_status` reports.
    let summary_state: Arc<Mutex<Option<ClientSummary>>> = Arc::new(Mutex::new(None));
    let thread_summary = Arc::clone(&summary_state);
    let worker_app = app.clone();

    let handle = thread::Builder::new()
        .name("nexus-client".to_string())
        .spawn(move || {
            run_client(
                worker_app,
                host,
                port,
                normalized,
                owner_name.unwrap_or_default(),
                relay,
                path,
                ice,
                thread_stop,
                thread_connected,
                thread_input,
                thread_summary,
                command_rx,
                ready_tx,
            );
        })
        .map_err(|error| format!("Could not start the remote session thread: {error}"))?;

    match ready_rx.recv_timeout(SESSION_READY_TIMEOUT) {
        Ok(Ok(summary)) => {
            let _ = app.emit(
                CLIENT_INPUT_EVENT,
                InputPayload {
                    enabled: summary.allow_input,
                },
            );
            *client.active.lock().expect("client state poisoned") = Some(ActiveClient {
                stop,
                sender: command_tx,
                thread: Some(handle),
                connected,
                input_enabled,
                summary: summary_state,
                last_error: Mutex::new(None),
            });
            Ok(summary)
        }
        Ok(Err(error)) => {
            let _ = handle.join();
            Err(error)
        }
        Err(_) => {
            stop.store(true, Ordering::SeqCst);
            let _ = handle.join();
            Err("The remote computer did not answer in time.".to_string())
        }
    }
}

#[tauri::command]
pub fn native_client_disconnect(client: State<'_, NativeClient>) -> Result<(), String> {
    if let Some(active) = client.take() {
        active.stop.store(true, Ordering::SeqCst);
        active.connected.store(false, Ordering::SeqCst);
        let _ = active.sender.send(ClientCommand::Disconnect);
        if let Some(handle) = active.thread {
            let _ = handle.join();
        }
    }
    Ok(())
}

#[tauri::command]
pub fn native_client_status(client: State<'_, NativeClient>) -> ClientStatus {
    client.status()
}

#[tauri::command]
pub fn native_client_input(
    client: State<'_, NativeClient>,
    event: RemoteInput,
) -> Result<(), String> {
    let active = client.active.lock().expect("client state poisoned");
    let Some(active) = active.as_ref() else {
        return Err("The remote session is not connected.".to_string());
    };
    if !active.input_enabled.load(Ordering::SeqCst) {
        return Err("The host has not enabled remote input.".to_string());
    }
    active
        .sender
        .send(ClientCommand::Input(event))
        .map_err(|_| "The remote session has ended.".to_string())
}

#[tauri::command]
pub fn native_client_request_keyframe(client: State<'_, NativeClient>) -> Result<(), String> {
    let active = client.active.lock().expect("client state poisoned");
    let Some(active) = active.as_ref() else {
        return Err("The remote session is not connected.".to_string());
    };
    active
        .sender
        .send(ClientCommand::RequestKeyframe)
        .map_err(|_| "The remote session has ended.".to_string())
}

/// Asks the connected host to switch its capture to another of its displays.
#[tauri::command]
pub fn native_client_request_display(
    client: State<'_, NativeClient>,
    index: usize,
) -> Result<(), String> {
    client.request_display(index)
}

/// Applies a `DisplayChanged` to the shared summary and builds the event
/// payload the frontend receives on `native-client:display`.
#[cfg(target_os = "macos")]
fn note_display_changed(
    summary: &Arc<Mutex<Option<ClientSummary>>>,
    index: usize,
    width: u32,
    height: u32,
) -> DisplayPayload {
    if let Ok(mut summary) = summary.lock() {
        if let Some(summary) = summary.as_mut() {
            summary.width = width;
            summary.height = height;
        }
    }
    DisplayPayload {
        index,
        width,
        height,
    }
}

#[cfg(target_os = "macos")]
fn request_keyframe(channel: &mut SessionChannel, last: &mut Option<Instant>) {
    let now = Instant::now();
    if last.is_none_or(|sent| now.duration_since(sent) >= KEYFRAME_REQUEST_INTERVAL) {
        let _ = channel.send_control(&Control::RequestKeyframe);
        *last = Some(now);
    }
}

/// Waits for the host's SDP answer before committing to WebRTC. A fallback
/// message leaves the session on TCP; a transport failure is fatal.
#[cfg(target_os = "macos")]
fn await_transport_answer(
    channel: &mut SecureChannel,
    timeout: Duration,
) -> Result<Option<String>, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Ok(None);
        };
        match channel.recv_timeout(remaining.min(Duration::from_millis(50))) {
            Ok(Some(message)) => match transport::parse_control::<Control>(&message) {
                Ok(Some(Control::TransportAnswer { sdp })) => return Ok(Some(sdp)),
                Ok(Some(Control::TransportFallback { .. })) => return Ok(None),
                Ok(Some(Control::Error { message })) => return Err(message),
                Ok(Some(Control::Bye)) => return Err("The host ended the session.".to_string()),
                Ok(_) => {}
                Err(error) => return Err(format!("The host sent an invalid message: {error}")),
            },
            Ok(None) => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => return Err(format!("The remote computer disconnected: {error}")),
        }
    }
}

/// The host sends `Welcome` on the transport it selected. Poll both while it
/// decides; a closed TCP signaling socket is not fatal once WebRTC is open.
#[cfg(target_os = "macos")]
fn await_welcome(
    tcp: &mut SecureChannel,
    webrtc: &mut Option<WebRtcChannel>,
    timeout: Duration,
) -> Result<(Control, TransportKind), String> {
    let deadline = Instant::now() + timeout;
    let mut tcp_dead = false;
    let mut webrtc_dead = webrtc.is_none();
    let mut fallback_reason: Option<String> = None;

    loop {
        if !webrtc_dead {
            if let Some(channel) = webrtc.as_mut() {
                match channel.poll_recv() {
                    Ok(Some(message)) => match transport::parse_control::<Control>(&message) {
                        Ok(Some(welcome @ Control::Welcome { .. })) => {
                            return Ok((welcome, TransportKind::WebRtc))
                        }
                        Ok(Some(Control::Error { message })) => return Err(message),
                        Ok(Some(Control::Bye)) => {
                            return Err("The host ended the session.".to_string())
                        }
                        Ok(_) => {}
                        Err(error) => {
                            return Err(format!("The host sent an invalid message: {error}"))
                        }
                    },
                    Ok(None) => {}
                    Err(error) => {
                        webrtc_dead = true;
                        fallback_reason = Some(format!(
                            "The WebRTC session ended before it started: {error}"
                        ));
                    }
                }
            }
        }

        if !tcp_dead {
            match tcp.poll_recv() {
                Ok(Some(message)) => match transport::parse_control::<Control>(&message) {
                    Ok(Some(welcome @ Control::Welcome { .. })) => {
                        return Ok((welcome, TransportKind::Tcp))
                    }
                    Ok(Some(Control::TransportFallback { reason })) => {
                        fallback_reason = Some(reason);
                    }
                    Ok(Some(Control::Error { message })) => return Err(message),
                    Ok(Some(Control::Bye)) => return Err("The host ended the session.".to_string()),
                    Ok(_) => {}
                    Err(error) => return Err(format!("The host sent an invalid message: {error}")),
                },
                Ok(None) => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) => {
                    tcp_dead = true;
                    if webrtc_dead {
                        return Err(fallback_reason.unwrap_or_else(|| {
                            format!("The remote computer disconnected: {error}")
                        }));
                    }
                }
            }
        }

        if webrtc_dead && tcp_dead {
            return Err(fallback_reason
                .unwrap_or_else(|| "The remote session ended before it started.".to_string()));
        }
        if Instant::now() >= deadline {
            return Err(fallback_reason
                .unwrap_or_else(|| "The remote computer did not answer in time.".to_string()));
        }
        thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(target_os = "macos")]
fn run_client(
    app: AppHandle,
    host: String,
    port: u16,
    code: String,
    owner_name: String,
    relay: Option<RelayInvite>,
    path: String,
    ice: IceSettings,
    stop: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    input_enabled: Arc<AtomicBool>,
    summary_state: Arc<Mutex<Option<ClientSummary>>>,
    commands: mpsc::Receiver<ClientCommand>,
    ready: mpsc::Sender<Result<ClientSummary, String>>,
) {
    let started = Instant::now();
    let mut frames = 0u64;
    let mut failure: Option<String> = None;

    let result = (|| -> Result<(ClientSummary, SessionChannel, Option<String>), String> {
        let (mut stream, peer) = match relay.as_ref() {
            Some(invite) => {
                let stream =
                    reachability::open_relay(&invite.target, RelayRole::Client, &invite.room)?;
                (stream, format!("relay:{}", invite.target))
            }
            None => {
                let socket = (host.as_str(), port)
                    .to_socket_addrs()
                    .map_err(|error| format!("Could not resolve {host}: {error}"))?
                    .next()
                    .ok_or_else(|| format!("Could not resolve {host}."))?;
                let stream = TcpStream::connect_timeout(&socket, CONNECT_TIMEOUT)
                    .map_err(|error| format!("Could not reach {host}:{port}: {error}"))?;
                let peer = stream
                    .peer_addr()
                    .map(|address| address.to_string())
                    .unwrap_or_else(|_| format!("{host}:{port}"));
                (stream, peer)
            }
        };

        let noise = transport::handshake_initiator(&mut stream, &code)
            .map_err(|_| "The host rejected the pairing code.".to_string())?;
        let mut encrypted = SecureChannel::new(stream, noise)
            .map_err(|error| format!("Could not secure the session: {error}"))?;

        encrypted
            .send_control(&Control::Hello {
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                device_name: crate::host::local_device_name(),
                owner_name,
            })
            .map_err(|_| "The remote computer closed the connection.".to_string())?;

        // WebRTC runs alongside the PC's own TCP listener; relay sessions hop
        // across two TCP legs per side, so ICE would only see a relay peer and
        // gain nothing.
        let mut webrtc_channel: Option<WebRtcChannel> = None;
        if relay.is_none() {
            if let Ok((mut endpoint, offer)) = WebRtcEndpoint::offer_on("0.0.0.0:0", &ice) {
                encrypted
                    .send_control(&Control::TransportOffer { sdp: offer })
                    .map_err(|_| "The remote computer closed the connection.".to_string())?;
                if let Some(answer) = await_transport_answer(&mut encrypted, ANSWER_TIMEOUT)? {
                    if endpoint.apply_answer(&answer).is_ok() {
                        webrtc_channel = endpoint.connect(WEBRTC_CONNECT_TIMEOUT).ok();
                    }
                }
            }
        }

        encrypted
            .send_control(&Control::TransportChoice {
                webrtc: webrtc_channel.is_some(),
            })
            .map_err(|_| "The remote computer closed the connection.".to_string())?;

        let (welcome, transport) =
            await_welcome(&mut encrypted, &mut webrtc_channel, WELCOME_TIMEOUT)?;
        let ice_path = webrtc_channel
            .as_ref()
            .and_then(|channel| channel.ice_path());
        let (width, height, allow_input, host_name, displays) = match welcome {
            Control::Welcome {
                width,
                height,
                allow_input,
                host_name,
                displays,
            } => (width, height, allow_input, host_name, displays),
            _ => return Err("The host did not complete the session handshake.".to_string()),
        };

        input_enabled.store(allow_input, Ordering::SeqCst);
        let summary = ClientSummary {
            peer: peer.clone(),
            host_name,
            width,
            height,
            allow_input,
            connected_at: now_ms(),
            path,
            transport,
            displays,
        };
        *summary_state.lock().expect("client summary poisoned") = Some(summary.clone());
        let mut session = match transport {
            TransportKind::WebRtc => SessionChannel::WebRtc(
                webrtc_channel.expect("WebRTC selected without an open channel"),
            ),
            TransportKind::Tcp => SessionChannel::Tcp(encrypted),
        };
        if ready.send(Ok(summary.clone())).is_err() {
            let _ = session.send_control(&Control::Bye);
            session.shutdown();
            return Ok((summary, session, ice_path));
        }
        let _ = app.emit(
            CLIENT_INPUT_EVENT,
            InputPayload {
                enabled: allow_input,
            },
        );
        Ok((summary, session, ice_path))
    })();

    let (summary, mut channel, ice_path) = match result {
        Ok(session) => session,
        Err(error) => {
            connected.store(false, Ordering::SeqCst);
            let _ = ready.send(Err(error));
            return;
        }
    };
    let mut decoder = HevcDecoder::new(&DecoderConfig::default());
    let mut assembler = FrameAssembler::default();
    let mut sequence = 0u64;
    let mut last_keyframe_request: Option<Instant> = None;
    let mut stats_started = Instant::now();
    let mut stats_frames = 0u64;
    let mut stats_bytes = 0u64;
    let mut stats_decode_ms = 0.0f64;
    let mut stats_encode_ms = 0.0f64;
    let mut video_size: Option<VideoSize> = None;
    let mut stream_width = summary.width;
    let mut stream_height = summary.height;
    let mut ping_raised = Instant::now();
    let mut last_ping_sent: Option<u64> = None;
    let mut clipboard_change = clipboard::change_count();
    let mut clipboard_text = clipboard::read_text().unwrap_or_default();
    let mut clipboard_poll = Instant::now();
    let mut rtt_ms = 0.0f64;
    let mut link = LinkMonitor::new(started.elapsed().as_millis() as u64);
    let mut stream_mbps = 0.0f64;
    let mut ceiling_mbps = 0.0f64;
    let mut adaptive = false;
    let webrtc = channel.is_webrtc();
    let mut rtp_assembler = RtpAssembler::default();
    let mut rtp_last: Option<(u32, Instant)> = None;
    let mut rtp_jitter = 0.0f64;
    let mut rtp_dropped = 0u32;
    let mut feedback_frames = 0u32;
    let mut feedback_dropped = 0u32;
    let mut feedback_started = Instant::now();

    while !stop.load(Ordering::SeqCst) {
        for _ in 0..MAX_COMMAND_BURST {
            match commands.try_recv() {
                Ok(ClientCommand::Input(event)) => {
                    let control = match event {
                        RemoteInput::Gamepad { state } => Control::Gamepad { state },
                        event => Control::Input { event },
                    };
                    if channel.send_control(&control).is_err() {
                        failure = Some("The remote computer disconnected.".to_string());
                        break;
                    }
                }
                Ok(ClientCommand::RequestKeyframe) => {
                    let _ = channel.send_control(&Control::RequestKeyframe);
                }
                Ok(ClientCommand::RequestDisplay(index)) => {
                    let _ = channel.send_control(&Control::RequestDisplay { index });
                }
                Ok(ClientCommand::Disconnect) => {
                    let _ = channel.send_control(&Control::Bye);
                    stop.store(true, Ordering::SeqCst);
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
        if failure.is_some() {
            break;
        }

        let mut messages = Vec::with_capacity(MAX_MESSAGE_BURST);
        let mut video_packets = Vec::new();
        if webrtc {
            while video_packets.len() < MAX_MESSAGE_BURST {
                match channel.poll_video() {
                    Ok(Some(packet)) => video_packets.push(packet),
                    Ok(None) => break,
                    Err(error) => {
                        failure = Some(format!("The remote session ended: {error}"));
                        break;
                    }
                }
            }
            if failure.is_some() {
                break;
            }
            if video_packets.is_empty() {
                match channel.recv_timeout(RECEIVE_POLL) {
                    Ok(Some(message)) => messages.push(message),
                    Ok(None) => {}
                    Err(error) => {
                        failure = Some(format!("The remote session ended: {error}"));
                        break;
                    }
                }
            }
            while messages.len() < MAX_MESSAGE_BURST {
                match channel.poll_recv() {
                    Ok(Some(message)) => messages.push(message),
                    Ok(None) => break,
                    Err(error) => {
                        failure = Some(format!("The remote session ended: {error}"));
                        break;
                    }
                }
            }
        } else {
            let first = match channel.recv_timeout(RECEIVE_POLL) {
                Ok(Some(message)) => Some(message),
                Ok(None) => None,
                Err(error) => {
                    failure = Some(if error.kind() == ErrorKind::InvalidData {
                        "The encrypted session could not be verified. Reconnect with the current \
                         pairing code."
                            .to_string()
                    } else {
                        "The remote computer ended the session.".to_string()
                    });
                    break;
                }
            };
            if let Some(message) = first {
                messages.push(message);
            }
            while messages.len() < MAX_MESSAGE_BURST {
                match channel.poll_recv() {
                    Ok(Some(message)) => messages.push(message),
                    Ok(None) => break,
                    Err(error) => {
                        failure = Some(format!("The remote session ended: {error}"));
                        break;
                    }
                }
            }
        }
        if failure.is_some() {
            break;
        }

        for packet in video_packets {
            rtp_assembler.push(packet, Instant::now());
            while let Some(output) = rtp_assembler.poll(Instant::now()) {
                match output {
                    AssemblerOutput::Frame(unit) => {
                        stats_bytes += unit.annex_b.len() as u64;
                        if let Some((last_ts, last_arrival)) = rtp_last {
                            let arrival_ms = Instant::now()
                                .saturating_duration_since(last_arrival)
                                .as_secs_f64()
                                * 1000.0;
                            let media_ms =
                                f64::from(unit.rtp_timestamp.wrapping_sub(last_ts) as i32) / 90.0;
                            let sample = (arrival_ms - media_ms).abs();
                            rtp_jitter = if rtp_jitter > 0.0 {
                                rtp_jitter * 0.9 + sample * 0.1
                            } else {
                                sample
                            }
                            .clamp(0.0, 60_000.0);
                        }
                        rtp_last = Some((unit.rtp_timestamp, Instant::now()));
                        let decode_started = Instant::now();
                        match decoder.decode(&unit.annex_b) {
                            Ok(decoded) => {
                                let decode_ms = decode_started.elapsed().as_secs_f64() * 1000.0;
                                let size = VideoSize {
                                    width: decoded.frame.width,
                                    height: decoded.frame.height,
                                };
                                sequence += 1;
                                frames += 1;
                                stats_frames += 1;
                                stats_decode_ms += decode_ms;
                                feedback_frames = feedback_frames.saturating_add(1);
                                app.state::<VideoSurface>().submit(decoded.frame);
                                if video_size != Some(size) {
                                    video_size = Some(size);
                                    let _ = app.emit(CLIENT_VIDEO_EVENT, size);
                                }
                            }
                            Err(DecodeError::MissingKeyframe) | Err(DecodeError::Frame(_)) => {
                                rtp_dropped = rtp_dropped.saturating_add(1);
                                request_keyframe(&mut channel, &mut last_keyframe_request);
                            }
                            Err(DecodeError::Fatal(error)) => {
                                failure = Some(format!("Video decoding failed: {error}"));
                            }
                        }
                    }
                    AssemblerOutput::Loss => {
                        rtp_dropped = rtp_dropped.saturating_add(1);
                        request_keyframe(&mut channel, &mut last_keyframe_request);
                    }
                }
                if failure.is_some() {
                    break;
                }
            }
            if failure.is_some() {
                break;
            }
        }
        if failure.is_some() {
            break;
        }

        for message in messages {
            if let Ok(Some(chunk)) = transport::parse_frame_chunk(&message) {
                match assembler.push(chunk) {
                    Ok(completed) => {
                        for frame in completed {
                            stats_bytes += frame.data.len() as u64;
                            stream_width = frame.width;
                            stream_height = frame.height;
                            let arrival_ms = started.elapsed().as_millis() as u64;
                            if link.observe_frame(frame.frame_id, frame.timestamp_ms, arrival_ms) {
                                request_keyframe(&mut channel, &mut last_keyframe_request);
                            }
                            let decode_started = Instant::now();
                            match decoder.decode(&frame.data) {
                                Ok(decoded) => {
                                    let decode_ms = decode_started.elapsed().as_secs_f64() * 1000.0;
                                    let size = VideoSize {
                                        width: decoded.frame.width,
                                        height: decoded.frame.height,
                                    };
                                    sequence = frame.frame_id as u64;
                                    frames += 1;
                                    stats_frames += 1;
                                    stats_decode_ms += decode_ms;
                                    stats_encode_ms += f64::from(frame.encode_ms);
                                    app.state::<VideoSurface>().submit(decoded.frame);
                                    if video_size != Some(size) {
                                        video_size = Some(size);
                                        let _ = app.emit(CLIENT_VIDEO_EVENT, size);
                                    }
                                }
                                Err(DecodeError::MissingKeyframe) | Err(DecodeError::Frame(_)) => {
                                    link.observe_failure();
                                    request_keyframe(&mut channel, &mut last_keyframe_request);
                                }
                                Err(DecodeError::Fatal(error)) => {
                                    failure = Some(format!("Video decoding failed: {error}"));
                                }
                            }
                            if failure.is_some() {
                                break;
                            }
                        }
                    }
                    Err(error) => {
                        failure = Some(format!("The remote video stream was malformed: {error}"));
                    }
                }
            } else if let Ok(Some(control)) = transport::parse_control::<Control>(&message) {
                match control {
                    Control::InputState { enabled } => {
                        input_enabled.store(enabled, Ordering::SeqCst);
                        let _ = app.emit(CLIENT_INPUT_EVENT, InputPayload { enabled });
                    }
                    Control::Clipboard { text } => {
                        if clipboard::is_forwardable(&text) && text != clipboard_text {
                            clipboard::write_text(&text);
                            clipboard_change = clipboard::change_count();
                            clipboard_text = text;
                        }
                    }
                    Control::Pong { sent_ms } => {
                        if last_ping_sent == Some(sent_ms) {
                            rtt_ms = now_ms().saturating_sub(sent_ms) as f64;
                        }
                    }
                    Control::StreamRate {
                        bitrate_mbps,
                        ceiling_mbps: ceiling,
                        adaptive: is_adaptive,
                    } => {
                        stream_mbps = if bitrate_mbps.is_finite() {
                            bitrate_mbps.clamp(0.0, 1_000.0)
                        } else {
                            0.0
                        };
                        ceiling_mbps = if ceiling.is_finite() {
                            ceiling.clamp(0.0, 1_000.0)
                        } else {
                            0.0
                        };
                        adaptive = is_adaptive;
                    }
                    Control::Error { message } => {
                        failure = Some(message);
                    }
                    Control::DisplayChanged {
                        index,
                        width,
                        height,
                    } => {
                        // The host's capture is now a different display. Keep
                        // the reported session size in step and let the HUD
                        // re-label the stream; the video path is unchanged.
                        stream_width = width;
                        stream_height = height;
                        let payload = note_display_changed(&summary_state, index, width, height);
                        let _ = app.emit(CLIENT_DISPLAY_EVENT, payload);
                    }
                    Control::Bye => {
                        failure = Some("The host ended the session.".to_string());
                    }
                    _ => {}
                }
            }
            if failure.is_some() {
                break;
            }
        }
        if failure.is_some() {
            break;
        }

        if clipboard_poll.elapsed() >= Duration::from_millis(500) {
            clipboard_poll = Instant::now();
            let change = clipboard::change_count();
            if change != clipboard_change {
                clipboard_change = change;
                if let Some(text) = clipboard::read_text() {
                    if clipboard::is_forwardable(&text) && text != clipboard_text {
                        clipboard_text = text.clone();
                        let _ = channel.send_control(&Control::Clipboard { text });
                    }
                }
            }
        }

        if ping_raised.elapsed() >= Duration::from_secs(1) {
            let sent_ms = now_ms();
            if channel.send_control(&Control::Ping { sent_ms }).is_ok() {
                last_ping_sent = Some(sent_ms);
            }
            ping_raised = Instant::now();
        }

        if webrtc {
            if feedback_started.elapsed() >= Duration::from_secs(1) {
                let received = std::mem::take(&mut feedback_frames);
                let dropped = std::mem::take(&mut feedback_dropped);
                let _ = channel.send_control(&Control::StreamFeedback {
                    received,
                    jitter_ms: rtp_jitter,
                    rtt_ms,
                    dropped,
                });
                feedback_started = Instant::now();
            }
        } else if let Some(report) = link.report(started.elapsed().as_millis() as u64) {
            let _ = channel.send_control(&Control::StreamFeedback {
                received: report.received,
                jitter_ms: report.jitter_ms,
                rtt_ms,
                dropped: report.dropped,
            });
        }

        if stats_started.elapsed() >= Duration::from_secs(1) {
            let seconds = stats_started.elapsed().as_secs_f64().max(0.001);
            let decoded = stats_frames.max(1) as f64;
            let _ = app.emit(
                CLIENT_STATS_EVENT,
                StatsPayload {
                    sequence,
                    fps: stats_frames as f64 / seconds,
                    mbps: stats_bytes as f64 * 8.0 / 1_000_000.0 / seconds,
                    decode_ms: stats_decode_ms / decoded,
                    encode_ms: stats_encode_ms / decoded,
                    rtt_ms,
                    frames,
                    width: stream_width,
                    height: stream_height,
                    uptime_secs: started.elapsed().as_secs(),
                    stream_mbps,
                    ceiling_mbps,
                    adaptive,
                    ice_path: ice_path.clone(),
                    jitter_ms: if webrtc { rtp_jitter } else { link.jitter_ms() },
                    dropped: if webrtc {
                        u64::from(rtp_dropped)
                    } else {
                        link.dropped_frames()
                    },
                },
            );
            stats_started = Instant::now();
            stats_frames = 0;
            stats_bytes = 0;
            stats_decode_ms = 0.0;
            stats_encode_ms = 0.0;
        }
    }

    let failed = failure.is_some();
    let reason = failure.unwrap_or_else(|| "Remote session ended.".to_string());
    let _ = channel.send_control(&Control::Bye);
    channel.shutdown();
    input_enabled.store(false, Ordering::SeqCst);
    connected.store(false, Ordering::SeqCst);
    let _ = app.emit(CLIENT_INPUT_EVENT, InputPayload { enabled: false });
    let _ = app.emit(
        CLIENT_STOPPED_EVENT,
        StoppedPayload {
            reason,
            failed,
            frames,
            uptime_secs: started.elapsed().as_secs(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::DEFAULT_PORT;

    #[test]
    fn parses_host_and_port_from_the_address() {
        assert_eq!(
            split_address("192.168.1.20", None),
            ("192.168.1.20".to_string(), DEFAULT_PORT)
        );
        assert_eq!(
            split_address(" workstation.local:40000 ", None),
            ("workstation.local".to_string(), 40000)
        );
        assert_eq!(
            split_address("workstation.local", Some(41000)),
            ("workstation.local".to_string(), 41000)
        );
        assert_eq!(
            split_address("workstation.local:abc", None),
            ("workstation.local:abc".to_string(), DEFAULT_PORT)
        );
    }

    /// Mirrors the real handshake: offer over Noise, answer back, then the host
    /// closes the signaling socket and greets over WebRTC. The client must not
    /// treat the TCP close as a session failure.
    #[cfg(target_os = "macos")]
    #[test]
    fn selects_webrtc_after_the_host_closes_the_signaling_socket() {
        use super::{await_transport_answer, await_welcome};
        use crate::{
            transport::{
                handshake_initiator, handshake_responder, parse_control, Control, SecureChannel,
                TransportKind,
            },
            webrtc_transport::{IceSettings, WebRtcEndpoint},
        };
        use std::{net::TcpListener, thread, time::Duration};

        const CODE: &str = "ABCD-2345";
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");

        let host = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let noise = handshake_responder(&mut stream, CODE).expect("host handshake");
            let mut tcp = SecureChannel::new(stream, noise).expect("host channel");
            let hello = tcp
                .recv_timeout(Duration::from_secs(5))
                .expect("hello")
                .expect("hello message");
            assert!(matches!(
                parse_control::<Control>(&hello).expect("parse hello"),
                Some(Control::Hello { .. })
            ));
            let offer = tcp
                .recv_timeout(Duration::from_secs(5))
                .expect("offer")
                .expect("offer message");
            let sdp = match parse_control::<Control>(&offer)
                .expect("parse offer")
                .expect("offer control")
            {
                Control::TransportOffer { sdp } => sdp,
                other => panic!("unexpected control: {other:?}"),
            };
            let (endpoint, answer) =
                WebRtcEndpoint::answer_on(&sdp, "127.0.0.1:0", 35.0, true, &IceSettings::default())
                    .expect("answer");
            tcp.send_control(&Control::TransportAnswer { sdp: answer })
                .expect("send answer");
            let choice = tcp
                .recv_timeout(Duration::from_secs(10))
                .expect("choice")
                .expect("choice message");
            assert!(matches!(
                parse_control::<Control>(&choice).expect("parse choice"),
                Some(Control::TransportChoice { webrtc: true })
            ));
            let mut webrtc = endpoint
                .connect(Duration::from_secs(10))
                .expect("host webrtc");
            // The host drops the signaling channel when it commits to WebRTC,
            // so the greeting must arrive over the data channels.
            drop(tcp);
            thread::sleep(Duration::from_millis(50));
            webrtc
                .send_control(&Control::Welcome {
                    width: 1280,
                    height: 720,
                    allow_input: false,
                    host_name: "Test host".to_string(),
                    displays: Vec::new(),
                })
                .expect("send welcome");
            thread::sleep(Duration::from_secs(1));
        });

        let mut stream = std::net::TcpStream::connect(address).expect("connect");
        let noise = handshake_initiator(&mut stream, CODE).expect("client handshake");
        let mut tcp = SecureChannel::new(stream, noise).expect("client channel");
        tcp.send_control(&Control::Hello {
            app_version: "test".to_string(),
            device_name: "Test client".to_string(),
            owner_name: String::new(),
        })
        .expect("send hello");

        let (mut endpoint, offer) =
            WebRtcEndpoint::offer_on("127.0.0.1:0", &IceSettings::default()).expect("offer");
        tcp.send_control(&Control::TransportOffer { sdp: offer })
            .expect("send offer");
        let answer = await_transport_answer(&mut tcp, Duration::from_secs(10))
            .expect("wait for answer")
            .expect("answer");
        endpoint.apply_answer(&answer).expect("apply answer");
        let mut webrtc = Some(
            endpoint
                .connect(Duration::from_secs(10))
                .expect("client webrtc"),
        );
        tcp.send_control(&Control::TransportChoice { webrtc: true })
            .expect("send choice");

        let (welcome, transport) =
            await_welcome(&mut tcp, &mut webrtc, Duration::from_secs(10)).expect("welcome");
        assert_eq!(transport, TransportKind::WebRtc);
        match welcome {
            Control::Welcome { width, height, .. } => {
                assert_eq!((width, height), (1280, 720));
            }
            other => panic!("unexpected greeting: {other:?}"),
        }
        host.join().expect("host thread");
    }

    fn test_summary() -> ClientSummary {
        ClientSummary {
            peer: "192.168.195.252:39200".to_string(),
            host_name: "Host".to_string(),
            width: 1710,
            height: 1107,
            allow_input: true,
            connected_at: 0,
            path: "direct".to_string(),
            transport: TransportKind::Tcp,
            displays: vec![DisplayOption {
                index: 1,
                label: "display 1".to_string(),
                width: 1710,
                height: 1107,
            }],
        }
    }

    #[test]
    fn welcome_carries_the_host_display_list() {
        use crate::transport::parse_control;

        let welcome = Control::Welcome {
            width: 2560,
            height: 1440,
            allow_input: true,
            host_name: "Host".to_string(),
            displays: vec![
                DisplayOption {
                    index: 1,
                    label: "display 1".to_string(),
                    width: 1710,
                    height: 1107,
                },
                DisplayOption {
                    index: 2,
                    label: "display 2".to_string(),
                    width: 2560,
                    height: 1440,
                },
            ],
        };
        let mut payload = vec![crate::transport::MESSAGE_CONTROL];
        payload.extend(serde_json::to_vec(&welcome).expect("encode welcome"));
        match parse_control::<Control>(&payload)
            .expect("parse welcome")
            .expect("welcome")
        {
            Control::Welcome {
                width,
                height,
                displays,
                ..
            } => {
                assert_eq!((width, height), (2560, 1440));
                assert_eq!(displays.len(), 2);
                assert_eq!(displays[0].index, 1);
                assert_eq!((displays[1].width, displays[1].height), (2560, 1440));
            }
            other => panic!("unexpected control: {other:?}"),
        }
    }

    #[test]
    fn display_changed_updates_the_summary_and_event_payload() {
        let summary = Arc::new(Mutex::new(Some(test_summary())));
        let payload = note_display_changed(&summary, 2, 2560, 1440);
        assert_eq!(
            (payload.index, payload.width, payload.height),
            (2, 2560, 1440)
        );
        let updated = summary
            .lock()
            .expect("summary")
            .clone()
            .expect("summary present");
        assert_eq!((updated.width, updated.height), (2560, 1440));
        // Display metadata from Welcome survives the size update.
        assert_eq!(updated.displays.len(), 1);

        let encoded = serde_json::to_value(DisplayPayload {
            index: 2,
            width: 2560,
            height: 1440,
        })
        .expect("encode payload");
        assert_eq!(encoded["index"], 2);
        assert_eq!(encoded["width"], 2560);
        assert_eq!(encoded["height"], 1440);
    }

    #[test]
    fn display_request_requires_an_active_session() {
        let client = NativeClient::default();
        let error = client
            .request_display(1)
            .expect_err("a disconnected client cannot switch displays");
        assert!(error.contains("not connected"), "{error}");

        let (sender, receiver) = mpsc::channel();
        *client.active.lock().expect("client state") = Some(ActiveClient {
            stop: Arc::new(AtomicBool::new(false)),
            sender,
            thread: None,
            connected: Arc::new(AtomicBool::new(true)),
            input_enabled: Arc::new(AtomicBool::new(true)),
            summary: Arc::new(Mutex::new(None)),
            last_error: Mutex::new(None),
        });
        client
            .request_display(3)
            .expect("queued for the session thread");
        assert!(matches!(
            receiver.recv().expect("command"),
            ClientCommand::RequestDisplay(3)
        ));
    }
}
