//! WebRTC session transport: a control data channel plus an RTP video track.
//!
//! Video used to ride an unordered SCTP data channel. That works on a LAN, but
//! SCTP still runs loss-based congestion control and a send buffer, so a real
//! internet path turns packet loss into queueing delay behind the frames that
//! follow — the buffer bloat this module exists to remove. Media now travels on
//! an encrypted RTP track instead, where the parts WebRTC already ships for
//! real-time video apply: transport-wide congestion control with delay-based
//! rate estimation, a pacer that meters packets, NACK for retransmission and
//! FlexFEC for repair without retransmission.
//!
//! The control channel stays SCTP because control messages are small, reliable
//! and latency-tolerant compared with a frame. Nothing else changed in the
//! session protocol: SDP still travels inside the Noise-authenticated TCP
//! channel, so only a peer holding the pairing code can inject a candidate or
//! a DTLS fingerprint.
//!
//! Roles are fixed by the existing negotiation. The client offers and receives
//! video (`recvonly`), the host answers and sends it. That makes the client the
//! TWCC reporter and the host the bandwidth estimator, which is the side that
//! needs the estimate to size the encoder.

use crate::congestion::{wire_bps_for, LinkEstimate, ScreenEstimator, PACING_FACTOR};
use crate::rtp_video::RtpVideoPacket;
use crate::transport::MESSAGE_CONTROL;
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use rand_core::{OsRng, RngCore};
use rtc::interceptor::{
    FlexFec03ReceiveBuilder, FlexFec03SendBuilder, PacerBuilder, Registry, Slot,
};
use rtc::media::Sample;
use rtc::media_stream::MediaStreamTrack;
use rtc::peer_connection::configuration::interceptor_registry::{
    configure_congestion_control, configure_nack, configure_rtcp_reports,
    configure_twcc_receiver_only, CongestionFeedback,
};
use rtc::peer_connection::configuration::media_engine::{
    MediaEngine, MIME_TYPE_FLEX_FEC03, MIME_TYPE_HEVC,
};
use rtc::peer_connection::transport::RTCIceServer;
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters,
    RTCRtpFecParameters, RtpCodecKind,
};
use rtc::rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit, SSRC};
use serde::Serialize;
use std::{
    collections::VecDeque,
    future::Future,
    io::{self, ErrorKind},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use webrtc::{
    data_channel::{DataChannel, DataChannelEvent},
    error::Error as WebRtcError,
    media_stream::{
        track_local::{static_sample::TrackLocalStaticSample, TrackLocal},
        track_remote::{TrackRemote, TrackRemoteEvent},
    },
    peer_connection::{
        PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
        RTCIceGatheringState, RTCPeerConnectionState, RTCSdpType, RTCSessionDescription,
    },
    runtime::Runtime,
};

const CONTROL_LABEL: &str = "nexus-control";
const DEFAULT_BIND: &str = "0.0.0.0:0";
const MAX_DATA_CHANNEL_MESSAGE: usize = 16_384;
const DATA_CHANNEL_SEND_BUFFER: usize = 2 * 1024 * 1024;
const GATHER_TIMEOUT: Duration = Duration::from_secs(5);
const CONTROL_SEND_TIMEOUT: Duration = Duration::from_secs(3);
/// One background write that has not completed in this long is abandoned.
/// Cancelling mid-frame leaves a sequence gap, which the receiver repairs with
/// a keyframe request; it must never block the session loop.
const VIDEO_WRITE_TIMEOUT: Duration = Duration::from_secs(3);
/// Frames the background writer may hold ahead of the pacer. Two rides out one
/// slow frame; beyond that the host drops rather than queue stale video.
const VIDEO_QUEUE_DEPTH: usize = 1;
const MAX_CONTROL_MESSAGES: usize = 1024;
const MAX_VIDEO_PACKETS: usize = 4096;
const MAX_VIDEO_BYTES: usize = 24 * 1024 * 1024;
/// Largest RTP packet accepted from a peer. One MTU of payload plus headers;
/// anything beyond that is not a media packet this transport produced.
const MAX_RTP_PACKET_BYTES: usize = 2_048;

/// HEVC as negotiated with the peer. Both endpoints register the codec, so the
/// payload type survives negotiation instead of being renumbered.
const HEVC_PAYLOAD_TYPE: u8 = 98;
/// FlexFEC-03 repair stream (RFC 8627) protecting the video SSRC.
const FLEX_FEC_PAYLOAD_TYPE: u8 = 49;
const FLEX_FEC_FRAME_BLOCK: u32 = 10;
const FLEX_FEC_REPAIR_PACKETS: u32 = 2;
/// The pacer's queue is bounded so a stalled path drops video instead of
/// accumulating seconds of stale frames. At the usual opening rate this is
/// under 100 ms of standing latency; FEC and NACK recover the drops.
const PACER_QUEUE_LIMIT: usize = 32;

/// Largest SDP accepted from a peer. Real offers are a few KiB; this bounds the
/// parser's exposure to an authenticated but misbehaving client.
pub const MAX_SIGNALING_BYTES: usize = 48 * 1024;

/// Test-only placement for the loss simulator: wire-ward of everything, so a
/// dropped packet is gone before the pacer, NACK buffer or transport sees it.
#[cfg(test)]
const DROP_MEDIA_SLOT: usize = 500;

/// Test-only lossy-path simulator. It drops one media packet in `drop_one_in`,
/// never repair packets, and otherwise passes traffic through unchanged.
#[cfg(test)]
struct DropMedia {
    drop_one_in: u64,
    fec_ssrcs: Vec<SSRC>,
    media_packets: u64,
    read_queue: VecDeque<rtc::interceptor::TaggedPacket>,
    write_queue: VecDeque<rtc::interceptor::TaggedPacket>,
}

#[cfg(test)]
impl DropMedia {
    fn new(drop_one_in: usize) -> Self {
        Self {
            drop_one_in: drop_one_in as u64,
            fec_ssrcs: Vec::new(),
            media_packets: 0,
            read_queue: VecDeque::new(),
            write_queue: VecDeque::new(),
        }
    }
}

#[cfg(test)]
impl rtc::sansio::Protocol<rtc::interceptor::TaggedPacket, rtc::interceptor::TaggedPacket, ()>
    for DropMedia
{
    type Rout = rtc::interceptor::TaggedPacket;
    type Wout = rtc::interceptor::TaggedPacket;
    type Eout = ();
    type Error = rtc::shared::error::Error;
    type Time = Instant;

    fn handle_read(&mut self, msg: rtc::interceptor::TaggedPacket) -> Result<(), Self::Error> {
        self.read_queue.push_back(msg);
        Ok(())
    }

    fn poll_read(&mut self) -> Option<Self::Rout> {
        self.read_queue.pop_front()
    }

    fn handle_write(&mut self, msg: rtc::interceptor::TaggedPacket) -> Result<(), Self::Error> {
        let rtc::interceptor::Packet::Rtp(ref packet) = msg.message.packet else {
            self.write_queue.push_back(msg);
            return Ok(());
        };
        if self.fec_ssrcs.contains(&packet.header.ssrc) {
            self.write_queue.push_back(msg);
            return Ok(());
        }
        self.media_packets += 1;
        if self.media_packets.is_multiple_of(self.drop_one_in) {
            return Ok(());
        }
        self.write_queue.push_back(msg);
        Ok(())
    }

    fn poll_write(&mut self) -> Option<Self::Wout> {
        self.write_queue.pop_front()
    }

    fn handle_timeout(&mut self, _now: Instant) -> Result<(), Self::Error> {
        Ok(())
    }

    fn poll_timeout(&mut self) -> Option<Self::Time> {
        None
    }
}

/// Test-only pass-through used to disable one interceptor slot without
/// disturbing the rest of the chain.
#[cfg(test)]
struct PassThrough {
    read_queue: VecDeque<rtc::interceptor::TaggedPacket>,
    write_queue: VecDeque<rtc::interceptor::TaggedPacket>,
}

#[cfg(test)]
impl PassThrough {
    fn new() -> Self {
        Self {
            read_queue: VecDeque::new(),
            write_queue: VecDeque::new(),
        }
    }
}

#[cfg(test)]
impl rtc::sansio::Protocol<rtc::interceptor::TaggedPacket, rtc::interceptor::TaggedPacket, ()>
    for PassThrough
{
    type Rout = rtc::interceptor::TaggedPacket;
    type Wout = rtc::interceptor::TaggedPacket;
    type Eout = ();
    type Error = rtc::shared::error::Error;
    type Time = Instant;

    fn handle_read(&mut self, msg: rtc::interceptor::TaggedPacket) -> Result<(), Self::Error> {
        self.read_queue.push_back(msg);
        Ok(())
    }

    fn poll_read(&mut self) -> Option<Self::Rout> {
        self.read_queue.pop_front()
    }

    fn handle_write(&mut self, msg: rtc::interceptor::TaggedPacket) -> Result<(), Self::Error> {
        self.write_queue.push_back(msg);
        Ok(())
    }

    fn poll_write(&mut self) -> Option<Self::Wout> {
        self.write_queue.pop_front()
    }

    fn handle_timeout(&mut self, _now: Instant) -> Result<(), Self::Error> {
        Ok(())
    }

    fn poll_timeout(&mut self) -> Option<Self::Time> {
        None
    }
}

#[cfg(test)]
impl rtc::interceptor::Interceptor for PassThrough {
    fn bind_local_stream(&mut self, _info: &rtc::interceptor::StreamInfo) {}
    fn unbind_local_stream(&mut self, _info: &rtc::interceptor::StreamInfo) {}
    fn bind_remote_stream(&mut self, _info: &rtc::interceptor::StreamInfo) {}
    fn unbind_remote_stream(&mut self, _info: &rtc::interceptor::StreamInfo) {}
}

#[cfg(test)]
impl rtc::interceptor::Interceptor for DropMedia {
    fn bind_local_stream(&mut self, info: &rtc::interceptor::StreamInfo) {
        if let Some(ssrc_fec) = info.ssrc_fec {
            self.fec_ssrcs.push(ssrc_fec);
        }
    }

    fn unbind_local_stream(&mut self, info: &rtc::interceptor::StreamInfo) {
        if let Some(ssrc_fec) = info.ssrc_fec {
            self.fec_ssrcs.retain(|ssrc| *ssrc != ssrc_fec);
        }
    }

    fn bind_remote_stream(&mut self, _info: &rtc::interceptor::StreamInfo) {}
    fn unbind_remote_stream(&mut self, _info: &rtc::interceptor::StreamInfo) {}
}

/// Wire-rate bounds for one session, in bits per second.
#[derive(Debug, Clone, Copy)]
struct RatePlan {
    initial_wire_bps: f64,
    min_wire_bps: f64,
    max_wire_bps: f64,
}

impl RatePlan {
    fn new(ceiling_mbps: f64, adaptive: bool) -> Self {
        let ceiling = if ceiling_mbps.is_finite() {
            ceiling_mbps.clamp(2.0, 200.0)
        } else {
            35.0
        };
        let max = wire_bps_for(ceiling);
        if adaptive {
            Self {
                // Start low enough that a poor path does not open with a
                // congestive burst, then let GCC probe upward.
                initial_wire_bps: wire_bps_for(ceiling.min(8.0)),
                min_wire_bps: wire_bps_for(2.0),
                max_wire_bps: max,
            }
        } else {
            Self {
                initial_wire_bps: max,
                min_wire_bps: max,
                max_wire_bps: max,
            }
        }
    }
}

/// STUN/TURN configuration shared by hosting and connecting.
///
/// Empty by default: Nexus contacts no third-party service unless the
/// operator types an address here. A STUN server only answers "what does my
/// NAT look like from outside"; a TURN server relays encrypted media when a
/// direct UDP path cannot be punched. TURN needs credentials, STUN does not.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IceSettings {
    pub stun_urls: Vec<String>,
    pub turn_urls: Vec<String>,
    pub turn_username: String,
    pub turn_credential: String,
}

