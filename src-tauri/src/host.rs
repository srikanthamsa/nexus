#[cfg(target_os = "macos")]
use crate::clipboard;
use crate::{
    adaptive::AdaptiveRate,
    capture::{now_ms, CaptureOptions, CapturePipeline, CaptureTick},
    congestion::encoder_mbps_for,
    gamepad::VirtualGamepad,
    input::{NativeInput, RemoteInput},
    ojd::{self, OjdPad},
    reachability::{self, DiscoveryAdvertisement, RelayInvite, RelayRole, RelayTarget},
    transport::{
        self, generate_pair_code, normalize_pair_code, Control, DisplayOption, OutgoingFrame,
        SecureChannel, SessionChannel, DEFAULT_PORT,
    },
    webrtc_transport::{IceSettings, WebRtcChannel, WebRtcEndpoint},
};
use serde::Serialize;
use std::{
    io::ErrorKind,
    net::{Shutdown, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, State};

pub const HOST_STATUS_EVENT: &str = "native-host:status";

const ACCEPT_POLL: Duration = Duration::from_millis(20);
const CAPTURE_POLL: Duration = Duration::from_millis(4);
const HELLO_WAIT: Duration = Duration::from_secs(5);
/// How long a pending request stays in the host's presence dock before it
/// expires. The client waits longer than this so the host can decide.
const APPROVAL_WAIT: Duration = Duration::from_secs(120);
const MAX_VIEWERS: usize = 8;
const APPROVAL_POLL: Duration = Duration::from_millis(20);
const TRANSPORT_WAIT: Duration = Duration::from_secs(20);
const WEBRTC_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_CONTROL_BURST: usize = 64;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSummary {
    pub address: String,
    pub port: u16,
    pub code: String,
    pub started_at: u64,
    pub relay: Option<RelaySummary>,
    pub nearby: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RelayState {
    Connecting,
    Waiting,
    Paired,
    Offline,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelaySummary {
    pub address: String,
    pub invite: String,
    pub state: RelayState,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
struct RelayRuntime {
    target: RelayTarget,
    room: String,
    state: RelayState,
    error: Option<String>,
}

/// One device waiting for approval. The presence dock renders these until the
/// host accepts/rejects or the request expires.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingConnection {
    pub peer_id: String,
    pub peer: String,
    pub device: String,
    pub owner: String,
    pub expires_in_ms: u64,
}

/// Live per-viewer metrics, refreshed by that viewer's session loop.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct ViewerStats {
    fps: f64,
    mbps: f64,
    frames: u64,
    uptime_secs: u64,
    bitrate_mbps: f64,
    ceiling_mbps: f64,
    adaptive: bool,
    delivery_ratio: f64,
    jitter_ms: f64,
    /// Candidate kinds that produced the RTP path, e.g. `srflx → host`.
    ice_path: Option<String>,
}

/// One connected viewer as shown in the presence dock.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerSummary {
    pub peer_id: String,
    pub peer: String,
    pub device: String,
    pub owner: String,
    pub connected_at: u64,
    pub transport: String,
    pub keyboard: bool,
    pub mouse: bool,
    pub gamepad: bool,
    /// Player slot for controller passthrough (host is 0, viewers start at 1).
    pub controller_slot: u8,
    #[serde(flatten)]
    stats: ViewerStats,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatus {
    pub running: bool,
    pub streaming: bool,
    pub allow_input: bool,
    /// Whether controller passthrough is enabled for this host run.
    pub gamepad: bool,
    /// Why the virtual gamepad could not be created, if it failed.
    pub gamepad_error: Option<String>,
    pub require_approval: bool,
    pub pending: Vec<PendingConnection>,
    pub viewers: Vec<ViewerSummary>,
    pub summary: Option<HostSummary>,
    pub last_error: Option<String>,
}

/// Per-viewer state the session thread, presence dock and permission commands
/// all share.
struct PeerState {
    peer_id: String,
    peer: String,
    device: String,
    owner: String,
    connected_at: u64,
    transport: Mutex<String>,
    keyboard: AtomicBool,
    mouse: AtomicBool,
    gamepad: AtomicBool,
    controller_slot: u8,
    kick: AtomicBool,
    stats: Mutex<ViewerStats>,
}

struct PendingRequest {
    peer_id: String,
    peer: String,
    device: String,
    owner: String,
    started: Instant,
    decision: mpsc::Sender<bool>,
}

struct HostShared {
    app: AppHandle,
    address: String,
    port: u16,
    started_at: u64,
    code: Mutex<String>,
    allow_input: AtomicBool,
    allow_gamepad: AtomicBool,
    gamepad_error: Mutex<Option<String>>,
    ojd_config: Option<ojd::OjdConfig>,
    running: AtomicBool,
    streaming: AtomicBool,
    require_approval: AtomicBool,
    nearby: AtomicBool,
    /// Serialises relay sessions; direct sessions run concurrently.
    session: Mutex<()>,
    relay: Mutex<Option<RelayRuntime>>,
    pending: Mutex<Vec<PendingRequest>>,
    peers: Mutex<Vec<Arc<PeerState>>>,
    peer_seq: std::sync::atomic::AtomicU64,
    last_error: Mutex<Option<String>>,
    ice: IceSettings,
    media_port: u16,
}

impl HostShared {
    fn summary(&self) -> HostSummary {
        HostSummary {
            address: self.address.clone(),
            port: self.port,
            code: self.code.lock().expect("host code poisoned").clone(),
            started_at: self.started_at,
            relay: self.relay_summary(),
            nearby: self.nearby.load(Ordering::SeqCst),
        }
    }

    fn relay_summary(&self) -> Option<RelaySummary> {
        self.relay
            .lock()
            .expect("host relay poisoned")
            .as_ref()
            .map(|runtime| RelaySummary {
                address: runtime.target.to_string(),
                invite: RelayInvite::new(runtime.target.clone(), runtime.room.clone())
                    .map(|invite| invite.encode())
                    .unwrap_or_default(),
                state: runtime.state,
                error: runtime.error.clone(),
            })
    }

    fn set_relay_state(&self, state: RelayState, error: Option<String>) {
        {
            let mut relay = self.relay.lock().expect("host relay poisoned");
            if let Some(runtime) = relay.as_mut() {
                runtime.state = state;
                runtime.error = error;
            }
        }
        self.emit();
    }

    fn rotate_relay_room(&self) -> Option<String> {
        let mut relay = self.relay.lock().expect("host relay poisoned");
        let runtime = relay.as_mut()?;
        runtime.room = reachability::generate_room();
        Some(runtime.room.clone())
    }

    fn status(&self) -> HostStatus {
        let running = self.running.load(Ordering::SeqCst);
        if !running {
            return HostStatus {
                running: false,
                streaming: false,
                allow_input: false,
                gamepad: false,
                gamepad_error: None,
                require_approval: self.require_approval.load(Ordering::SeqCst),
                pending: Vec::new(),
                viewers: Vec::new(),
                summary: None,
                last_error: self.last_error.lock().expect("host error poisoned").clone(),
            };
        }
        let now = Instant::now();
        let pending: Vec<PendingConnection> = self
            .pending
            .lock()
            .expect("host pending poisoned")
            .iter()
            .map(|request| PendingConnection {
                peer_id: request.peer_id.clone(),
                peer: request.peer.clone(),
                device: request.device.clone(),
                owner: request.owner.clone(),
                expires_in_ms: APPROVAL_WAIT
                    .checked_sub(now.saturating_duration_since(request.started))
                    .unwrap_or_default()
                    .as_millis() as u64,
            })
            .collect();
        let viewers: Vec<ViewerSummary> = self
            .peers
            .lock()
            .expect("host peers poisoned")
            .iter()
            .map(|peer| ViewerSummary {
                peer_id: peer.peer_id.clone(),
                peer: peer.peer.clone(),
                device: peer.device.clone(),
                owner: peer.owner.clone(),
                connected_at: peer.connected_at,
                transport: peer
                    .transport
                    .lock()
                    .expect("viewer transport poisoned")
                    .clone(),
                keyboard: peer.keyboard.load(Ordering::SeqCst),
                mouse: peer.mouse.load(Ordering::SeqCst),
                gamepad: peer.gamepad.load(Ordering::SeqCst),
                controller_slot: peer.controller_slot,
                stats: peer.stats.lock().expect("viewer stats poisoned").clone(),
            })
            .collect();
        HostStatus {
            running,
            streaming: !viewers.is_empty(),
            allow_input: self.allow_input.load(Ordering::SeqCst),
            gamepad: self.allow_gamepad.load(Ordering::SeqCst),
            gamepad_error: self
                .gamepad_error
                .lock()
                .expect("host gamepad error poisoned")
                .clone(),
            require_approval: self.require_approval.load(Ordering::SeqCst),
            pending,
            viewers,
            summary: Some(self.summary()),
            last_error: self.last_error.lock().expect("host error poisoned").clone(),
        }
    }

    fn next_peer_id(&self) -> String {
        format!(
            "peer-{:x}-{:x}",
            now_ms(),
            self.peer_seq.fetch_add(1, Ordering::SeqCst)
        )
    }

    fn register_pending(
        &self,
        peer_id: String,
        peer: String,
        device: String,
        owner: String,
    ) -> mpsc::Receiver<bool> {
        let (decision, receiver) = mpsc::channel();
        self.pending
            .lock()
            .expect("host pending poisoned")
            .push(PendingRequest {
                peer_id,
                peer,
                device,
                owner,
                started: Instant::now(),
                decision,
            });
        self.emit();
        receiver
    }

    fn resolve_pending(&self, peer_id: &str, approve: bool) -> Result<(), String> {
        let request = {
            let mut pending = self.pending.lock().expect("host pending poisoned");
            let index = pending
                .iter()
                .position(|request| request.peer_id == peer_id)
                .ok_or_else(|| "That connection request is no longer waiting.".to_string())?;
            pending.remove(index)
        };
        request
            .decision
            .send(approve)
            .map_err(|_| "The waiting device already disconnected.".to_string())
    }

    fn drop_pending(&self, peer_id: &str) {
        self.pending
            .lock()
            .expect("host pending poisoned")
            .retain(|request| request.peer_id != peer_id);
        self.emit();
    }

    fn clear_pending(&self) {
        self.pending.lock().expect("host pending poisoned").clear();
        self.emit();
    }

    fn add_peer(
        &self,
        peer_id: String,
        peer: String,
        device: String,
        owner: String,
        transport: &str,
        ice_path: Option<String>,
    ) {
        // Reuse the lowest free controller slot so a reconnect does not
        // exhaust a game that only supports four pads.
        let controller_slot = {
            let peers = self.peers.lock().expect("host peers poisoned");
            let mut slot = 1u8;
            while peers.iter().any(|peer| peer.controller_slot == slot) {
                slot = slot.saturating_add(1);
            }
            slot
        };
        let state = Arc::new(PeerState {
            peer_id,
            peer,
            device,
            owner,
            connected_at: now_ms(),
            transport: Mutex::new(transport.to_string()),
            keyboard: AtomicBool::new(true),
            mouse: AtomicBool::new(true),
            gamepad: AtomicBool::new(true),
            controller_slot,
            kick: AtomicBool::new(false),
            stats: Mutex::new(ViewerStats {
                ice_path,
                ..ViewerStats::default()
            }),
        });
        self.peers.lock().expect("host peers poisoned").push(state);
        self.streaming.store(true, Ordering::SeqCst);
        self.emit();
    }

    fn peer(&self, peer_id: &str) -> Option<Arc<PeerState>> {
        self.peers
            .lock()
            .expect("host peers poisoned")
            .iter()
            .find(|peer| peer.peer_id == peer_id)
            .cloned()
    }

    fn remove_peer(&self, peer_id: &str) {
        {
            let mut peers = self.peers.lock().expect("host peers poisoned");
            peers.retain(|peer| peer.peer_id != peer_id);
            self.streaming.store(!peers.is_empty(), Ordering::SeqCst);
        }
        self.emit();
    }

    fn update_viewer_stats(&self, peer_id: &str, stats: ViewerStats) {
        if let Some(peer) = self.peer(peer_id) {
            *peer.stats.lock().expect("viewer stats poisoned") = stats;
        }
        self.emit();
    }

    fn set_peer_permissions(
        &self,
        peer_id: &str,
        keyboard: bool,
        mouse: bool,
        gamepad: bool,
    ) -> Result<(), String> {
        let peer = self
            .peer(peer_id)
            .ok_or_else(|| "That viewer is no longer connected.".to_string())?;
        peer.keyboard.store(keyboard, Ordering::SeqCst);
        peer.mouse.store(mouse, Ordering::SeqCst);
        peer.gamepad.store(gamepad, Ordering::SeqCst);
        self.emit();
        Ok(())
    }

    fn kick_peer(&self, peer_id: &str) -> Result<(), String> {
        let peer = self
            .peer(peer_id)
            .ok_or_else(|| "That viewer is no longer connected.".to_string())?;
        peer.kick.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn emit(&self) {
        let _ = self.app.emit(HOST_STATUS_EVENT, self.status());
    }

    fn rotate_code(&self) -> String {
        let code = generate_pair_code();
        *self.code.lock().expect("host code poisoned") = code.clone();
        code
    }

    fn set_error(&self, message: impl Into<String>) {
        *self.last_error.lock().expect("host error poisoned") = Some(message.into());
    }

    fn clear_error(&self) {
        *self.last_error.lock().expect("host error poisoned") = None;
    }

    fn set_gamepad_error(&self, message: Option<String>) {
        *self
            .gamepad_error
            .lock()
            .expect("host gamepad error poisoned") = message;
    }
}

struct RelayWorker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

struct ActiveHost {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<HostShared>,
    relay: Mutex<Option<RelayWorker>>,
    // Only kept alive so Bonjour stays registered for the host's lifetime.
    #[allow(dead_code)]
    advertisement: Option<DiscoveryAdvertisement>,
}

impl ActiveHost {
    fn stop_relay(&self) {
        if let Some(worker) = self.relay.lock().expect("host relay poisoned").take() {
            worker.stop.store(true, Ordering::SeqCst);
            if let Some(handle) = worker.thread {
                let _ = handle.join();
            }
        }
    }
}

#[derive(Default)]
pub struct NativeHost {
    active: Mutex<Option<ActiveHost>>,
}

impl NativeHost {
    pub fn status(&self) -> HostStatus {
        let active = self.active.lock().expect("host state poisoned");
        match active.as_ref() {
            Some(host) => host.shared.status(),
            None => HostStatus {
                running: false,
                streaming: false,
                allow_input: false,
                gamepad: false,
                gamepad_error: None,
                require_approval: true,
                pending: Vec::new(),
                viewers: Vec::new(),
                summary: None,
                last_error: None,
            },
        }
    }

    fn take(&self) -> Option<ActiveHost> {
        self.active.lock().expect("host state poisoned").take()
    }

    pub fn shutdown(&self) {
        if let Some(host) = self.take() {
            host.stop.store(true, Ordering::SeqCst);
            host.shared.running.store(false, Ordering::SeqCst);
            host.stop_relay();
            if let Some(handle) = host.thread {
                let _ = handle.join();
            }
            host.shared.emit();
        }
    }
}

pub(crate) fn local_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "Nexus host".to_string())
}

/// Best-effort LAN address: opening a UDP socket toward a TEST-NET address makes
/// the kernel select the outbound interface without sending a packet.
fn lan_address() -> String {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            socket.connect("192.0.2.1:9")?;
            socket.local_addr()
        })
        .map(|address| address.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string())
}