impl IceSettings {
    /// Trims, drops blanks, validates schemes and TURN credentials, and caps
    /// the list so a typo cannot turn into a long ICE stall.
    pub fn normalized(mut self) -> Result<Self, String> {
        fn clean(values: Vec<String>, scheme: &str) -> Result<Vec<String>, String> {
            let mut cleaned = Vec::new();
            for value in values {
                let value = value.trim().to_string();
                if value.is_empty() {
                    continue;
                }
                if value.len() > 256 {
                    return Err(format!("The {scheme} server address is too long."));
                }
                let lower = value.to_ascii_lowercase();
                let valid = match scheme {
                    "STUN" => lower.starts_with("stun:"),
                    _ => lower.starts_with("turn:") || lower.starts_with("turns:"),
                };
                if !valid {
                    return Err(if scheme == "STUN" {
                        format!("`{value}` is not a stun: URL (for example stun:stun.l.google.com:19302).")
                    } else {
                        format!("`{value}` is not a turn: or turns: URL.")
                    });
                }
                cleaned.push(value);
            }
            if cleaned.len() > 4 {
                return Err(format!("Use at most four {scheme} servers."));
            }
            Ok(cleaned)
        }

        self.stun_urls = clean(self.stun_urls, "STUN")?;
        self.turn_urls = clean(self.turn_urls, "TURN")?;
        self.turn_username = self.turn_username.trim().to_string();
        self.turn_credential = self.turn_credential.trim().to_string();
        if !self.turn_urls.is_empty()
            && (self.turn_username.is_empty() || self.turn_credential.is_empty())
        {
            return Err("TURN servers need a username and credential.".to_string());
        }
        Ok(self)
    }

    pub fn is_empty(&self) -> bool {
        self.stun_urls.is_empty() && self.turn_urls.is_empty()
    }

    fn servers(&self) -> Vec<RTCIceServer> {
        let mut servers = Vec::new();
        if !self.stun_urls.is_empty() {
            servers.push(RTCIceServer {
                urls: self.stun_urls.clone(),
                username: String::new(),
                credential: String::new(),
            });
        }
        if !self.turn_urls.is_empty() {
            servers.push(RTCIceServer {
                urls: self.turn_urls.clone(),
                username: self.turn_username.clone(),
                credential: self.turn_credential.clone(),
            });
        }
        servers
    }

    /// Gathering needs longer when a STUN/TURN round trip is involved, but the
    /// host creates its answer while the 15-second approval countdown is
    /// running, so the extra wait stays bounded.
    fn gather_timeout(&self) -> Duration {
        if self.is_empty() {
            GATHER_TIMEOUT
        } else {
            GATHER_TIMEOUT + Duration::from_secs(3)
        }
    }
}

/// The candidate kinds found in an SDP description, deduplicated and sorted.
/// This is what the session HUD reports as the path, e.g. `srflx, host`.
fn candidate_types(sdp: &str) -> Vec<String> {
    let mut kinds: Vec<String> = Vec::new();
    for line in sdp.lines() {
        let line = line.trim();
        let rest = line
            .strip_prefix("a=candidate:")
            .or_else(|| line.strip_prefix("candidate:"));
        let Some(rest) = rest else { continue };
        let Some(index) = rest.find(" typ ") else {
            continue;
        };
        if let Some(kind) = rest[index + 5..].split_whitespace().next() {
            if !kind.is_empty() && !kinds.iter().any(|existing| existing == kind) {
                kinds.push(kind.to_string());
            }
        }
    }
    kinds.sort();
    kinds
}

/// WebRTC's async API is bridged onto the synchronous session threads by this
/// runtime. The peer-connection drivers run on webrtc's own reactor pool, so
/// blocking here never starves them.
fn bridge() -> &'static tokio::runtime::Runtime {
    static BRIDGE: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    BRIDGE.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("nexus-webrtc")
            .enable_all()
            .build()
            .expect("failed to build the WebRTC bridge runtime")
    })
}

fn drive<F: Future>(future: F) -> F::Output {
    bridge().block_on(future)
}

fn spawn_task(runtime: &Arc<dyn Runtime>, future: impl Future<Output = ()> + Send + 'static) {
    runtime.spawn_reactor(1, Box::pin(future));
}

#[derive(Default)]
struct VideoQueue {
    packets: VecDeque<RtpVideoPacket>,
    bytes: usize,
}

/// Receive side shared by the async reader tasks and the synchronous session.
/// Control messages are authoritative and drained first; video packets are
/// expendable, so the oldest are evicted under memory pressure.
#[derive(Default)]
struct Inbox {
    control: Mutex<VecDeque<Vec<u8>>>,
    video: Mutex<VideoQueue>,
    closed: AtomicBool,
    reason: Mutex<Option<String>>,
}

impl Inbox {
    fn push_control(&self, data: Vec<u8>) {
        let mut queue = self.control.lock().expect("control queue poisoned");
        if queue.len() >= MAX_CONTROL_MESSAGES {
            queue.pop_front();
        }
        queue.push_back(data);
    }

    fn push_video(&self, packet: RtpVideoPacket) {
        let mut queue = self.video.lock().expect("video queue poisoned");
        queue.bytes += packet.payload.len();
        queue.packets.push_back(packet);
        while queue.packets.len() > MAX_VIDEO_PACKETS || queue.bytes > MAX_VIDEO_BYTES {
            let Some(dropped) = queue.packets.pop_front() else {
                break;
            };
            queue.bytes = queue.bytes.saturating_sub(dropped.payload.len());
        }
    }

    fn close(&self, reason: &str) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            *self.reason.lock().expect("close reason poisoned") = Some(reason.to_string());
        }
    }

    fn closed_error(&self) -> Option<io::Error> {
        if !self.closed.load(Ordering::SeqCst) {
            return None;
        }
        let reason = self
            .reason
            .lock()
            .expect("close reason poisoned")
            .clone()
            .unwrap_or_else(|| "The WebRTC session ended.".to_string());
        Some(io::Error::new(ErrorKind::ConnectionAborted, reason))
    }

    fn poll_control(&self) -> io::Result<Option<Vec<u8>>> {
        if let Some(message) = self
            .control
            .lock()
            .expect("control queue poisoned")
            .pop_front()
        {
            return Ok(Some(message));
        }
        if let Some(error) = self.closed_error() {
            return Err(error);
        }
        Ok(None)
    }

    fn poll_video(&self) -> io::Result<Option<RtpVideoPacket>> {
        let mut queue = self.video.lock().expect("video queue poisoned");
        if let Some(packet) = queue.packets.pop_front() {
            queue.bytes = queue.bytes.saturating_sub(packet.payload.len());
            return Ok(Some(packet));
        }
        // Closure is reported by the control path, so a queued frame is never
        // skipped in favour of the error.
        Ok(None)
    }
}

struct EndpointHandler {
    runtime: Arc<dyn Runtime>,
    gathering: mpsc::Sender<()>,
    control: mpsc::Sender<Arc<dyn DataChannel>>,
    inbox: Arc<Inbox>,
    failure: Arc<Mutex<Option<String>>>,
}

#[async_trait]
impl PeerConnectionEventHandler for EndpointHandler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.gathering.send(());
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        if state == RTCPeerConnectionState::Failed {
            let mut failure = self.failure.lock().expect("failure slot poisoned");
            if failure.is_none() {
                *failure = Some("The WebRTC connection could not be established.".to_string());
            }
        }
    }

    async fn on_data_channel(&self, channel: Arc<dyn DataChannel>) {
        let runtime = self.runtime.clone();
        let control = self.control.clone();
        spawn_task(&runtime, async move {
            let Ok(label) = channel.label().await else {
                return;
            };
            if label != CONTROL_LABEL || control.send(channel.clone()).is_err() {
                let _ = channel.close().await;
            }
        });
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let runtime = self.runtime.clone();
        let inbox = self.inbox.clone();
        let failure = self.failure.clone();
        spawn_task(&runtime, async move {
            let mut media_ssrc = track.ssrcs().await.first().copied();
            while let Some(event) = track.poll().await {
                match event {
                    TrackRemoteEvent::OnRtpPacket(packet) => {
                        // Repair packets are consumed by the FEC decoder before
                        // this point. If the track reported no SSRC, learn it
                        // from the first media packet.
                        let ssrc = *media_ssrc.get_or_insert(packet.header.ssrc);
                        if packet.header.ssrc != ssrc {
                            continue;
                        }
                        if packet.header.payload_type != HEVC_PAYLOAD_TYPE {
                            continue;
                        }
                        if packet.payload.len() > MAX_RTP_PACKET_BYTES {
                            continue;
                        }
                        inbox.push_video(RtpVideoPacket {
                            sequence: packet.header.sequence_number,
                            timestamp: packet.header.timestamp,
                            marker: packet.header.marker,
                            payload: packet.payload,
                        });
                    }
                    TrackRemoteEvent::OnEnded => {
                        inbox.close("The remote video track ended.");
                        break;
                    }
                    TrackRemoteEvent::OnEnding => break,
                    _ => {}
                }
            }
            // A track that stops without an explicit end is a transport
            // failure; the session loop must not wait for frames forever.
            inbox.close("The remote video track stopped.");
            let mut slot = failure.lock().expect("failure slot poisoned");
            if slot.is_none() {
                *slot = Some("The remote video track stopped.".to_string());
            }
        });
    }
}