#[tauri::command(async)]
pub fn native_host_start(
    app: AppHandle,
    host: State<'_, NativeHost>,
    input: State<'_, NativeInput>,
    options: Option<CaptureOptions>,
    port: Option<u16>,
    code: Option<String>,
    require_approval: Option<bool>,
    adaptive_bitrate: Option<bool>,
    relay_address: Option<String>,
    ice: Option<IceSettings>,
    media_port: Option<u16>,
    gamepad: Option<bool>,
) -> Result<HostStatus, String> {
    #[cfg(target_os = "macos")]
    {
        let options = options.unwrap_or_default().normalized()?;
        let gamepad = gamepad.unwrap_or(true);
        // Prefer OpenJoystickDriver: it is already signed with the virtual-HID
        // entitlement and needs no SIP/AMFI changes. Only probe the in-process
        // HID backend when OJD is not configured.
        let ojd_config = gamepad.then(ojd::discover).flatten();
        let gamepad_error = if gamepad && ojd_config.is_none() {
            match VirtualGamepad::new(0) {
                Ok(device) => {
                    drop(device);
                    None
                }
                Err(error) => Some(error),
            }
        } else {
            None
        };
        let ice = ice.unwrap_or_default().normalized()?;
        let media_port = media_port.unwrap_or(39201);
        if media_port != 0 && media_port < 1024 {
            return Err("The media UDP port must be 0 (automatic) or at least 1024.".to_string());
        }
        let code = match code {
            Some(value) => {
                let normalized = normalize_pair_code(&value)?;
                format!("{}-{}", &normalized[..4], &normalized[4..])
            }
            None => generate_pair_code(),
        };
        let port = port.unwrap_or(DEFAULT_PORT);
        let adaptive = adaptive_bitrate.unwrap_or(true);
        let relay_target = relay_address
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(RelayTarget::parse)
            .transpose()?;

        host.shutdown();
        input.release_all();

        let listener = TcpListener::bind(("0.0.0.0", port))
            .map_err(|error| format!("Could not listen on port {port}: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("Could not configure the host listener: {error}"))?;
        let local_port = listener
            .local_addr()
            .map(|address| address.port())
            .unwrap_or(port);

        let advertisement = DiscoveryAdvertisement::start(local_port).ok();
        let nearby = advertisement.is_some();
        let relay = relay_target.as_ref().map(|target| RelayRuntime {
            target: target.clone(),
            room: reachability::generate_room(),
            state: RelayState::Connecting,
            error: None,
        });

        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(HostShared {
            app: app.clone(),
            address: lan_address(),
            port: local_port,
            started_at: now_ms(),
            code: Mutex::new(code),
            allow_input: AtomicBool::new(false),
            allow_gamepad: AtomicBool::new(gamepad),
            gamepad_error: Mutex::new(gamepad_error),
            ojd_config: ojd_config.clone(),
            running: AtomicBool::new(true),
            streaming: AtomicBool::new(false),
            require_approval: AtomicBool::new(require_approval.unwrap_or(true)),
            nearby: AtomicBool::new(nearby),
            session: Mutex::new(()),
            relay: Mutex::new(relay),
            pending: Mutex::new(Vec::new()),
            peers: Mutex::new(Vec::new()),
            peer_seq: std::sync::atomic::AtomicU64::new(0),
            last_error: Mutex::new(None),
            ice,
            media_port,
        });
        let relay_options = options.clone();
        let relay_input = input.inner().clone();
        let thread_shared = Arc::clone(&shared);
        let thread_stop = Arc::clone(&stop);
        let input = input.inner().clone();
        let (ready_tx, ready_rx) = mpsc::channel();

        let handle = thread::Builder::new()
            .name("nexus-host".to_string())
            .spawn(move || {
                run_host(
                    listener,
                    options,
                    adaptive,
                    input,
                    thread_shared,
                    thread_stop,
                    ready_tx,
                );
            })
            .map_err(|error| format!("Could not start the host thread: {error}"))?;

        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => {
                let relay_worker = match relay_target {
                    Some(target) => start_relay_worker(
                        target,
                        relay_options,
                        adaptive,
                        relay_input,
                        &shared,
                        &stop,
                    ),
                    None => None,
                };
                let status = shared.status();
                *host.active.lock().expect("host state poisoned") = Some(ActiveHost {
                    stop,
                    thread: Some(handle),
                    shared,
                    relay: Mutex::new(relay_worker),
                    advertisement,
                });
                let _ = app.emit(HOST_STATUS_EVENT, &status);
                Ok(status)
            }
            Ok(Err(error)) => {
                let _ = handle.join();
                Err(error)
            }
            Err(_) => {
                stop.store(true, Ordering::SeqCst);
                let _ = handle.join();
                Err("Timed out while starting the host listener.".to_string())
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (
            app,
            host,
            input,
            options,
            port,
            code,
            require_approval,
            adaptive_bitrate,
            relay_address,
            ice,
            media_port,
        );
        Err("Direct hosting is only supported on macOS.".to_string())
    }
}

#[tauri::command(async)]
pub fn native_host_stop(
    app: AppHandle,
    host: State<'_, NativeHost>,
    input: State<'_, NativeInput>,
) -> Result<HostStatus, String> {
    input.release_all();
    if let Some(active) = host.take() {
        active.stop.store(true, Ordering::SeqCst);
        active.shared.running.store(false, Ordering::SeqCst);
        active.stop_relay();
        if let Some(handle) = active.thread {
            let _ = handle.join();
        }
        active.shared.emit();
    }
    let status = host.status();
    let _ = app.emit(HOST_STATUS_EVENT, &status);
    Ok(status)
}

#[tauri::command]
pub fn native_host_status(host: State<'_, NativeHost>) -> HostStatus {
    host.status()
}

#[tauri::command]
pub fn native_host_rotate_code(
    app: AppHandle,
    host: State<'_, NativeHost>,
) -> Result<HostStatus, String> {
    let active = host.active.lock().expect("host state poisoned");
    let Some(active) = active.as_ref() else {
        return Err("Start hosting before changing the pairing code.".to_string());
    };
    active.shared.rotate_code();
    active.shared.clear_error();
    let status = active.shared.status();
    let _ = app.emit(HOST_STATUS_EVENT, &status);
    Ok(status)
}

#[tauri::command]
pub fn native_host_set_input(
    app: AppHandle,
    host: State<'_, NativeHost>,
    input: State<'_, NativeInput>,
    allow: bool,
) -> Result<HostStatus, String> {
    let input_status = input.set_armed(allow)?;
    let active = host.active.lock().expect("host state poisoned");
    let Some(active) = active.as_ref() else {
        if allow {
            input.set_armed(false)?;
        }
        return Err("Start hosting before enabling remote input.".to_string());
    };
    active
        .shared
        .allow_input
        .store(allow && input_status.armed, Ordering::SeqCst);
    active.shared.clear_error();
    let status = active.shared.status();
    let _ = app.emit(HOST_STATUS_EVENT, &status);
    Ok(status)
}

#[tauri::command]
pub fn native_host_set_gamepad(
    app: AppHandle,
    host: State<'_, NativeHost>,
    allow: bool,
) -> Result<HostStatus, String> {
    let active = host.active.lock().expect("host state poisoned");
    let Some(active) = active.as_ref() else {
        return Err("Start hosting before changing controller passthrough.".to_string());
    };
    active.shared.allow_gamepad.store(allow, Ordering::SeqCst);
    if allow {
        // OJD is already available; otherwise re-probe so a build that gained
        // the entitlement starts working without restarting the host.
        if active.shared.ojd_config.is_some() {
            active.shared.set_gamepad_error(None);
        } else {
            match VirtualGamepad::new(0) {
                Ok(device) => {
                    drop(device);
                    active.shared.set_gamepad_error(None);
                }
                Err(error) => active.shared.set_gamepad_error(Some(error)),
            }
        }
    } else {
        active.shared.set_gamepad_error(None);
    }
    let status = active.shared.status();
    let _ = app.emit(HOST_STATUS_EVENT, &status);
    Ok(status)
}

#[tauri::command]
pub fn native_host_set_approval(
    app: AppHandle,
    host: State<'_, NativeHost>,
    require: bool,
) -> Result<HostStatus, String> {
    let active = host.active.lock().expect("host state poisoned");
    let Some(active) = active.as_ref() else {
        return Err("Start hosting before changing approval.".to_string());
    };
    active
        .shared
        .require_approval
        .store(require, Ordering::SeqCst);
    active.shared.clear_error();
    let status = active.shared.status();
    let _ = app.emit(HOST_STATUS_EVENT, &status);
    Ok(status)
}

#[tauri::command]
pub fn native_host_respond(
    app: AppHandle,
    host: State<'_, NativeHost>,
    peer_id: String,
    approve: bool,
) -> Result<HostStatus, String> {
    let active = host.active.lock().expect("host state poisoned");
    let Some(active) = active.as_ref() else {
        return Err("No connection request is waiting.".to_string());
    };
    active.shared.resolve_pending(&peer_id, approve)?;
    let status = active.shared.status();
    let _ = app.emit(HOST_STATUS_EVENT, &status);
    Ok(status)
}

#[tauri::command]
pub fn native_host_set_peer_permissions(
    app: AppHandle,
    host: State<'_, NativeHost>,
    peer_id: String,
    keyboard: bool,
    mouse: bool,
    gamepad: bool,
) -> Result<HostStatus, String> {
    let active = host.active.lock().expect("host state poisoned");
    let Some(active) = active.as_ref() else {
        return Err("Start hosting before changing viewer access.".to_string());
    };
    active
        .shared
        .set_peer_permissions(&peer_id, keyboard, mouse, gamepad)?;
    let status = active.shared.status();
    let _ = app.emit(HOST_STATUS_EVENT, &status);
    Ok(status)
}

#[tauri::command]
pub fn native_host_kick(
    app: AppHandle,
    host: State<'_, NativeHost>,
    peer_id: String,
) -> Result<HostStatus, String> {
    let active = host.active.lock().expect("host state poisoned");
    let Some(active) = active.as_ref() else {
        return Err("Start hosting before disconnecting a viewer.".to_string());
    };
    active.shared.kick_peer(&peer_id)?;
    let status = active.shared.status();
    let _ = app.emit(HOST_STATUS_EVENT, &status);
    Ok(status)
}

#[cfg(target_os = "macos")]
fn run_host(
    listener: TcpListener,
    options: CaptureOptions,
    adaptive: bool,
    input: NativeInput,
    shared: Arc<HostShared>,
    stop: Arc<AtomicBool>,
    ready: mpsc::Sender<Result<(), String>>,
) {
    if ready.send(Ok(())).is_err() {
        shared.running.store(false, Ordering::SeqCst);
        return;
    }
    shared.emit();

    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, peer)) => {
                let busy = shared.peers.lock().expect("host peers poisoned").len()
                    + shared.pending.lock().expect("host pending poisoned").len();
                if busy >= MAX_VIEWERS {
                    let _ = stream.shutdown(Shutdown::Both);
                    continue;
                }
                let options = options.clone();
                let session_input = input.clone();
                let session_shared = Arc::clone(&shared);
                let session_stop = Arc::clone(&stop);
                let peer = peer.to_string();
                if let Err(error) = thread::Builder::new()
                    .name("nexus-viewer".to_string())
                    .spawn(move || {
                        serve_client(
                            &options,
                            adaptive,
                            &session_input,
                            &session_shared,
                            &session_stop,
                            stream,
                            peer,
                            true,
                        );
                    })
                {
                    shared.set_error(format!("Could not start a viewer session: {error}"));
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => {
                if !stop.load(Ordering::SeqCst) {
                    shared.set_error(format!("The host listener stopped: {error}"));
                }
                break;
            }
        }
    }

    input.release_all();
    shared.allow_input.store(false, Ordering::SeqCst);
    shared.streaming.store(false, Ordering::SeqCst);
    shared.clear_pending();
    shared.peers.lock().expect("host peers poisoned").clear();
    shared.running.store(false, Ordering::SeqCst);
    shared.emit();
}

#[cfg(target_os = "macos")]
fn start_relay_worker(
    target: RelayTarget,
    options: CaptureOptions,
    adaptive: bool,
    input: NativeInput,
    shared: &Arc<HostShared>,
    host_stop: &Arc<AtomicBool>,
) -> Option<RelayWorker> {
    let relay_stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&relay_stop);
    let thread_host_stop = Arc::clone(host_stop);
    let thread_shared = Arc::clone(shared);
    match thread::Builder::new()
        .name("nexus-relay".to_string())
        .spawn(move || {
            run_relay_host(
                target,
                options,
                adaptive,
                input,
                thread_shared,
                thread_host_stop,
                thread_stop,
            );
        }) {
        Ok(thread) => Some(RelayWorker {
            stop: relay_stop,
            thread: Some(thread),
        }),
        Err(error) => {
            shared.set_relay_state(
                RelayState::Offline,
                Some(format!("Could not start the relay worker: {error}")),
            );
            None
        }
    }
}