pub struct WebRtcEndpoint {
    runtime: Arc<dyn Runtime>,
    pc: Arc<dyn PeerConnection>,
    inbox: Arc<Inbox>,
    gather_rx: mpsc::Receiver<()>,
    control_rx: mpsc::Receiver<Arc<dyn DataChannel>>,
    open_tx: mpsc::Sender<()>,
    open_rx: mpsc::Receiver<()>,
    failure: Arc<Mutex<Option<String>>>,
    closed: Arc<AtomicBool>,
    control: Option<Arc<dyn DataChannel>>,
    video_track: Option<Arc<TrackLocalStaticSample>>,
    video_ssrc: Option<SSRC>,
    video_payload_type: Option<u8>,
    link: Option<Arc<LinkEstimate>>,
    gather_timeout: Duration,
    local_candidates: Vec<String>,
    remote_candidates: Vec<String>,
    handed_off: bool,
}

impl WebRtcEndpoint {
    /// Builds the offerer and returns its local description as JSON. The
    /// receive-only video transceiver and the control channel are created
    /// before the offer, so the offer carries both the media m-line and SCTP.
    /// Answers with the host's encoder ceiling, adaptive flag, ICE servers and
    /// the UDP port its media sockets should bind to (0 = ephemeral).
    pub fn answer_planned(
        offer_sdp: &str,
        ceiling_mbps: f64,
        adaptive: bool,
        ice: &IceSettings,
        media_port: u16,
    ) -> Result<(Self, String), String> {
        let bind = if media_port == 0 {
            DEFAULT_BIND.to_string()
        } else {
            format!("0.0.0.0:{media_port}")
        };
        Self::answer_on(offer_sdp, &bind, ceiling_mbps, adaptive, ice)
    }

    pub fn offer_on(bind: &str, ice: &IceSettings) -> Result<(Self, String), String> {
        Self::offer_on_inner(bind, false, ice)
    }

    /// `suppress_nack` lets a test isolate FlexFEC repair from NACK
    /// retransmission by replacing the loss detector with a pass-through.
    fn offer_on_inner(
        bind: &str,
        suppress_nack: bool,
        ice: &IceSettings,
    ) -> Result<(Self, String), String> {
        let mut endpoint = Self::build(bind, Role::Receiver { suppress_nack }, 0, ice)?;
        drive(endpoint.pc.add_transceiver_from_kind(
            RtpCodecKind::Video,
            Some(RTCRtpTransceiverInit {
                direction: RTCRtpTransceiverDirection::Recvonly,
                ..Default::default()
            }),
        ))
        .map_err(|error| format!("Could not open the WebRTC video receiver: {error}"))?;
        let control = drive(endpoint.pc.create_data_channel(CONTROL_LABEL, None))
            .map_err(|error| format!("Could not open the WebRTC control channel: {error}"))?;
        endpoint.control = Some(control);

        let offer = drive(endpoint.pc.create_offer(None))
            .map_err(|error| format!("Could not create a WebRTC offer: {error}"))?;
        let (sdp, candidates) = endpoint.publish_local(offer)?;
        endpoint.local_candidates = candidates;
        Ok((endpoint, sdp))
    }

    /// Answers an offer and attaches the video sender. `ceiling_mbps` is the
    /// configured encoder ceiling; `adaptive` false pins the estimate to it.
    pub fn answer_on(
        offer_sdp: &str,
        bind: &str,
        ceiling_mbps: f64,
        adaptive: bool,
        ice: &IceSettings,
    ) -> Result<(Self, String), String> {
        Self::answer_on_inner(offer_sdp, bind, ceiling_mbps, adaptive, 0, ice)
    }

    /// `drop_one_in` exists so a test can prove the FlexFEC repair flow repairs
    /// real losses; production answers pass 0.
    fn answer_on_inner(
        offer_sdp: &str,
        bind: &str,
        ceiling_mbps: f64,
        adaptive: bool,
        drop_one_in: usize,
        ice: &IceSettings,
    ) -> Result<(Self, String), String> {
        if offer_sdp.len() > MAX_SIGNALING_BYTES {
            return Err("The remote WebRTC offer is too large.".to_string());
        }
        let remote: RTCSessionDescription = serde_json::from_str(offer_sdp)
            .map_err(|error| format!("Could not read the WebRTC offer: {error}"))?;
        if remote.sdp_type != RTCSdpType::Offer {
            return Err("The remote description is not a WebRTC offer.".to_string());
        }

        let plan = RatePlan::new(ceiling_mbps, adaptive);
        let mut endpoint = Self::build(bind, Role::Sender(plan), drop_one_in, ice)?;
        endpoint.remote_candidates = candidate_types(&remote.sdp);
        drive(endpoint.pc.set_remote_description(remote))
            .map_err(|error| format!("Could not accept the WebRTC offer: {error}"))?;

        // The track must exist before the answer is created, or the answer
        // carries no sending m-line and the client never receives media.
        let (track, ssrc, payload_type) = Self::video_sender()?;
        let sender = drive(
            endpoint
                .pc
                .add_track(Arc::clone(&track) as Arc<dyn TrackLocal>),
        )
        .map_err(|error| format!("Could not attach the WebRTC video track: {error}"))?;

        let answer = drive(endpoint.pc.create_answer(None))
            .map_err(|error| format!("Could not create a WebRTC answer: {error}"))?;
        let (sdp, candidates) = endpoint.publish_local(answer)?;
        endpoint.local_candidates = candidates;

        // The negotiated payload type can differ from the offered one if the
        // remote ranked another codec. Resolve it from the sender.
        let negotiated = drive(sender.get_parameters())
            .ok()
            .and_then(|parameters| {
                parameters
                    .rtp_parameters
                    .codecs
                    .iter()
                    .find(|codec| !codec.rtp_codec.mime_type.to_lowercase().contains("flexfec"))
                    .map(|codec| codec.payload_type)
            })
            .unwrap_or(payload_type);

        endpoint.video_track = Some(track);
        endpoint.video_ssrc = Some(ssrc);
        endpoint.video_payload_type = Some(negotiated);
        Ok((endpoint, sdp))
    }

    fn video_sender() -> Result<(Arc<TrackLocalStaticSample>, SSRC, u8), String> {
        let mut rng = OsRng;
        let ssrc = rng.next_u32();
        let fec_ssrc = rng.next_u32();
        let codec = hevc_codec();
        let track = MediaStreamTrack::new(
            "nexus-video-stream".to_string(),
            "nexus-video-track".to_string(),
            "nexus-video".to_string(),
            RtpCodecKind::Video,
            vec![RTCRtpEncodingParameters {
                rtp_coding_parameters: RTCRtpCodingParameters {
                    ssrc: Some(ssrc),
                    fec: Some(RTCRtpFecParameters { ssrc: fec_ssrc }),
                    ..Default::default()
                },
                codec,
                ..Default::default()
            }],
        );
        let track = TrackLocalStaticSample::new(Instant::now(), track)
            .map_err(|error| format!("Could not prepare the WebRTC video track: {error}"))?;
        Ok((Arc::new(track), ssrc, HEVC_PAYLOAD_TYPE))
    }

    fn build(
        bind: &str,
        role: Role,
        drop_one_in: usize,
        ice: &IceSettings,
    ) -> Result<Self, String> {
        let runtime = webrtc::runtime::default_runtime()
            .ok_or_else(|| "No async runtime is available for WebRTC.".to_string())?;
        let (gather_tx, gather_rx) = mpsc::channel();
        let (control_tx, control_rx) = mpsc::channel();
        let (open_tx, open_rx) = mpsc::channel();
        let failure = Arc::new(Mutex::new(None));
        let inbox = Arc::new(Inbox::default());

        let mut media_engine = video_media_engine()?;
        let (registry, link) = match role {
            Role::Receiver { suppress_nack } => {
                (receiver_registry(&mut media_engine, suppress_nack)?, None)
            }
            Role::Sender(plan) => {
                let shared = Arc::new(LinkEstimate::new(plan.initial_wire_bps));
                let estimator = ScreenEstimator::new(
                    plan.initial_wire_bps,
                    plan.min_wire_bps,
                    plan.max_wire_bps,
                    shared.clone(),
                );
                (
                    sender_registry(
                        &mut media_engine,
                        estimator,
                        plan.initial_wire_bps,
                        drop_one_in,
                    )?,
                    Some(shared),
                )
            }
        };

        let mut configuration = RTCConfigurationBuilder::new();
        if !ice.is_empty() {
            configuration = configuration.with_ice_servers(ice.servers());
        }
        let handler = Arc::new(EndpointHandler {
            runtime: runtime.clone(),
            gathering: gather_tx,
            control: control_tx,
            inbox: inbox.clone(),
            failure: failure.clone(),
        });
        let connection = drive(
            PeerConnectionBuilder::new()
                .with_configuration(configuration.build())
                .with_media_engine(media_engine)
                .with_interceptor_registry(registry)
                .with_handler(handler)
                .with_runtime(runtime.clone())
                .with_udp_addrs(vec![bind.to_string()])
                .with_dedicated_reactor_pool_size(1)
                .with_data_channel_send_buffer_limit(DATA_CHANNEL_SEND_BUFFER)
                .build(),
        )
        .map_err(|error| format!("Could not start the WebRTC transport: {error}"))?;

        Ok(Self {
            runtime,
            pc: Arc::new(connection),
            inbox,
            gather_rx,
            control_rx,
            open_tx,
            open_rx,
            failure,
            closed: Arc::new(AtomicBool::new(false)),
            control: None,
            video_track: None,
            video_ssrc: None,
            video_payload_type: None,
            link,
            gather_timeout: ice.gather_timeout(),
            local_candidates: Vec::new(),
            remote_candidates: Vec::new(),
            handed_off: false,
        })
    }

    fn publish_local(
        &self,
        description: RTCSessionDescription,
    ) -> Result<(String, Vec<String>), String> {
        drive(self.pc.set_local_description(description.clone()))
            .map_err(|error| format!("Could not publish the WebRTC description: {error}"))?;
        self.wait_for_gathering(self.gather_timeout);
        let local = drive(self.pc.local_description()).unwrap_or(description);
        let types = candidate_types(&local.sdp);
        let json = serde_json::to_string(&local)
            .map_err(|error| format!("Could not encode the WebRTC description: {error}"))?;
        Ok((json, types))
    }

    fn wait_for_gathering(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match self
                .gather_rx
                .recv_timeout(remaining.min(Duration::from_millis(100)))
            {
                Ok(()) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    pub fn apply_answer(&mut self, answer_sdp: &str) -> Result<(), String> {
        if answer_sdp.len() > MAX_SIGNALING_BYTES {
            return Err("The remote WebRTC answer is too large.".to_string());
        }
        let remote: RTCSessionDescription = serde_json::from_str(answer_sdp)
            .map_err(|error| format!("Could not read the WebRTC answer: {error}"))?;
        if remote.sdp_type != RTCSdpType::Answer {
            return Err("The remote description is not a WebRTC answer.".to_string());
        }
        self.remote_candidates = candidate_types(&remote.sdp);
        drive(self.pc.set_remote_description(remote))
            .map_err(|error| format!("Could not accept the WebRTC answer: {error}"))
    }

    /// Waits for the control channel to open and hands the endpoint to a
    /// synchronous channel facade. Consumes the endpoint: a failed connect
    /// closes it.
    pub fn connect(mut self, timeout: Duration) -> Result<WebRtcChannel, String> {
        let deadline = Instant::now() + timeout;
        let control = match self.control.take() {
            Some(channel) => channel,
            None => Self::receive_control(&self.control_rx, &self.failure, deadline)?,
        };

        spawn_control_reader(
            &self.runtime,
            control.clone(),
            self.inbox.clone(),
            self.open_tx.clone(),
            self.failure.clone(),
        );

        loop {
            if let Some(reason) = self.failure.lock().expect("failure slot poisoned").clone() {
                return Err(reason);
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err("Timed out while opening the WebRTC control channel.".to_string());
            };
            match self
                .open_rx
                .recv_timeout(remaining.min(Duration::from_millis(50)))
            {
                Ok(()) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("The WebRTC control channel closed before it opened.".to_string())
                }
            }
        }

        self.handed_off = true;
        let ice_path = if self.local_candidates.is_empty() && self.remote_candidates.is_empty() {
            None
        } else {
            let local = if self.local_candidates.is_empty() {
                "unknown".to_string()
            } else {
                self.local_candidates.join(", ")
            };
            let remote = if self.remote_candidates.is_empty() {
                "unknown".to_string()
            } else {
                self.remote_candidates.join(", ")
            };
            Some(format!("{local} → {remote}"))
        };
        let video_writer = self.video_track.take().map(|track| {
            VideoWriter::spawn(VideoSender {
                track,
                ssrc: self.video_ssrc.unwrap_or_default(),
                payload_type: self.video_payload_type.unwrap_or(HEVC_PAYLOAD_TYPE),
            })
        });
        Ok(WebRtcChannel {
            runtime: self.runtime.clone(),
            pc: self.pc.clone(),
            control,
            video_writer,
            ice_path,
            link: self.link.clone(),
            inbox: self.inbox.clone(),
            closed: self.closed.clone(),
        })
    }

    fn receive_control(
        receiver: &mpsc::Receiver<Arc<dyn DataChannel>>,
        failure: &Mutex<Option<String>>,
        deadline: Instant,
    ) -> Result<Arc<dyn DataChannel>, String> {
        loop {
            if let Some(reason) = failure.lock().expect("failure slot poisoned").clone() {
                return Err(reason);
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err("Timed out waiting for the WebRTC control channel.".to_string());
            };
            match receiver.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Ok(channel) => return Ok(channel),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("The WebRTC control channel was never opened.".to_string())
                }
            }
        }
    }
}