/// Keeps one rendezvous room open for as long as hosting runs. A relay session
/// still uses the same Noise handshake and approval gate as a direct session;
/// the relay only forwards the encrypted bytes.
#[cfg(target_os = "macos")]
fn run_relay_host(
    target: RelayTarget,
    options: CaptureOptions,
    adaptive: bool,
    input: NativeInput,
    shared: Arc<HostShared>,
    stop: Arc<AtomicBool>,
    relay_stop: Arc<AtomicBool>,
) {
    let mut room = match shared.relay.lock().expect("host relay poisoned").as_ref() {
        Some(runtime) => runtime.room.clone(),
        None => return,
    };

    while !stop.load(Ordering::SeqCst) && !relay_stop.load(Ordering::SeqCst) {
        let stopped = || stop.load(Ordering::SeqCst) || relay_stop.load(Ordering::SeqCst);
        shared.set_relay_state(RelayState::Connecting, None);
        let retry = match reachability::open_relay(&target, RelayRole::Host, &room) {
            Ok(mut stream) => {
                shared.set_relay_state(RelayState::Waiting, None);
                match reachability::wait_for_relay_pairing(&mut stream, &stopped) {
                    Ok(()) => {
                        shared.set_relay_state(RelayState::Paired, None);
                        let session = shared.session.lock().expect("host session poisoned");
                        if stopped() {
                            let _ = stream.shutdown(Shutdown::Both);
                            break;
                        }
                        serve_client(
                            &options,
                            adaptive,
                            &input,
                            &shared,
                            &stop,
                            stream,
                            format!("relay:{target}"),
                            false,
                        );
                        drop(session);
                        false
                    }
                    Err(message) => {
                        let _ = stream.shutdown(Shutdown::Both);
                        if stopped() {
                            break;
                        }
                        shared.set_relay_state(RelayState::Offline, Some(message));
                        true
                    }
                }
            }
            Err(message) => {
                if stopped() {
                    break;
                }
                if message.to_ascii_lowercase().contains("already waiting") {
                    if let Some(rotated) = shared.rotate_relay_room() {
                        room = rotated;
                    }
                }
                shared.set_relay_state(RelayState::Offline, Some(message));
                true
            }
        };
        if retry {
            wait_for_relay_retry(&stop, &relay_stop);
        }
    }

    if shared.running.load(Ordering::SeqCst) {
        shared.set_relay_state(RelayState::Offline, None);
    }
}