enum Role {
    Receiver { suppress_nack: bool },
    Sender(RatePlan),
}

impl Drop for WebRtcEndpoint {
    fn drop(&mut self) {
        if self.handed_off {
            return;
        }
        close_transport(&self.runtime, &self.pc, &self.inbox, &self.closed);
    }
}

struct VideoSender {
    track: Arc<TrackLocalStaticSample>,
    ssrc: SSRC,
    payload_type: u8,
}

/// Background writer for the RTP track. The session loop must never block on a
/// media write: a keyframe is hundreds of awaits, and the host loop also
/// services capture, input and control. Frames pass through a small mailbox;
/// when the writer is behind, the newest frame is refused and the host treats
/// that as backpressure.
struct VideoWriter {
    sender: Mutex<Option<mpsc::SyncSender<Sample>>>,
    failure: Arc<Mutex<Option<String>>>,
}

impl VideoWriter {
    fn spawn(video: VideoSender) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Sample>(VIDEO_QUEUE_DEPTH);
        let failure = Arc::new(Mutex::new(None));
        let thread_failure = failure.clone();
        let _ = std::thread::Builder::new()
            .name("nexus-rtp-video".to_string())
            .spawn(move || {
                while let Ok(sample) = receiver.recv() {
                    let written = drive(async {
                        tokio::time::timeout(
                            VIDEO_WRITE_TIMEOUT,
                            video
                                .track
                                .sample_writer(video.ssrc, video.payload_type)
                                .write_sample(&sample),
                        )
                        .await
                    });
                    match written {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            *thread_failure.lock().expect("video failure poisoned") =
                                Some(map_send_error(error).to_string());
                            break;
                        }
                        Err(_elapsed) => {
                            // Cancelled mid-frame: the packetizer consumed the
                            // sequence numbers, so the receiver sees a gap and
                            // asks for a keyframe. Keep the writer alive.
                        }
                    }
                }
            });
        Self {
            sender: Mutex::new(Some(sender)),
            failure,
        }
    }

    fn submit(&self, sample: Sample) -> io::Result<()> {
        if let Some(reason) = self.failure.lock().expect("video failure poisoned").clone() {
            return Err(io::Error::new(ErrorKind::ConnectionAborted, reason));
        }
        let guard = self.sender.lock().expect("video sender poisoned");
        let Some(sender) = guard.as_ref() else {
            return Err(io::Error::new(
                ErrorKind::ConnectionAborted,
                "The video writer is closed.",
            ));
        };
        match sender.try_send(sample) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(_)) => Err(io::Error::new(
                ErrorKind::WouldBlock,
                "The video writer is behind.",
            )),
            Err(mpsc::TrySendError::Disconnected(_)) => Err(io::Error::new(
                ErrorKind::ConnectionAborted,
                "The video writer stopped.",
            )),
        }
    }

    fn stop(&self) {
        self.sender.lock().expect("video sender poisoned").take();
    }
}

pub struct WebRtcChannel {
    runtime: Arc<dyn Runtime>,
    pc: Arc<dyn PeerConnection>,
    control: Arc<dyn DataChannel>,
    video_writer: Option<VideoWriter>,
    ice_path: Option<String>,
    link: Option<Arc<LinkEstimate>>,
    inbox: Arc<Inbox>,
    closed: Arc<AtomicBool>,
}

impl WebRtcChannel {
    pub fn send_control<T: Serialize>(&mut self, message: &T) -> io::Result<()> {
        let json = serde_json::to_vec(message)
            .map_err(|error| io::Error::new(ErrorKind::InvalidInput, error.to_string()))?;
        let mut payload = Vec::with_capacity(json.len() + 1);
        payload.push(MESSAGE_CONTROL);
        payload.extend_from_slice(&json);
        send_blocking(&self.control, &payload)
    }

    pub fn poll_recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        self.inbox.poll_control()
    }

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

    /// One inbound video packet, or `None` when nothing has arrived yet.
    pub fn poll_video(&mut self) -> io::Result<Option<RtpVideoPacket>> {
        self.inbox.poll_video()
    }

    /// Packetizes one Annex-B HEVC access unit onto the RTP track. The duration
    /// is the frame interval and drives the RTP timestamp increment.
    pub fn send_video(&mut self, annex_b: &[u8], duration: Duration) -> io::Result<()> {
        if annex_b.is_empty() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "Cannot send an empty video frame.",
            ));
        }
        let Some(writer) = self.video_writer.as_ref() else {
            return Err(io::Error::new(
                ErrorKind::Unsupported,
                "This endpoint does not carry an outbound video track.",
            ));
        };
        let sample = Sample {
            data: Bytes::copy_from_slice(annex_b),
            duration,
            ..Sample::new(Instant::now())
        };
        writer.submit(sample)
    }

    /// Candidate kinds that produced this session, e.g. `srflx → host`.
    pub fn ice_path(&self) -> Option<String> {
        self.ice_path.clone()
    }

    /// The host's live bandwidth estimate, in wire bits per second.
    pub fn link_estimate(&self) -> Option<Arc<LinkEstimate>> {
        self.link.clone()
    }

    pub fn shutdown(&self) {
        if let Some(writer) = &self.video_writer {
            writer.stop();
        }
        close_transport(&self.runtime, &self.pc, &self.inbox, &self.closed);
    }
}

impl Drop for WebRtcChannel {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn video_media_engine() -> Result<MediaEngine, String> {
    let mut engine = MediaEngine::default();
    engine
        .register_codec(
            RTCRtpCodecParameters {
                rtp_codec: hevc_codec(),
                payload_type: HEVC_PAYLOAD_TYPE,
            },
            RtpCodecKind::Video,
        )
        .map_err(|error| format!("Could not register the HEVC codec: {error}"))?;
    engine
        .register_codec(
            RTCRtpCodecParameters {
                rtp_codec: RTCRtpCodec {
                    mime_type: MIME_TYPE_FLEX_FEC03.to_string(),
                    clock_rate: 90_000,
                    channels: 0,
                    sdp_fmtp_line: "repair-window=10000000".to_string(),
                    rtcp_feedback: vec![],
                },
                payload_type: FLEX_FEC_PAYLOAD_TYPE,
            },
            RtpCodecKind::Video,
        )
        .map_err(|error| format!("Could not register the FlexFEC codec: {error}"))?;
    Ok(engine)
}

fn hevc_codec() -> RTCRtpCodec {
    RTCRtpCodec {
        mime_type: MIME_TYPE_HEVC.to_string(),
        clock_rate: 90_000,
        channels: 0,
        sdp_fmtp_line: String::new(),
        rtcp_feedback: vec![],
    }
}

/// The receiving side reports arrivals and asks for retransmission and repair;
/// it does not estimate anything because it sends no media.
fn receiver_registry(
    media_engine: &mut MediaEngine,
    _suppress_nack: bool,
) -> Result<Registry, String> {
    let registry = configure_twcc_receiver_only(Registry::new(), media_engine)
        .map_err(|error| format!("Could not configure arrival feedback: {error}"))?;
    let registry = configure_nack(registry, media_engine);
    #[cfg(test)]
    let registry = if _suppress_nack {
        // A pass-through at the generator slot keeps RTCP flowing to the TWCC
        // reporter while guaranteeing that only FlexFEC can repair the loss.
        registry.with(Slot::NackGenerator, PassThrough::new())
    } else {
        registry
    };
    let registry = registry.with(Slot::FecDecoder, FlexFec03ReceiveBuilder::new().build());
    Ok(configure_rtcp_reports(registry))
}

/// The sending side estimates the path, paces packets at the estimate, answers
/// NACKs and generates FlexFEC repair.
fn sender_registry(
    media_engine: &mut MediaEngine,
    estimator: ScreenEstimator,
    initial_wire_bps: f64,
    _drop_one_in: usize,
) -> Result<Registry, String> {
    let registry = configure_congestion_control(
        Registry::new(),
        estimator,
        CongestionFeedback::Twcc,
        media_engine,
    )
    .map_err(|error| format!("Could not configure congestion control: {error}"))?;
    let registry = configure_nack(registry, media_engine);
    let registry = registry
        // Congestion control installs a default pacer. Replace it so the first
        // frames are metered at the session's opening rate rather than the
        // builder default, and so a stalled path cannot hold seconds of video.
        .with(
            Slot::Pacer,
            PacerBuilder::new()
                .with_target_bitrate(initial_wire_bps * PACING_FACTOR)
                .with_queue_limit(PACER_QUEUE_LIMIT)
                .build(),
        )
        .with(
            Slot::FecEncoder,
            FlexFec03SendBuilder::new()
                .with_num_media_packets(FLEX_FEC_FRAME_BLOCK)
                .with_num_fec_packets(FLEX_FEC_REPAIR_PACKETS)
                .build(),
        );
    #[cfg(test)]
    let registry = if _drop_one_in > 0 {
        registry.with(Slot::from(DROP_MEDIA_SLOT), DropMedia::new(_drop_one_in))
    } else {
        registry
    };
    Ok(configure_rtcp_reports(registry))
}

fn close_transport(
    runtime: &Arc<dyn Runtime>,
    pc: &Arc<dyn PeerConnection>,
    inbox: &Arc<Inbox>,
    closed: &Arc<AtomicBool>,
) {
    if closed.swap(true, Ordering::SeqCst) {
        return;
    }
    inbox.close("The WebRTC transport was closed.");
    let pc = pc.clone();
    spawn_task(runtime, async move {
        let _ = pc.close().await;
    });
}

fn send_blocking(channel: &Arc<dyn DataChannel>, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_DATA_CHANNEL_MESSAGE {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "Transport message is too large for a WebRTC data channel.",
        ));
    }
    let data = BytesMut::from(payload);
    match drive(async {
        tokio::time::timeout(CONTROL_SEND_TIMEOUT, channel.send(data))
            .await
            .ok()
    }) {
        Some(Ok(())) => Ok(()),
        Some(Err(error)) => Err(map_send_error(error)),
        None => Err(io::Error::new(
            ErrorKind::TimedOut,
            "Timed out while sending to the remote device.",
        )),
    }
}

fn map_send_error(error: WebRtcError) -> io::Error {
    match error {
        WebRtcError::ErrDataChannelClosed
        | WebRtcError::ErrDataChannelNotOpen
        | WebRtcError::ErrDataChannelNotExisted
        | WebRtcError::ErrConnectionClosed => io::Error::new(
            ErrorKind::ConnectionAborted,
            "The remote device closed the WebRTC session.",
        ),
        other => io::Error::other(format!("WebRTC transport error: {other}")),
    }
}