#[cfg(target_os = "macos")]
fn wait_for_relay_retry(stop: &AtomicBool, relay_stop: &AtomicBool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if stop.load(Ordering::SeqCst) || relay_stop.load(Ordering::SeqCst) {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(target_os = "macos")]
fn stream_rate(rate: &AdaptiveRate) -> Control {
    Control::StreamRate {
        bitrate_mbps: rate.current_mbps(),
        ceiling_mbps: rate.ceiling_mbps(),
        adaptive: rate.enabled(),
    }
}

/// Handles the two-signal SDP exchange that runs over the encrypted TCP
/// channel before the session picks a transport. Returns false for controls
/// that belong to the streaming protocol instead.
#[cfg(target_os = "macos")]
fn handle_signaling(
    control: Control,
    channel: &mut SecureChannel,
    endpoint: &mut Option<WebRtcEndpoint>,
    choice: &mut Option<bool>,
    ceiling_mbps: f64,
    adaptive: bool,
    ice: &IceSettings,
    media_port: u16,
) -> bool {
    match control {
        Control::TransportOffer { sdp } => {
            if endpoint.is_some() {
                let _ = channel.send_control(&Control::TransportFallback {
                    reason: "The host already negotiated a WebRTC transport.".to_string(),
                });
            } else {
                match WebRtcEndpoint::answer_planned(&sdp, ceiling_mbps, adaptive, ice, media_port)
                {
                    Ok((candidate, answer)) => {
                        if channel
                            .send_control(&Control::TransportAnswer { sdp: answer })
                            .is_ok()
                        {
                            *endpoint = Some(candidate);
                        }
                    }
                    Err(reason) => {
                        let _ = channel.send_control(&Control::TransportFallback { reason });
                    }
                }
            }
            true
        }
        Control::TransportChoice { webrtc } => {
            *choice = Some(webrtc);
            true
        }
        _ => false,
    }
}

/// Waits for signaling messages on the encrypted TCP channel for up to
/// `timeout`, returning false when the client goes away.
#[cfg(target_os = "macos")]
fn pump_signaling(
    channel: &mut SecureChannel,
    endpoint: &mut Option<WebRtcEndpoint>,
    choice: &mut Option<bool>,
    timeout: Duration,
    ceiling_mbps: f64,
    adaptive: bool,
    ice: &IceSettings,
    media_port: u16,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline && choice.is_none() {
        let received = channel.poll_recv();
        match received {
            Ok(Some(message)) => match transport::parse_control::<Control>(&message) {
                Ok(Some(Control::Bye)) => return false,
                Ok(Some(control)) => {
                    handle_signaling(
                        control,
                        channel,
                        endpoint,
                        choice,
                        ceiling_mbps,
                        adaptive,
                        ice,
                        media_port,
                    );
                }
                _ => {}
            },
            Ok(None) => thread::sleep(APPROVAL_POLL),
            Err(error) if error.kind() == ErrorKind::WouldBlock => thread::sleep(APPROVAL_POLL),
            Err(_) => return false,
        }
    }
    true
}

#[cfg(target_os = "macos")]
fn serve_client(
    options: &CaptureOptions,
    adaptive: bool,
    input: &NativeInput,
    shared: &Arc<HostShared>,
    stop: &Arc<AtomicBool>,
    mut stream: TcpStream,
    peer: String,
    direct: bool,
) {
    let _ = stream.set_nonblocking(false);
    let code = shared.code.lock().expect("host code poisoned").clone();

    let noise = match transport::handshake_responder(&mut stream, &code) {
        Ok(noise) => noise,
        Err(_) => {
            shared.set_error("A device could not pair. Check the code and try again.".to_string());
            shared.emit();
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
    };

    let mut channel = match SecureChannel::new(stream, noise) {
        Ok(channel) => channel,
        Err(_) => return,
    };

    let hello = match channel.recv_timeout(HELLO_WAIT) {
        Ok(Some(message)) => transport::parse_control::<Control>(&message).ok().flatten(),
        _ => None,
    };
    let (device, owner) = match hello {
        Some(Control::Hello {
            device_name,
            owner_name,
            ..
        }) => {
            let device = device_name.trim();
            let owner = owner_name.trim();
            (
                if device.is_empty() {
                    "Unknown device".to_string()
                } else {
                    device.to_string()
                },
                owner.to_string(),
            )
        }
        _ => {
            let _ = channel.send_control(&Control::Error {
                message: "The remote client did not complete the session handshake.".to_string(),
            });
            channel.shutdown();
            return;
        }
    };

    // Capture needs Screen Recording before a session can start. Checking here
    // turns a silent post-approval session close into an actionable message for
    // both sides, which matters after every rebuild because ad-hoc signing
    // invalidates the old TCC grant.
    #[cfg(target_os = "macos")]
    if unsafe { crate::mac_permissions::CGPreflightScreenCaptureAccess() } == 0 {
        let message = "The host needs Screen Recording permission. Open System Settings → \
                       Privacy & Security → Screen Recording, enable Nexus, relaunch the \
                       app, then start hosting again.";
        shared.set_error(message.to_string());
        shared.emit();
        let _ = channel.send_control(&Control::Error {
            message: message.to_string(),
        });
        channel.shutdown();
        return;
    }

    let mut webrtc_endpoint: Option<WebRtcEndpoint> = None;
    let mut webrtc_choice: Option<bool> = None;
    let ceiling_mbps = f64::from(options.bitrate_mbps);
    let peer_id = shared.next_peer_id();

    if shared.require_approval.load(Ordering::SeqCst) {
        // Requests stay in the presence dock until the host decides; the client
        // waits longer than APPROVAL_WAIT so the host never has to rush.
        let decision_rx =
            shared.register_pending(peer_id.clone(), peer.clone(), device.clone(), owner.clone());

        let deadline = Instant::now() + APPROVAL_WAIT;
        let mut decision: Option<bool> = None;
        let mut client_gone = false;
        while !stop.load(Ordering::SeqCst) && Instant::now() < deadline {
            if !shared.require_approval.load(Ordering::SeqCst) {
                decision = Some(true);
                break;
            }
            match decision_rx.try_recv() {
                Ok(value) => {
                    decision = Some(value);
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
            let received = channel.poll_recv();
            match received {
                Ok(Some(message)) => match transport::parse_control::<Control>(&message) {
                    Ok(Some(Control::Bye)) => {
                        client_gone = true;
                        break;
                    }
                    Ok(Some(control)) => {
                        handle_signaling(
                            control,
                            &mut channel,
                            &mut webrtc_endpoint,
                            &mut webrtc_choice,
                            ceiling_mbps,
                            adaptive,
                            &shared.ice,
                            shared.media_port,
                        );
                    }
                    _ => {}
                },
                Ok(None) => thread::sleep(APPROVAL_POLL),
                Err(error) if error.kind() == ErrorKind::WouldBlock => thread::sleep(APPROVAL_POLL),
                Err(_) => {
                    client_gone = true;
                    break;
                }
            }
        }
        if decision.is_none() {
            if let Ok(value) = decision_rx.try_recv() {
                decision = Some(value);
            }
        }

        shared.drop_pending(&peer_id);

        if client_gone || stop.load(Ordering::SeqCst) {
            channel.shutdown();
            return;
        }
        if decision != Some(true) {
            let message = if decision == Some(false) {
                "The host declined this connection."
            } else {
                "The host did not approve this connection in time."
            };
            let _ = channel.send_control(&Control::Error {
                message: message.to_string(),
            });
            channel.shutdown();
            shared.emit();
            return;
        }
    }

    if direct
        && webrtc_choice.is_none()
        && !pump_signaling(
            &mut channel,
            &mut webrtc_endpoint,
            &mut webrtc_choice,
            TRANSPORT_WAIT,
            ceiling_mbps,
            adaptive,
            &shared.ice,
            shared.media_port,
        )
    {
        channel.shutdown();
        return;
    }

    let mut webrtc_channel: Option<WebRtcChannel> = None;
    if webrtc_choice == Some(true) {
        match webrtc_endpoint.take() {
            Some(endpoint) => match endpoint.connect(WEBRTC_CONNECT_TIMEOUT) {
                Ok(channel) => webrtc_channel = Some(channel),
                Err(reason) => {
                    let _ = channel.send_control(&Control::TransportFallback { reason });
                }
            },
            None => {
                let _ = channel.send_control(&Control::TransportFallback {
                    reason: "The host did not negotiate a WebRTC transport.".to_string(),
                });
            }
        }
    }
    drop(webrtc_endpoint);
    let ice_path = webrtc_channel
        .as_ref()
        .and_then(|channel| channel.ice_path());

    let mut session = match webrtc_channel {
        Some(channel) => SessionChannel::WebRtc(channel),
        None => SessionChannel::Tcp(channel),
    };

    let mut pipeline = match CapturePipeline::open(options) {
        Ok(pipeline) => pipeline,
        Err(error) => {
            shared.set_error(error.clone());
            let _ = session.send_control(&Control::Error { message: error });
            shared.emit();
            session.shutdown();
            return;
        }
    };
    pipeline.request_keyframe();
    // The settings the live pipeline was opened with. `RequestDisplay` builds
    // the next pipeline from these, keeps the session's width, and swaps both
    // so the frame-size bookkeeping stays in step.
    let mut capture_options = options.clone();
    // Advertised once per connection; the client's display picker comes from
    // this list and its indices are what `RequestDisplay` sends back.
    let displays: Vec<DisplayOption> =
        display_options(crate::display_info::displays().unwrap_or_default());

    let allow_input = shared.allow_input.load(Ordering::SeqCst);
    if let Err(error) = session.send_control(&Control::Welcome {
        width: capture_options.width,
        height: capture_options.height,
        allow_input,
        host_name: local_device_name(),
        displays: displays.clone(),
    }) {
        shared.set_error(format!("Could not greet the remote device: {error}"));
        shared.emit();
        session.shutdown();
        return;
    }

    let mut rate = AdaptiveRate::new(adaptive, ceiling_mbps);
    let webrtc = session.is_webrtc();
    let webrtc_link = session.link_estimate();
    let mut encoder_mbps = ceiling_mbps;
    if let Some(link) = webrtc_link.as_ref() {
        // Open the encoder at the estimator's opening target rather than at the
        // ceiling, so the first frames do not burst while the plan says 8 Mbps.
        encoder_mbps = encoder_mbps_for(link.target_bps(), ceiling_mbps);
        let _ = pipeline.set_bitrate_mbps(encoder_mbps);
    }
    let initial_rate = if webrtc {
        Control::StreamRate {
            bitrate_mbps: encoder_mbps,
            ceiling_mbps,
            adaptive,
        }
    } else {
        stream_rate(&rate)
    };
    if let Err(error) = session.send_control(&initial_rate) {
        shared.set_error(format!("Could not publish the stream rate: {error}"));
        shared.emit();
        session.shutdown();
        return;
    }

    shared.clear_error();
    let transport_label = if session.is_webrtc() { "webrtc" } else { "tcp" };
    shared.add_peer(
        peer_id.clone(),
        peer.clone(),
        device.clone(),
        owner.clone(),
        transport_label,
        ice_path.clone(),
    );
    // The invite code stays valid for one-click reconnects. Users can rotate it
    // manually from the Remote access card when they want to revoke access.
    shared.emit();

    let started = Instant::now();
    let mut stats_started = Instant::now();
    let mut stats_frames = 0u64;
    let mut stats_bytes = 0u64;
    let mut sent_since_feedback = 0u32;
    let mut last_delivery = 1.0f64;
    let mut last_jitter = 0.0f64;
    let mut rtt_baseline: Option<f64> = None;
    let mut last_latency_cut: Option<Instant> = None;
    let mut last_backpressure: Option<Instant> = None;
    let mut last_input_state = allow_input;
    // Created lazily on the first controller snapshot and dropped with the
    // session, so a viewer that never uses a controller costs nothing.
    let mut virtual_gamepad: Option<VirtualGamepad> = None;
    let mut ojd_pad: Option<OjdPad> = None;
    let controller_slot = shared.peer(&peer_id).map_or(1, |peer| peer.controller_slot);
    let mut clipboard_change = if options.clipboard {
        clipboard::change_count()
    } else {
        0
    };
    let mut clipboard_text = if options.clipboard {
        clipboard::read_text().unwrap_or_default()
    } else {
        String::new()
    };
    let mut clipboard_poll = Instant::now();
    let mut failure: Option<String> = None;
    let mut frames = 0u64;
    let mut last_timestamp_ms: Option<u64> = None;
    let mut last_link_update = Instant::now();
    let mut last_webrtc_keyframe: Option<Instant> = None;
    // Clients ping every second and send stream feedback while connected. A
    // silent channel means the viewer vanished without a clean Bye; time it out
    // so the host does not keep a ghost viewer in the presence dock.
    let mut last_client_activity = Instant::now();

    while !stop.load(Ordering::SeqCst) {
        if shared
            .peer(&peer_id)
            .is_some_and(|peer| peer.kick.load(Ordering::SeqCst))
        {
            let _ = session.send_control(&Control::Bye);
            break;
        }
        let allow = shared.allow_input.load(Ordering::SeqCst);
        let viewer_input = shared.peer(&peer_id).is_none_or(|peer| {
            peer.keyboard.load(Ordering::SeqCst)
                || peer.mouse.load(Ordering::SeqCst)
                || peer.gamepad.load(Ordering::SeqCst)
        });
        let effective_input = allow && viewer_input;
        if effective_input != last_input_state {
            last_input_state = effective_input;
            if session
                .send_control(&Control::InputState {
                    enabled: effective_input,
                })
                .is_err()
            {
                break;
            }
        }
        // Release the virtual pad as soon as the host or the viewer revokes
        // controller access, rather than waiting for the next snapshot.
        if virtual_gamepad.is_some()
            && (!shared.allow_gamepad.load(Ordering::SeqCst)
                || !shared.allow_input.load(Ordering::SeqCst)
                || shared
                    .peer(&peer_id)
                    .is_some_and(|peer| !peer.gamepad.load(Ordering::SeqCst)))
        {
            virtual_gamepad = None;
        }

        match pipeline.next(CAPTURE_POLL) {
            Ok(CaptureTick::Frame(captured)) => {
                let encoded = &captured.encoded;
                // The capture timestamp is milliseconds since the epoch, so the
                // first frame uses the configured interval rather than a delta
                // from zero. Later frames use the real inter-frame time.
                let frame_duration = match last_timestamp_ms {
                    Some(previous) => Duration::from_millis(
                        encoded
                            .timestamp_ms
                            .saturating_sub(previous)
                            .clamp(1, 1_000),
                    ),
                    None => Duration::from_millis(
                        (1_000.0 / options.frame_rate).round().clamp(1.0, 1_000.0) as u64,
                    ),
                };
                last_timestamp_ms = Some(encoded.timestamp_ms);
                let send_started = Instant::now();
                let mut sent = 0usize;
                let mut failed = false;
                let mut saturated = false;
                if webrtc {
                    match session.send_video(&encoded.annex_b, frame_duration) {
                        Ok(()) => sent = encoded.annex_b.len(),
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            saturated = true;
                        }
                        Err(error) => {
                            failure =
                                Some(format!("The remote device dropped the session: {error}"));
                            failed = true;
                        }
                    }
                } else {
                    let outgoing = OutgoingFrame {
                        frame_id: encoded.sequence.min(u32::MAX as u64) as u32,
                        keyframe: encoded.is_keyframe,
                        timestamp_ms: encoded.timestamp_ms,
                        width: encoded.width,
                        height: encoded.height,
                        encode_ms: encoded.encode_ms as f32,
                        data: &encoded.annex_b,
                    };
                    let messages = match transport::frame_messages(&outgoing) {
                        Ok(messages) => messages,
                        Err(error) => {
                            failure = Some(error);
                            break;
                        }
                    };
                    for message in &messages {
                        match session.send(message) {
                            Ok(()) => sent += message.len(),
                            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                                saturated = true;
                                break;
                            }
                            Err(error) => {
                                failure =
                                    Some(format!("The remote device dropped the session: {error}"));
                                failed = true;
                                break;
                            }
                        }
                    }
                }
                if failed {
                    break;
                }
                if !saturated {
                    frames += 1;
                    stats_frames += 1;
                }
                // The pacer may still drop a packet after this point and the
                // transport does not report that back, so this counts media
                // enqueued rather than individually confirmed wire bytes.
                stats_bytes += sent as u64;
                sent_since_feedback = sent_since_feedback.saturating_add(1);

                if let Some(link) = webrtc_link.as_ref() {
                    // RTP: the sender-side delay-based estimator owns the
                    // encoder target. It reacts to rising queueing delay before
                    // loss, so the old delivery-ratio controller is not used.
                    if last_link_update.elapsed() >= Duration::from_millis(200) {
                        last_link_update = Instant::now();
                        let target = encoder_mbps_for(link.target_bps(), ceiling_mbps);
                        if (target - encoder_mbps).abs() >= 0.1 {
                            if let Err(error) = pipeline.set_bitrate_mbps(target) {
                                shared.set_error(error);
                            } else {
                                encoder_mbps = target;
                                let _ = session.send_control(&Control::StreamRate {
                                    bitrate_mbps: target,
                                    ceiling_mbps,
                                    adaptive,
                                });
                            }
                        }
                    }
                    if saturated
                        && last_webrtc_keyframe
                            .is_none_or(|last| last.elapsed() >= Duration::from_secs(1))
                    {
                        // The writer is behind, so the frame will be dropped and
                        // the receiver will see a hole. Ask for a fresh
                        // reference instead of waiting for a decode failure.
                        last_webrtc_keyframe = Some(Instant::now());
                        pipeline.request_keyframe();
                    }
                } else {
                    // TCP: blocking writes are the earliest congestion signal.
                    // Client feedback arrives a second later. Throttle cuts to
                    // one per second.
                    let send_ms = send_started.elapsed().as_secs_f64() * 1000.0;
                    let backpressured = (saturated || send_ms > 120.0)
                        && last_backpressure
                            .is_none_or(|last| last.elapsed() >= Duration::from_secs(1));
                    if backpressured && rate.degrade() {
                        last_backpressure = Some(Instant::now());
                        if let Err(error) = pipeline.set_bitrate_mbps(rate.current_mbps()) {
                            shared.set_error(error);
                        } else {
                            let _ = session.send_control(&stream_rate(&rate));
                        }
                    }
                }
            }
            Ok(CaptureTick::Idle) => {}
            Ok(CaptureTick::Stopped) => {
                failure = Some("macOS stopped the host's screen capture stream.".to_string());
                break;
            }
            Err(error) => {
                failure = Some(error);
                break;
            }
        }

        if let Some(pad) = ojd_pad.as_mut() {
            if let Err(error) = pad.keepalive_if_due() {
                shared.set_gamepad_error(Some(error));
                shared.emit();
                ojd_pad = None;
            }
        }

        for _ in 0..MAX_CONTROL_BURST {
            match session.poll_recv() {
                Ok(Some(message)) => {
                    last_client_activity = Instant::now();
                    let control = match transport::parse_control::<Control>(&message) {
                        Ok(Some(control)) => control,
                        _ => continue,
                    };
                    match control {
                        Control::Input { event } => {
                            if !shared.allow_input.load(Ordering::SeqCst) {
                                continue;
                            }
                            let allowed = shared.peer(&peer_id).is_none_or(|peer| match &event {
                                RemoteInput::Pointer { .. }
                                | RemoteInput::Button { .. }
                                | RemoteInput::Scroll { .. } => peer.mouse.load(Ordering::SeqCst),
                                RemoteInput::Key { .. } | RemoteInput::Text { .. } => {
                                    peer.keyboard.load(Ordering::SeqCst)
                                }
                                RemoteInput::Gamepad { .. } => peer.gamepad.load(Ordering::SeqCst),
                            });
                            if !allowed {
                                continue;
                            }
                            if let Err(error) = input.apply(&event) {
                                shared.allow_input.store(false, Ordering::SeqCst);
                                input.set_armed(false).ok();
                                shared.set_error(error);
                                let _ =
                                    session.send_control(&Control::InputState { enabled: false });
                                shared.emit();
                            }
                        }
                        Control::Gamepad { state } => {
                            // Controller passthrough is independent from the
                            // keyboard/mouse consent arm: controllers are the
                            // default remote input, and each viewer has their
                            // own per-viewer switch.
                            if !shared.allow_gamepad.load(Ordering::SeqCst) {
                                virtual_gamepad = None;
                                ojd_pad = None;
                                continue;
                            }
                            let allowed = shared
                                .peer(&peer_id)
                                .is_none_or(|peer| peer.gamepad.load(Ordering::SeqCst));
                            if !allowed {
                                virtual_gamepad = None;
                                ojd_pad = None;
                                continue;
                            }
                            if !state.connected {
                                virtual_gamepad = None;
                                ojd_pad = None;
                                continue;
                            }

                            if let Some(config) = shared.ojd_config.as_ref() {
                                if ojd_pad.is_none() {
                                    match OjdPad::connect(config, controller_slot) {
                                        Ok(pad) => {
                                            shared.set_gamepad_error(None);
                                            ojd_pad = Some(pad);
                                        }
                                        Err(error) => {
                                            shared.set_gamepad_error(Some(error));
                                            shared.emit();
                                            continue;
                                        }
                                    }
                                }
                                if let Some(pad) = ojd_pad.as_mut() {
                                    if let Err(error) = pad.send_state(&state) {
                                        shared.set_gamepad_error(Some(error));
                                        shared.emit();
                                        ojd_pad = None;
                                    }
                                }
                                continue;
                            }

                            if virtual_gamepad.is_none() {
                                match VirtualGamepad::new(controller_slot) {
                                    Ok(device) => {
                                        shared.set_gamepad_error(None);
                                        virtual_gamepad = Some(device);
                                    }
                                    Err(error) => {
                                        shared.set_gamepad_error(Some(error));
                                        shared.emit();
                                        continue;
                                    }
                                }
                            }
                            if let Some(device) = virtual_gamepad.as_ref() {
                                if let Err(error) = device.apply(&state) {
                                    shared.set_gamepad_error(Some(error));
                                    shared.emit();
                                }
                            }
                        }
                        Control::StreamFeedback {
                            received,
                            jitter_ms,
                            rtt_ms,
                            dropped,
                        } => {
                            let jitter = if jitter_ms.is_finite() {
                                jitter_ms.clamp(0.0, 60_000.0)
                            } else {
                                0.0
                            };
                            let sent = std::mem::take(&mut sent_since_feedback);
                            last_jitter = jitter;
                            if sent > 0 {
                                last_delivery =
                                    (f64::from(received) / f64::from(sent)).clamp(0.0, 1.0);
                            }
                            if webrtc_link.is_none() {
                                if rate.observe(received, sent, jitter, dropped) {
                                    if let Err(error) =
                                        pipeline.set_bitrate_mbps(rate.current_mbps())
                                    {
                                        shared.set_error(error);
                                    } else {
                                        let _ = session.send_control(&stream_rate(&rate));
                                    }
                                }
                            } else {
                                // The GCC estimator reacts to queue delay; the
                                // receiver's own RTT/delivery numbers catch a
                                // standing queue sooner on Wi-Fi. Cap the encoder
                                // when the path is demonstrably queuing or losing.
                                let rtt = if rtt_ms.is_finite() {
                                    rtt_ms.clamp(0.0, 60_000.0)
                                } else {
                                    0.0
                                };
                                if rtt > 0.0 {
                                    rtt_baseline = Some(
                                        rtt_baseline.map_or(rtt, |baseline| baseline.min(rtt)),
                                    );
                                }
                                let queued = rtt > 0.0
                                    && rtt_baseline.is_some_and(|baseline| rtt > baseline + 40.0);
                                let losing = sent > 0 && last_delivery < 0.97;
                                let cooled = last_latency_cut
                                    .is_none_or(|last| last.elapsed() >= Duration::from_secs(1));
                                if (queued || losing) && cooled {
                                    let target = (encoder_mbps * 0.85).max(2.0);
                                    if target < encoder_mbps
                                        && pipeline.set_bitrate_mbps(target).is_ok()
                                    {
                                        encoder_mbps = target;
                                        last_latency_cut = Some(Instant::now());
                                        let _ = session.send_control(&Control::StreamRate {
                                            bitrate_mbps: encoder_mbps,
                                            ceiling_mbps,
                                            adaptive,
                                        });
                                    }
                                }
                            }
                        }
                        Control::Clipboard { text } => {
                            if options.clipboard
                                && clipboard::is_forwardable(&text)
                                && text != clipboard_text
                            {
                                clipboard::write_text(&text);
                                clipboard_change = clipboard::change_count();
                                clipboard_text = text;
                            }
                        }
                        Control::Ping { sent_ms } => {
                            let _ = session.send_control(&Control::Pong { sent_ms });
                        }
                        Control::RequestKeyframe => pipeline.request_keyframe(),
                        Control::RequestDisplay { index } => {
                            // Switching is best-effort: an unknown or busy
                            // display must never end a live session.
                            if let Some(display) = display_option(&displays, index) {
                                let next_options =
                                    display_capture_options(&capture_options, display);
                                match CapturePipeline::open(&next_options) {
                                    Ok(mut next_pipeline) => {
                                        next_pipeline.request_keyframe();
                                        // Keep the session's current rate
                                        // instead of the options default.
                                        let target = if webrtc {
                                            encoder_mbps
                                        } else {
                                            rate.current_mbps()
                                        };
                                        let _ = next_pipeline.set_bitrate_mbps(target);
                                        pipeline = next_pipeline;
                                        capture_options = next_options;
                                        // New encoder clock; the next frame
                                        // uses the configured interval.
                                        last_timestamp_ms = None;
                                        let _ = session.send_control(&Control::DisplayChanged {
                                            index,
                                            width: capture_options.width,
                                            height: capture_options.height,
                                        });
                                    }
                                    Err(error) => {
                                        // Keep the old pipeline. Control::Error
                                        // would end the client session, so the
                                        // failure is surfaced on the host only.
                                        shared.set_error(format!(
                                            "Could not switch to display {index}: {error}"
                                        ));
                                        shared.emit();
                                    }
                                }
                            }
                        }
                        Control::Bye => {
                            stop_session_loop(&mut session, shared, input, &peer_id);
                            return;
                        }
                        _ => {}
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    if error.kind() != ErrorKind::WouldBlock {
                        failure = Some(format!("The remote device disconnected: {error}"));
                    }
                    break;
                }
            }
        }

        if options.clipboard && clipboard_poll.elapsed() >= Duration::from_millis(500) {
            clipboard_poll = Instant::now();
            let change = clipboard::change_count();
            if change != clipboard_change {
                clipboard_change = change;
                if let Some(text) = clipboard::read_text() {
                    if clipboard::is_forwardable(&text) && text != clipboard_text {
                        clipboard_text = text.clone();
                        let _ = session.send_control(&Control::Clipboard { text });
                    }
                }
            }
        }

        if last_client_activity.elapsed() > Duration::from_secs(6) {
            failure = Some("The remote device disconnected.".to_string());
            break;
        }

        if stats_started.elapsed() >= Duration::from_secs(1) {
            let seconds = stats_started.elapsed().as_secs_f64().max(0.001);
            shared.update_viewer_stats(
                &peer_id,
                ViewerStats {
                    fps: stats_frames as f64 / seconds,
                    mbps: stats_bytes as f64 * 8.0 / 1_000_000.0 / seconds,
                    frames,
                    uptime_secs: started.elapsed().as_secs(),
                    bitrate_mbps: if webrtc {
                        encoder_mbps
                    } else {
                        rate.current_mbps()
                    },
                    ceiling_mbps: if webrtc {
                        ceiling_mbps
                    } else {
                        rate.ceiling_mbps()
                    },
                    adaptive: if webrtc { adaptive } else { rate.enabled() },
                    delivery_ratio: last_delivery,
                    jitter_ms: last_jitter,
                    ice_path: ice_path.clone(),
                },
            );
            stats_started = Instant::now();
            stats_frames = 0;
            stats_bytes = 0;
        }
    }

    if let Some(error) = &failure {
        shared.set_error(error.clone());
        let _ = session.send_control(&Control::Error {
            message: error.clone(),
        });
    }
    stop_session_loop(&mut session, shared, input, &peer_id);
}

#[cfg(target_os = "macos")]
fn stop_session_loop(
    channel: &mut SessionChannel,
    shared: &Arc<HostShared>,
    input: &NativeInput,
    peer_id: &str,
) {
    if shared.allow_input.load(Ordering::SeqCst) {
        input.release_all();
        shared.allow_input.store(false, Ordering::SeqCst);
    }
    channel.shutdown();
    shared.remove_peer(peer_id);
}

/// Maps the shared display list into the wire form advertised in `Welcome`.
fn display_options(displays: Vec<crate::display_info::DisplayInfo>) -> Vec<DisplayOption> {
    displays
        .into_iter()
        .map(|display| DisplayOption {
            index: display.index,
            label: display.label,
            width: display.width,
            height: display.height,
        })
        .collect()
}

/// The advertised display with `index`, if the host still has it.
fn display_option(displays: &[DisplayOption], index: usize) -> Option<&DisplayOption> {
    displays.iter().find(|display| display.index == index)
}

/// Capture settings for switching to `display`.
///
/// The session keeps its width so the client's window does not change shape
/// mid-stream; the height is derived from the display's aspect ratio and
/// rounded down to an even number, which is what HEVC requires.
fn display_capture_options(current: &CaptureOptions, display: &DisplayOption) -> CaptureOptions {
    let mut options = current.clone();
    options.display_index = Some(display.index);
    let width = if current.width == 0 {
        1920
    } else {
        current.width
    };
    let height = if display.width == 0 {
        current.height
    } else {
        ((f64::from(width) * f64::from(display.height) / f64::from(display.width)).round() as u32)
            .clamp(240, 4320)
    };
    // HEVC needs even dimensions; `CaptureOptions::normalized` applies the
    // same rounding to caller-supplied presets.
    options.width = (width.clamp(320, 7680) / 2) * 2;
    options.height = (height / 2) * 2;
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(index: usize, width: u32, height: u32) -> DisplayOption {
        DisplayOption {
            index,
            label: format!("display {index}"),
            width,
            height,
        }
    }

    #[test]
    fn display_switch_keeps_width_and_derives_an_even_height() {
        let current = CaptureOptions::default();
        let switched = display_capture_options(&current, &display(1, 1710, 1107));
        assert_eq!(switched.display_index, Some(1));
        // 1920 * 1107 / 1710 = 1243, rounded down to 1242 for HEVC.
        assert_eq!((switched.width, switched.height), (1920, 1242));
        // Session-wide settings carry over untouched.
        assert_eq!(switched.frame_rate, current.frame_rate);
        assert_eq!(switched.bitrate_mbps, current.bitrate_mbps);
        assert_eq!(switched.shows_cursor, current.shows_cursor);
        assert_eq!(switched.clipboard, current.clipboard);
    }

    #[test]
    fn display_switch_gives_a_square_display_a_square_even_size() {
        let current = CaptureOptions::default();
        let switched = display_capture_options(&current, &display(2, 1000, 1000));
        assert_eq!((switched.width, switched.height), (1920, 1920));
        assert_eq!(switched.height % 2, 0);
    }

    #[test]
    fn display_switch_tolerates_an_empty_display_size() {
        let current = CaptureOptions::default();
        let switched = display_capture_options(&current, &display(3, 0, 0));
        assert_eq!(switched.height, current.height);
        assert_eq!(switched.width, current.width);
    }

    #[test]
    fn unknown_display_indices_are_ignored() {
        let displays = vec![display(1, 1710, 1107)];
        assert_eq!(
            display_option(&displays, 1).map(|display| display.index),
            Some(1)
        );
        assert!(display_option(&displays, 4).is_none());
    }

    #[test]
    fn maps_display_infos_into_wire_options() {
        let mapped = display_options(vec![crate::display_info::DisplayInfo {
            index: 1,
            label: "display 1".to_string(),
            width: 1710,
            height: 1107,
        }]);
        assert_eq!(mapped, vec![display(1, 1710, 1107)]);
    }
}