fn spawn_control_reader(
    runtime: &Arc<dyn Runtime>,
    channel: Arc<dyn DataChannel>,
    inbox: Arc<Inbox>,
    opened: mpsc::Sender<()>,
    failure: Arc<Mutex<Option<String>>>,
) {
    let runtime = runtime.clone();
    spawn_task(&runtime, async move {
        while let Some(event) = channel.poll().await {
            match event {
                DataChannelEvent::OnOpen => {
                    let _ = opened.send(());
                }
                DataChannelEvent::OnMessage(message) => {
                    inbox.push_control(message.data.to_vec());
                }
                DataChannelEvent::OnClose | DataChannelEvent::OnError => {
                    let mut slot = match failure.lock() {
                        Ok(slot) => slot,
                        Err(_) => break,
                    };
                    if slot.is_none() {
                        *slot = Some("The remote device closed the WebRTC session.".to_string());
                    }
                    drop(slot);
                    if matches!(event, DataChannelEvent::OnClose) {
                        break;
                    }
                }
                DataChannelEvent::OnClosing
                | DataChannelEvent::OnBufferedAmountLow
                | DataChannelEvent::OnBufferedAmountHigh => {}
                _ => {}
            }
        }
        inbox.close("The remote device closed the WebRTC session.");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rtp_video::{AssemblerOutput, RtpAssembler};
    use crate::transport::{parse_control, Control};
    use std::thread;

    /// A small but structurally valid Annex-B HEVC access unit: VPS, SPS, PPS
    /// and an IDR. The payloader only inspects NAL headers, so the payloads do
    /// not need to decode.
    /// Submits one frame, waiting out the writer mailbox the way the host does
    /// when its capture loop is slower than the pacer.
    fn send_frame(channel: &mut WebRtcChannel, sample: &[u8]) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match channel.send_video(sample, Duration::from_millis(16)) {
                Ok(()) => return,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "the video writer never drained the queued frame"
                    );
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("send video failed: {error}"),
            }
        }
    }

    fn hevc_access_unit() -> Vec<u8> {
        let nalus: [(&[u8; 2], &[u8]); 4] = [
            (&[0x40, 0x01], &[0x0c, 0x01, 0xff, 0xff, 0x01, 0x60]),
            (&[0x42, 0x01], &[0x01, 0x01, 0x60, 0x00, 0x00, 0x03]),
            (&[0x44, 0x01], &[0xc1, 0x62, 0x00]),
            (
                &[0x26, 0x01],
                &[0xaf, 0x08, 0x40, 0x00, 0x11, 0x22, 0x33, 0x44],
            ),
        ];
        let mut data = Vec::new();
        for (header, payload) in nalus {
            data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            data.extend_from_slice(header);
            data.extend_from_slice(payload);
        }
        data
    }

    #[test]
    fn negotiation_advertises_hevc_and_a_fec_repair_flow() {
        let (_offerer, offer_sdp) =
            WebRtcEndpoint::offer_on("127.0.0.1:0", &IceSettings::default()).expect("offer");
        assert!(
            offer_sdp.contains("H265/90000"),
            "the offer must advertise HEVC: {offer_sdp}"
        );
        let (_answerer, answer_sdp) = WebRtcEndpoint::answer_on(
            &offer_sdp,
            "127.0.0.1:0",
            35.0,
            true,
            &IceSettings::default(),
        )
        .expect("answer");
        assert!(
            answer_sdp.contains("H265/90000"),
            "the answer must accept HEVC: {answer_sdp}"
        );
        assert!(
            answer_sdp.to_lowercase().contains("flexfec-03/90000"),
            "the answer must select the repair codec: {answer_sdp}"
        );
        assert!(
            answer_sdp.contains("ssrc-group:FEC-FR"),
            "the answer must associate the repair SSRC: {answer_sdp}"
        );
    }

    #[test]
    fn streams_an_access_unit_over_rtp() {
        let (mut offerer, offer_sdp) =
            WebRtcEndpoint::offer_on("127.0.0.1:0", &IceSettings::default()).expect("offer");
        let (answerer, answer_sdp) = WebRtcEndpoint::answer_on(
            &offer_sdp,
            "127.0.0.1:0",
            35.0,
            true,
            &IceSettings::default(),
        )
        .expect("answer");
        offerer.apply_answer(&answer_sdp).expect("apply answer");

        let offerer_thread = thread::spawn(move || offerer.connect(Duration::from_secs(15)));
        let mut answerer = answerer
            .connect(Duration::from_secs(15))
            .expect("answerer connect");
        let mut offerer = offerer_thread
            .join()
            .expect("offerer thread")
            .expect("offerer connect");

        assert!(
            answerer.link_estimate().is_some(),
            "the sending side must publish a live estimate"
        );

        offerer
            .send_control(&Control::Ping { sent_ms: 7 })
            .expect("send control");
        let message = answerer
            .recv_timeout(Duration::from_secs(5))
            .expect("receive control")
            .expect("control message");
        match parse_control::<Control>(&message)
            .expect("parse control")
            .expect("control")
        {
            Control::Ping { sent_ms } => assert_eq!(sent_ms, 7),
            other => panic!("unexpected control message: {other:?}"),
        }

        let sample = hevc_access_unit();
        send_frame(&mut answerer, &sample);

        let mut assembler = RtpAssembler::default();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut frame = None;
        while frame.is_none() && Instant::now() < deadline {
            match offerer.poll_video() {
                Ok(Some(packet)) => {
                    assembler.push(packet, Instant::now());
                    while let Some(output) = assembler.poll(Instant::now()) {
                        if let AssemblerOutput::Frame(unit) = output {
                            frame = Some(unit);
                            break;
                        }
                    }
                }
                Ok(None) => thread::sleep(Duration::from_millis(2)),
                Err(error) => panic!("video receive failed: {error}"),
            }
        }
        let frame = frame.expect("an assembled access unit");
        assert!(
            frame
                .annex_b
                .windows(sample.len())
                .any(|window| window == sample.as_slice()),
            "the access unit must carry the encoded NAL units"
        );
    }

    #[test]
    fn normalizes_ice_server_settings() {
        let ice = IceSettings {
            stun_urls: vec![
                "  stun:stun.l.google.com:19302 ".to_string(),
                String::new(),
                "stun:stun.cloudflare.com:3478".to_string(),
            ],
            turn_urls: vec!["turn:relay.example.com:3478?transport=udp".to_string()],
            turn_username: " parsec ".to_string(),
            turn_credential: "secret".to_string(),
        }
        .normalized()
        .expect("valid settings");
        assert_eq!(ice.stun_urls.len(), 2);
        assert_eq!(ice.stun_urls[0], "stun:stun.l.google.com:19302");
        assert_eq!(ice.turn_username, "parsec");

        assert!(IceSettings {
            stun_urls: vec!["http://example.com".to_string()],
            ..Default::default()
        }
        .normalized()
        .is_err());
        assert!(IceSettings {
            turn_urls: vec!["turn:relay.example.com:3478".to_string()],
            ..Default::default()
        }
        .normalized()
        .is_err());
        assert!(IceSettings::default()
            .normalized()
            .expect("empty")
            .is_empty());
    }

    #[test]
    fn reads_candidate_kinds_from_sdp() {
        let sdp = "v=0\r\n\
a=candidate:1 1 udp 2122260223 192.168.1.5 39201 typ host generation 0\r\n\
a=candidate:2 1 udp 1686052607 203.0.113.7 39201 typ srflx raddr 192.168.1.5 rport 39201\r\n\
a=candidate:3 1 udp 41885439 198.51.100.9 50000 typ relay raddr 203.0.113.7 rport 39201\r\n\
a=candidate:4 1 udp 1686052607 203.0.113.7 39201 typ srflx raddr 192.168.1.5 rport 39201\r\n";
        assert_eq!(candidate_types(sdp), vec!["host", "relay", "srflx"]);
    }

    #[test]
    fn drops_the_oldest_control_messages_when_the_queue_fills() {
        let inbox = Inbox::default();
        for index in 0..(MAX_CONTROL_MESSAGES + 4) {
            inbox.push_control(index.to_be_bytes().to_vec());
        }
        for expected in 4..(MAX_CONTROL_MESSAGES + 4) {
            assert_eq!(
                inbox.poll_control().expect("poll").expect("control"),
                expected.to_be_bytes().to_vec()
            );
        }
        assert!(inbox.poll_control().expect("empty").is_none());
    }

    #[test]
    fn flexfec_repairs_dropped_media_packets() {
        // NACK generation is disabled in this test: the only thing that can
        // repair the two dropped packets is the FlexFEC block.
        let (mut offerer, offer_sdp) =
            WebRtcEndpoint::offer_on_inner("127.0.0.1:0", true, &IceSettings::default())
                .expect("offer");
        // One media packet in five never reaches the wire. The first FlexFEC
        // block covers ten media packets with two repair packets, so both
        // drops inside that block are recoverable by repair alone.
        let (answerer, answer_sdp) = WebRtcEndpoint::answer_on_inner(
            &offer_sdp,
            "127.0.0.1:0",
            35.0,
            true,
            5,
            &IceSettings::default(),
        )
        .expect("answer");
        offerer.apply_answer(&answer_sdp).expect("apply answer");

        let offerer_thread = thread::spawn(move || offerer.connect(Duration::from_secs(15)));
        let mut answerer = answerer
            .connect(Duration::from_secs(15))
            .expect("answerer connect");
        let mut offerer = offerer_thread
            .join()
            .expect("offerer thread")
            .expect("offerer connect");

        let sample = hevc_access_unit();
        for _ in 0..12 {
            send_frame(&mut answerer, &sample);
        }

        let mut assembler = RtpAssembler::default();
        let mut frames = 0usize;
        let mut loss_declared = false;
        let deadline = Instant::now() + Duration::from_secs(10);
        while frames < 12 && Instant::now() < deadline {
            match offerer.poll_video() {
                Ok(Some(packet)) => {
                    assembler.push(packet, Instant::now());
                    while let Some(output) = assembler.poll(Instant::now()) {
                        match output {
                            AssemblerOutput::Frame(_) => frames += 1,
                            AssemblerOutput::Loss => loss_declared = true,
                        }
                    }
                }
                Ok(None) => thread::sleep(Duration::from_millis(1)),
                Err(error) => panic!("video receive failed: {error}"),
            }
        }

        assert!(
            !loss_declared,
            "FlexFEC should have repaired both drops inside the block, not declared loss"
        );
        assert_eq!(frames, 12, "every access unit should arrive after repair");
    }
}
