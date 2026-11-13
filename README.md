# ParsecPlus

A clean-room, low-latency remote desktop client and host for macOS, with a web preview fallback. Built with Tauri 2, React 19, TypeScript, ScreenCaptureKit, and VideoToolbox.

> **Independence notice.** ParsecPlus is an independent clean-room project. It is not affiliated with, endorsed by, or derived from Parsec Technologies. All interface design, artwork, and code in this repository are original.

## Status

**Phase 1 — interactive product shell (complete, real data only).** The app ships its full surface: a local device profile, real recent connections, Host configuration and diagnostics, settings, search, Arcade, and real live-preview/remote session screens with telemetry, HUD, and controls. There is no seeded demo workspace: friends, teams, the computer library and simulated sessions have been removed, and Friends/Teams are explicit account-phase placeholders.

**Phase 2 — capture, encode, input (implemented on macOS).** The native app captures a display with ScreenCaptureKit, encodes with the hardware HEVC encoder through VideoToolbox, and streams JPEG preview frames into the live session view at a capped preview rate. Capture and encode metrics are measured in Rust and surfaced in the HUD and performance panel. A guarded CGEvent input backend is implemented behind Accessibility permission and explicit arming; it is intentionally not wired into local preview.

**Phase 3 — encrypted direct transport (implemented on macOS).** A ParsecPlus host can accept one direct client on the local network and stream hardware HEVC video over an authenticated, encrypted channel: an RTP video track over UDP when ICE connects, with the Noise-encrypted TCP session as signaling and fallback. The client decodes with hardware HEVC and presents decoded frames directly on a native Metal surface at the stream's full resolution, with real round-trip and decode telemetry. Remote input is forwarded only when the host explicitly allows it, and every injected event still requires Accessibility trust and an armed session.

**Phase 3.5 — reachability foundation (implemented).** Hosts advertise themselves on the local network over Bonjour (`_parsecplus._tcp`), and the Direct connect dialog lists nearby hosts automatically. A separate, self-hostable relay crate adds room-based rendezvous for peers that cannot reach each other directly. The relay only pairs two sockets and forwards opaque bytes; the Noise handshake still authenticates both peers with the pairing code, and the host approval gate still runs.

**Phase 3.6 — adaptive streaming (implemented on macOS).** The client measures frame delivery, arrival jitter, and decode continuity, and sends that health report back over the encrypted channel once per second. On the TCP fallback, the host uses it to retarget the hardware HEVC encoder between 2 Mbps and the configured maximum, stepping down quickly under congestion and recovering after stable windows. Missing frames and decode failures trigger an immediate keyframe request.

**Phase 3.7 — WebRTC transport (implemented on macOS).** Direct sessions negotiate a WebRTC transport using `webrtc-rs`: an ordered, reliable SCTP data channel carries control messages and input, and the ICE/DTLS session carries the media. ICE, DTLS, and SCTP negotiation run inside the process, while SDP offer/answer and transport selection travel through the existing Noise-authenticated TCP channel, so only a peer holding the pairing code can inject a candidate or fingerprint. TCP remains the signaling path and the automatic fallback when ICE cannot connect. Relay sessions stay on TCP because the relay only forwards opaque bytes, so WebRTC would see another TCP hop rather than a direct path. No STUN or TURN server is configured by default, so candidates are local interfaces only and no third-party service is contacted; configuring one (Phase 3.9) enables NAT traversal for internet sessions.

**Phase 3.8 — RTP media path (implemented on macOS).** Video no longer rides SCTP. The host packetizes each encoded access unit with the HEVC RTP payload format onto an encrypted RTP track, and the client depacketizes it back into access units. Transport-wide feedback from the receiver feeds a delay-based congestion estimator on the sender, which sizes the running encoder and drives a packet pacer; NACK retransmits individual losses and a FlexFEC-03 repair flow recovers a block of ten media packets from two repair packets. Both are negotiated in the SDP. The ordered control channel still carries input, telemetry, keyframe requests, and commands, and the TCP fallback keeps its chunk-framed video path and delivery-ratio controller. This is the change that removes SCTP's loss-based backoff and send buffer from the real-time video path.

**Phase 3.9 — internet reachability (implemented).** Sessions no longer stop at the LAN. STUN and TURN servers are configurable for both hosting and connecting; the host can pin its media UDP port so a router forward is deterministic; and **Host → Network → Detect public address** sends one STUN binding request from that media port, showing the public IP a friend should dial and the UDP mapping the router produced. A mapped port equal to the configured media port means the forward works; a different one tells the operator which external port to forward. Empty fields keep the old behaviour: no third party contacted, local candidates only. The two-Mac procedure, port table, TURN and relay fallbacks, and troubleshooting live in [TEST_GUIDE.md](TEST_GUIDE.md).

**Phase 3.10 — Tailscale-aware join codes (implemented).** For overlays that already provide an L3 network (this deployment uses Tailscale), the host screen enumerates its interfaces, detects the Tailscale tailnet through `tailscale status --json`, and lets the host pick the address to share. It then turns address + TCP port + the current reusable invite code into one `PC1-…` direct connection string. The friend pastes that string under **Connect directly → Join code**, the existing approval gate runs on the host, and the stream starts — no port forwarding, STUN, or relay involved. Direct connection strings stay valid for saved computers until you rotate the invite code manually. Persistent friend lists still need the Phase 4 service layer; the join code is the secure, accountless equivalent.

**Phase 3.11 — multi-viewer presence (implemented).** The host no longer admits one viewer and rejects the rest: direct sessions run concurrently (up to eight), each with its own transport, encoder and statistics. Incoming requests persist for two minutes in a floating **presence dock** visible on every screen, instead of disappearing after fifteen seconds; clicking a waiting avatar accepts or rejects that specific device. Connected avatars open a popover with the device, peer address, transport/ICE path, live fps/bitrate/jitter, per-person **keyboard, mouse and gamepad** switches, and a **Disconnect** action. The Host screen itself is reorganised into sticky **Host · Access · Video · Network · Advanced** sections, and "Match display" now derives a 16:10-safe capture size from the real display (1920×1243 on the 1710×1107 logical panel) instead of forcing 1920×1080, so the remote picture is no longer letterboxed or cropped. Default host bitrate is now 12 Mbps, and the RTP path keeps a session-long base delay while trimming the encoder on receiver RTT/loss so a Wi-Fi standing queue cannot silently add hundreds of milliseconds.

### What is real in the native build

- Custom frameless macOS window with traffic lights, drag region, and window controls.
- System details from the local machine (`hostname`, OS version, chip, architecture, model).
- Screen Recording, Accessibility, and Input Monitoring status via CoreGraphics / ApplicationServices, plus System Settings deep links for permission approval.
- ScreenCaptureKit display capture with hardware cursor, scaled to the requested session resolution.
- Hardware HEVC (Annex B) VideoToolbox encoding with a configurable bitrate, frame rate, and keyframe interval.
- Direct hosting on TCP 39200: listener lifecycle, streaming status, host metrics, error reporting, and pairing-code rotation.
- A WebRTC transport (`webrtc-rs`) for direct sessions: an encrypted RTP video track with HEVC packetization, a delay-based congestion estimator, a pacer, NACK and FlexFEC repair, plus one ordered SCTP control channel over ICE/UDP, authenticated through SDP exchanged inside the Noise channel, with automatic TCP fallback when ICE cannot connect.
- Configurable STUN/TURN ICE servers for both roles, a fixed host media UDP port, and a built-in STUN public-address probe that doubles as a router port-forward check.
- Tailscale-aware address detection and a single-string join code (address + port + pairing code) that the host shares so a friend can connect and approve with one paste.
- Bonjour advertisement while hosting and nearby-host discovery in the client; the advertisement carries only the host name, platform, version, and a stable non-secret device ID. It never contains the pairing code.
- A self-hostable relay route: the host holds one rendezvous room, the client joins it with the relay invite, and the relay forwards encrypted bytes between the two sockets.
- An authenticated Noise XX channel (`XXpsk3_25519_ChaChaPoly_BLAKE2s`) with a pre-shared key derived from the single-use pairing code. Noise protects signaling, the relay route, and every TCP fallback session; WebRTC media is protected by DTLS/SRTP and control by DTLS/SCTP underneath, with the DTLS fingerprints authenticated by the Noise channel.
- A host approval gate: after the encrypted handshake succeeds, the host sees who is asking (owner, device, peer address) and can decline or accept from a request that persists for two minutes in the presence dock. Declining or timing out rotates the pairing code.
- A direct client that connects by address and code, decodes HEVC in hardware, and draws each decoded frame straight onto a native Metal layer at full stream resolution. There is no JPEG re-encode, base64 hop, or `<img>` surface on the remote path.
- A transparent session WebView above the video layer: HUD, stats, notices, and the session menu stay interactive while the Metal view renders underneath. The view is repositioned and rescaled on window resize and fullscreen changes.
- Encrypted stream feedback and host-side rate control: on the RTP path the host's delay-based estimator consumes transport-wide feedback and retargets the encoder inside the measured link; on the TCP fallback the client reports delivered frames, jitter, dropped frames, and RTT, and the host reacts to blocking writes. Both recover toward the configured ceiling when the link stabilizes.
- Frame-gap detection and automatic keyframe recovery when frames go missing or fail to decode.
- Remote pointer, button, scroll, text, and key forwarding, gated by host consent plus Accessibility trust.
- On-device JPEG preview frames for the local live preview only, live capture/encode stats, and a clean engine stop/restart lifecycle.
- Guarded CGEvent injection that requires Accessibility trust and explicit arming, checks trust on every event, and releases held buttons when disarmed.
- Always-on-top and native fullscreen.
- Local persisted workspace state; no telemetry.

### Honest state / not yet implemented

- **No accounts yet.** The first screen creates a local profile that never leaves this Mac. Friends, Teams, requests, roles and shared computer libraries arrive with the Phase 5 service; those screens are explicit placeholders. Anything shown as "planned" or "coming soon" in Host/Settings is disabled rather than simulated.
- **No demo/simulated sessions.** Computers lists only real recent connections from actual Direct sessions; there is no fake library, latency, packet loss or desktop scene.
- **No audio, file transfer, virtual displays or HDR yet.** Their settings are disabled/planned until the corresponding phase lands. Controller passthrough is implemented end to end (client Gamepad API → encrypted control channel → host virtual HID gamepad), but macOS only lets a process create a virtual HID device when it is signed with the restricted `com.apple.developer.hid.virtual.device` entitlement; an unsigned build reports that in **Host → Advanced** instead of silently dropping input.
- **Browser fallback** still exists for development: it shows desktop-only notices for capture, permissions, hosting, nearby discovery and real sessions instead of faking them.

## Live preview

Open **Host → Start live preview** in the native app. ParsecPlus asks for Screen Recording access if it is missing; macOS requires the app to be relaunched after that grant before capture can start. Frames stay on the Mac — nothing is sent over a network.

The preview runs at 12 fps and up to 1280 px wide, so the HUD reflects real capture and HEVC encode timings without saturating the session. Ending the preview stops the engine and clears its state. In the browser fallback the same action shows a desktop-only notice instead of starting capture.

The input backend is wired only to direct remote sessions. Local preview never arms input: injected CGEvents would be captured by ScreenCaptureKit and loop back into the preview.

## Remote video rendering

Direct sessions no longer turn video into images. VideoToolbox decodes each HEVC frame into tightly packed BGRA, which is uploaded into a three-texture Metal ring and presented by a dedicated render thread on a `CAMetalLayer`. That layer lives in a native `NSView` inserted directly below the WebView at the session stage's bounds, so the HTML HUD, stats, and menus stay on top and fully interactive. The renderer keeps only the newest pending frame, drops stale ones under load, letterboxes to the stream's aspect ratio, and sizes its drawable in device pixels for the window's backing scale.

The page goes transparent only while the native surface is active: the `html`, `body`, app shell, session, and stage stop painting, while the titlebar, HUD, and notices keep their own backgrounds. Window resizes, fullscreen changes, and display scale changes re-sync the surface bounds. Detaching joins the render thread and removes the view, so disconnecting leaves no native layer behind.

Current limits: the host still caps the requested refresh rate at 60 fps. Frames are decoded to CPU BGRA before upload, with a zero-copy IOSurface path as future work. WebRTC is enabled for direct sessions only; ICE gathers host candidates plus any configured STUN/TURN servers, and sessions that cannot establish UDP fall back to the Noise-encrypted TCP path.

## Nearby, direct, and relay sessions

A session connects two ParsecPlus apps over the local network or through a relay. Direct sessions first try an authenticated WebRTC path over UDP and fall back to the Noise-encrypted TCP session; relay sessions always use TCP, because the relay forwards opaque bytes rather than offering a direct path.

1. On the host Mac, remote access starts automatically when the local profile is created. ParsecPlus binds TCP 39200, advertises the host over Bonjour, and shows a direct address plus a single-use invite code. If **Network → Relay invite** is on and a relay address is set, the host also joins a relay room and shows its invite.
2. On the client Mac, open **Connect directly** from Computers. The **Nearby** tab lists hosts advertising on the network; select one or enter an `address:port` manually. The **Relay invite** tab accepts the full invite from the host.
3. Enter the pairing code and connect. The host sees an incoming-request card over any screen with the peer address and Hello identity (owner and device name), a 15-second countdown, and **Decline** / **Accept** actions. The client waits up to 20 seconds for the session greeting and shows a “waiting for approval” note after 6 seconds.
4. On accept, the host completes transport negotiation, opens capture and the encoder, and sends its greeting on the selected transport. The client shows the host's encrypted video. The Host screen reports the connected peer plus live frame rate, actual bitrate, and the adaptive target.

### WebRTC transport

Direct sessions negotiate a UDP path before the host starts capture:

1. After the Noise handshake and `Hello`, the client creates an SDP offer and sends it to the host as an encrypted control message. The host answers with its SDP, or explicitly declines WebRTC.
2. Both sides apply the descriptions. The client's offer carries a receive-only HEVC m-line and the FlexFEC repair codec; the host attaches its video track before creating the answer, so the answer carries the sending m-line, the video SSRC, and the `FEC-FR` association. The client opens the `parsecplus-control` data channel (ordered, reliable) for input, telemetry, keyframe requests, and commands.
3. Once the control channel opens, the client tells the host which transport it will use, and the host sends `Welcome` on that transport. If either side cannot establish WebRTC, the host sends a fallback control message and the session continues over the Noise-encrypted TCP channel.
4. Video leaves as HEVC RTP packets ([RFC 7798](https://datatracker.ietf.org/doc/html/rfc7798)) on the media track, not as SCTP messages. The host's estimator consumes TWCC feedback, sizes the encoder, and the pacer meters packets through a bounded queue; NACK retransmits individual losses and FlexFEC-03 repairs a block of ten media packets with two repair packets. The client reassembles access units by extended RTP sequence number, waits 80 ms for a hole before declaring loss, and then emits only IRAP access units until a keyframe restores the reference chain.

ICE gathers local interface candidates by default. With STUN or TURN configured under **Settings → Network**, it also gathers server-reflexive and relay candidates, which is what lets two Macs on different networks punch a direct UDP path; without them an internet session falls back to the encrypted TCP channel. The host can pin the media UDP port so a router forward is deterministic, and **Host → Network → Detect public address** probes that port to show the public IP and confirm the NAT mapping. Relay sessions stay on TCP by design, because the relay forwards opaque bytes rather than offering a direct path. See [TEST_GUIDE.md](TEST_GUIDE.md) for the full two-Mac procedure. DTLS encrypts the RTP media (SRTP) and the control channel (DTLS/SCTP), and the DTLS fingerprints travel inside the SDP that only a paired peer can read or inject, so the WebRTC path inherits the pairing code's authentication.

**Rate control.** On the RTP path the host runs a delay-based estimator fed by transport-wide feedback: it retargets the running encoder between 2 Mbps and the configured ceiling, and the pacer drains at a multiple of the estimate so a keyframe burst does not become a standing queue. A full pacer queue drops packets rather than growing latency, and NACK and FEC recover them; the queue is bounded to roughly a tenth of a second at the opening rate. On the TCP fallback the previous loop still applies: each second the client sends an encrypted window with frames received, arrival jitter, dropped frames, and RTT; the host cuts the HEVC target in 20% steps down to 2 Mbps when the link falls behind, raises it in smaller steps after three healthy windows, and cuts immediately on a blocking write. The configured **Maximum bitrate** is the ceiling on both paths. The session performance panel shows the current target, ceiling, and jitter on the client, and the Host screen shows the adaptive target on the sending side. When frames are missing or fail to decode, the client requests an IDR frame immediately (throttled to one request per second) so playback can recover without waiting for the periodic keyframe.

### Relay route

The relay is a standalone service in `relay/`. It performs a short newline-delimited JSON rendezvous (`host`/`client` join, `waiting`/`ready`/`error` reply), then splices the two sockets together without parsing the stream.

- Invite format: `relay.example.com:39210#<32 hex room token>`.
- The room token is a 128-bit routing capability, not a trust credential. Authentication is still the Noise XX handshake keyed by the pairing code, followed by the host approval gate.
- The relay never receives the pairing code, Noise keys, video, or input. It sees the room token and ciphertext.
- Rooms are single-host/single-client, expire after 10 minutes, and are capped at 1024 concurrent rooms. The default relay port is TCP 39210.
- The host keeps one room for the hosting run and re-registers it after each session. Changing the relay address or toggle applies the next time hosting starts.

Host requirements:

- **Screen Recording** for capture. macOS prompts the first time hosting or live preview starts, and the app must be relaunched after the grant.
- **Network reachability**: allow incoming connections for ParsecPlus in the macOS firewall. A direct internet session needs inbound TCP 39200 for signaling/fallback and inbound UDP on the configured media port (default 39201) forwarded to the host Mac; the client needs only outbound access. Relay sessions need the relay's TCP port (39210 by default), and TURN (optional) needs outbound UDP/TCP to the TURN server. macOS also asks for Local Network access the first time ParsecPlus advertises or browses Bonjour services.
- **Accessibility** plus the **Allow remote input** toggle for input forwarding. Input stays off by default, and clients are view-only until the host enables it.

Up to eight direct viewers can be connected at once; relay rooms still serialize one viewer at a time because the relay forwards opaque bytes. Each direct connection is approved separately and gets its own session thread and encoder; the presence dock shows who is waiting, who is connected, and per-viewer access. The invite code stays valid for saved reconnect entries until you press **New code** to rotate it. Remote access is controlled by the Host toggle and is off at launch. Ending a session releases any held mouse buttons, disarms that viewer's input, and clears its consent.

**Require approval** is on by default. Requests stay pending for two minutes and appear in the cross-screen presence dock; clicking the avatar opens Accept/Reject with the device name, owner and peer address. Turning approval off makes the invite code the only gate: any client presenting the current invite code is admitted directly. The gate runs after the Noise handshake, so a declined or expired request never starts capture. Per-viewer keyboard/mouse/gamepad switches take effect immediately, and Disconnect sends the viewer a clean Bye. The gamepad switch gates controller snapshots from that viewer; the host turns them into a virtual HID gamepad when the app is signed with the virtual-HID entitlement. The browser fallback shows a desktop-only notice instead of simulating an approval card.

Direct and relay sessions carry video and input. The RTP path implements delay-based congestion control, pacing, NACK and FlexFEC; resolution scaling and dynamic quality ladders are not implemented. With no STUN/TURN configured, WebRTC sees only local candidates and internet sessions fall back to TCP; STUN/TURN and a forwardable media port are configurable, but TURN relaying, a startup bandwidth probe, and a real two-Mac internet measurement have not been exercised in this workspace. Concurrent viewers each run their own ScreenCaptureKit stream and HEVC encoder, which costs CPU per viewer; a shared capture/encode fan-out and per-viewer rate control are still to come. Controller passthrough is implemented: the client polls the W3C Gamepad API, sends only changed snapshots over the encrypted control channel, and the host injects them through a virtual HID gamepad, gated by the host toggle and the per-viewer gamepad switch. Clipboard text sync is implemented (plain text only, host Advanced toggle); audio, file transfer, all-display sharing and QUIC are still to come; client-side display switching now works without reconnecting. The direct, relay, WebRTC, RTP and adaptive-control paths are covered by Rust tests, including a live loopback ICE/DTLS session that carries HEVC access units over RTP, one that drops media packets with NACK generation disabled and recovers them through the negotiated FlexFEC flow alone, a full Noise handshake over the relay, and encrypted stream-feedback round trips. They have not been runtime-tested across two physical Macs or a real NAT/relay deployment in this workspace.

## Features

- **Local profile** — "Set up this Mac" creates a display name and local id; no email, password, demo workspace, plan or billing. Everything stays on-device.
- **Computers** — real saved connections (host name, address, last connected, transport) that remember the invite code, so **Reconnect** sends the request without retyping a code. Equal **Direct connect** and **Host this computer** actions; no seeded machines/filters.
- **Host** — flat, clearly sectioned Host/Access/Video/Network/Advanced settings; an explicit remote-access toggle; TCC permission flows; reusable invite codes and direct connection strings; real display picker from ScreenCaptureKit; aspect-matched capture; per-viewer presence dock with accept/reject, keyboard/mouse toggles and disconnect; relay and STUN/TURN configuration; media port; public-address probe; adaptive bitrate; clipboard text sync.
- **Friends / Teams** — honest placeholders explaining that accounts and the directory are the next service phase; no seeded people, requests or teams.
- **Arcade** — a coming-soon section under Teams with a neon arcade-cabinet placeholder; no functionality claimed yet.
- **Reachability** — Bonjour presence while hosting, nearby-host scanning, a self-hostable relay route, WebRTC-over-UDP for direct sessions with TCP fallback, up to eight concurrent direct viewers with a cross-screen presence dock, configurable STUN/TURN, a pinned media UDP port with public-address and port-forward detection, Tailscale address detection with a one-string join code, adaptive/transport-aware rate control with RTT/delivery trimming, live relay state, and frame-gap keyframe recovery.
- **Session shell** — live preview or real remote HEVC video only, auto-hiding HUD, performance panel with real adaptive rate and jitter, session menu, fullscreen, and disconnect. No ambient/simulated desktop; an empty state offers Start live preview / Connect directly.
- **Settings** — real local profile name, four accent themes, density/motion/always-on-top, hotkeys reference, STUN/TURN, relay/media port, clipboard, permissions, advanced reset, About. Unimplemented audio/decoder/bandwidth controls are disabled/planned rather than simulated.
- **Search palette** — `⌘K` search across destinations, hotkeys and real recent connections.
- **Accessibility** — focus-visible styling, semantic dialogs with focus traps, reduced-motion support, and ARIA labels on custom controls.

## Getting started

The packaged build is ad-hoc signed, not notarized. If macOS refuses to open a transferred copy
(“can’t be opened” or “damaged”), drag it to Applications, then run
`xattr -dr com.apple.quarantine /Applications/ParsecPlus.app` and
`codesign --force --deep --sign - /Applications/ParsecPlus.app`, or use
**System Settings → Privacy & Security → Open Anyway** after one failed launch attempt. Details in
[TEST_GUIDE.md](TEST_GUIDE.md).

Requirements: Node.js 20.19+ (or 22.12+), npm, and — for native development — macOS 12.3+, a Rust toolchain, and Xcode Command Line Tools. ScreenCaptureKit is used for capture, so the native app targets macOS 12.3 and later.

```bash
npm install
npm run dev          # web preview at http://localhost:1420
npm run tauri dev    # native macOS app
```

Quality and production builds:

```bash
npm run check        # TypeScript type checking
npm run build        # type check + production web bundle
npm run tauri build  # packaged native app
cargo test --manifest-path src-tauri/Cargo.toml  # Rust unit and integration tests
cargo test --manifest-path relay/Cargo.toml      # relay tests
```

### Self-hosted relay

The relay is a separate Rust crate with no Tauri or macOS dependency:

```bash
cargo run --manifest-path relay/Cargo.toml --release -- --bind 0.0.0.0 --port 39210
```

Run it on a machine both peers can reach, then enter its `host:port` under **Host → Network → Relay address** and restart hosting. Both Macs need TCP access to that port. For a local smoke test, bind to `127.0.0.1` instead.

### Internet sessions

**Tailscale path (least setup).** Join both Macs to the same Tailscale network, start hosting, copy the join code from **Host → Network → Share with a friend**, and have the friend paste it under **Connect directly → Join code**. The host approves the request and the stream starts; no port forwarding or STUN is needed because Tailscale supplies the address. See [TEST_GUIDE.md](TEST_GUIDE.md).

**Direct internet path.** Without an overlay, give both Macs the same STUN server under **Settings → Network → NAT traversal (STUN/TURN)**, forward **TCP 39200** and the host's **media UDP port** (default 39201) on the host's router, and use **Host → Network → Detect public address** to check the public IP and mapping. Then connect to `PUBLIC_IP:39200` with the pairing code. Empty STUN/TURN fields mean no third party is contacted, and the session falls back to the encrypted TCP stream when UDP cannot be established. The complete procedure, including TURN, relay, Tailscale fallbacks and troubleshooting, is in [TEST_GUIDE.md](TEST_GUIDE.md). If you only have one Mac, [LOCAL_TESTING.md](LOCAL_TESTING.md) shows how to run two app copies locally as host and client.

The native app declares `NSLocalNetworkUsageDescription` and the `_parsecplus._tcp` Bonjour service in `src-tauri/Info.plist`. Approve the macOS Local Network prompt the first time ParsecPlus advertises or scans for hosts.

To use live preview or host a direct session, allow ParsecPlus under **System Settings → Privacy & Security → Screen Recording**, then relaunch the app. Capture starts only when you press **Start live preview** on the Host screen or turn hosting on. To forward remote input, also allow **Accessibility** and enable **Allow remote input**; ParsecPlus never grants these permissions for you.

The native app icon set is generated from the original source at `src-tauri/icons/source/app-icon.svg` using the Tauri icon pipeline.

## Architecture

```
src/
  components/   Shared UI primitives and product components
  screens/      Route-level screens (auth, computers, friends, host, teams, settings, session)
  lib/          Store, seed data, native bridge, types, utilities
  styles/       Design tokens, base styles, component styles, shell and screen CSS
src-tauri/
  src/lib.rs       Tauri entry point, commands, registration, and macOS permission checks
  src/capture.rs   ScreenCaptureKit capture + VideoToolbox HEVC pipeline
  src/engine.rs    Live preview engine and capture-frame events
  src/preview.rs   Bounded JPEG preview encoding for the local live preview
  src/host.rs      Direct host listener, encrypted session loop, pairing-code lifecycle
  src/client.rs    Direct client handshake, hardware decode, and input command queue
  src/video.rs     Native Metal video surface, frame handoff, and bounds management
  src/adaptive.rs  Host rate controller and client link-health monitor
  src/reachability.rs  Bonjour discovery, relay client, and relay invite handling
  src/transport.rs Noise handshake, encrypted framing, frame chunking and reassembly
  src/webrtc_transport.rs  WebRTC endpoints, SDP negotiation, RTP video, control channel
  src/congestion.rs        Delay-based send-rate estimator and encoder sizing
  src/rtp_video.rs         HEVC RTP access-unit reassembly and loss/keyframe gating
  src/public_address.rs    STUN binding probe for the host's public address and port mapping
  src/network.rs           Usable IPv4 addresses and Tailscale network detection
  src/join_code.rs         One-string join code: host address + port + pairing code
  src/input.rs     Armed, Accessibility-gated CGEvent input backend
  capabilities/    Tauri permissions
  icons/           Generated app icons + original SVG source
relay/
  src/lib.rs       Room-based rendezvous relay with opaque byte forwarding
  src/main.rs      Relay CLI
```

- **State** lives in a typed React context (`src/lib/store.tsx`) and is mirrored to `localStorage` under `parsecplus.data.v1`. The store exposes explicit actions for every product mutation.
- **Native bridge** (`src/lib/native.ts`) probes for the Tauri runtime and falls back to deterministic browser behavior, so every screen is testable in a normal browser.
- **Design system** is defined with CSS custom properties in `src/styles/tokens.css`; accent themes and computer color tones are token-driven. Inter Variable is bundled locally.
- **UI primitives** are original and shared across screens: buttons, modal, popover, fields, toggle, segmented control, slider, toast, avatars, ring, sparkline, keyboard hints, and the ParsecPlus logo mark.
- **Reachability** (`src/reachability.rs`) advertises hosts over Bonjour while hosting, browses for nearby hosts on demand, and implements the relay join/pair client. The relay service is a separate crate so it can be deployed independently of the macOS app.
- **WebRTC transport** (`src/webrtc_transport.rs`) wraps `webrtc-rs` behind the same synchronous session interface as the TCP channel. A process-wide Tokio bridge keeps the session threads synchronous while the peer-connection driver runs on webrtc's reactor pool. Video is an RTP track packetized by the HEVC payloader; control is one data channel. Signaling (SDP offer/answer, transport choice, fallback) is exchanged as control messages inside the Noise channel, so ICE candidates and DTLS fingerprints are authenticated by the pairing code.
- **Join codes** (`src/network.rs`, `src/join_code.rs`) list usable interface addresses, mark the ones on a Tailscale tailnet, and pack a chosen address, the TCP port, and the current pairing code into a `PC1-…` string. The string is a single-use secret: the friend pastes it, the client unpacks it, and the ordinary Noise handshake plus host approval gate do the authorization.
- **Internet reachability** (`src/public_address.rs`) sends one STUN binding request from the media port and reports the mapped address, which both tells the host what to share and validates the router forward. `src/webrtc_transport.rs` accepts STUN/TURN servers and a pinned UDP port for either role.
- **Rate control** is split by transport. `src/adaptive.rs` is the pure, unit-tested TCP-fallback controller: a host rate controller that steps an encoder target between 2 Mbps and the configured ceiling, and a client link monitor that derives jitter and frame gaps from arrival timing. `src/congestion.rs` is the RTP path's delay-based estimator: it consumes TWCC reports, tracks standing queue delay and loss, sizes the encoder, and publishes the target the pacer drains at. `src/rtp_video.rs` turns HEVC RTP packets back into access units and gates the stream on keyframes after a loss.

### Native commands

| Command | Purpose |
| --- | --- |
| `system_info` | Hostname, OS, version, architecture, model, chip, app version |
| `permission_status` | Screen Recording, Accessibility, Input Monitoring |
| `request_permission` | Requests a macOS permission or opens the relevant privacy pane |
| `open_privacy_settings` | Deep-links to the correct System Settings privacy section |
| `generate_pair_code` | Generates a single-use `XXXX-XXXX` pairing code |
| `native_discover_hosts` | Browses Bonjour for nearby hosts; returns name, address, port, platform, and version |
| `native_discover_public_address` | Sends one STUN binding request (optionally from the media port) and returns the mapped public address, port, and local socket |
| `native_network_addresses` | Lists usable IPv4 interface addresses and detects the Tailscale tailnet, for the host's address picker |
| `native_make_join_code` | Packs an address, TCP port, and pairing code into the single-use `PC1-…` join code |
| `native_parse_join_code` | Validates and unpacks a join code into address, port, and pairing code |
| `native_engine_start` | Starts ScreenCaptureKit capture and hardware HEVC encoding; returns the active session summary |
| `native_engine_stop` | Stops the capture thread and releases encoder and stream resources |
| `native_engine_status` | Reports whether capture is running and the current engine summary |
| `native_input_arm` | Explicitly arms or disarms native input; arming requires Accessibility trust |
| `native_input_status` | Reports arming state, Accessibility trust, and platform support |
| `native_input_pointer` | Moves the pointer using normalized display coordinates |
| `native_input_button` | Presses or releases left, right, or middle mouse buttons |
| `native_input_scroll` | Scrolls by pixel deltas |
| `native_input_text` | Types Unicode text through keyboard events |
| `native_host_start` | Starts the encrypted direct host listener with capture options, STUN/TURN ICE servers, and a media UDP port; optionally joins a relay room and enables adaptive bitrate |
| `native_host_stop` | Stops hosting, ends the active session, closes the relay room, and releases input |
| `native_host_status` | Reports running/streaming state, client, pairing summary, and last error |
| `native_host_rotate_code` | Rotates the live single-use pairing code while hosting |
| `native_host_set_input` | Grants or revokes remote input consent for the active session |
| `native_host_set_approval` | Turns the per-connection approval gate on or off; disabling it admits a pending request |
| `native_host_respond` | Accepts or declines one waiting request by peer id |
| `native_host_set_peer_permissions` | Enables or disables keyboard, mouse and gamepad access for one connected viewer |
| `native_host_kick` | Disconnects one viewer by peer id |
| `native_display_size` | Returns the main display's logical size for the aspect-matched capture preset |
| `native_client_connect` | Connects by direct address or relay invite with a pairing code and STUN/TURN ICE servers; returns the session summary |
| `native_client_disconnect` | Ends the direct client session and joins its worker |
| `native_client_status` | Reports client connection state and the current summary |
| `native_client_input` | Queues a remote pointer, button, scroll, text, or key event |
| `native_client_request_keyframe` | Asks the host for an immediate IDR frame |
| `native_video_attach` | Creates the Metal surface under the WebView and starts the render thread for the given CSS bounds |
| `native_video_bounds` | Repositions and rescales the Metal surface after resize or fullscreen changes |
| `native_video_size` | Returns the current decoded stream dimensions, or `null` before the first frame |
| `native_video_detach` | Stops the render thread and removes the native video view |

The live preview consumes `native-capture:frame`, `native-capture:stats`, and `native-capture:stopped`. Direct sessions add `native-host:status`, `native-host:stats` (frame rate, actual bitrate, adaptive target and ceiling, delivery ratio, jitter), `native-client:video` (stream dimensions, emitted when they change), `native-video:error` (the renderer stopped), `native-client:stats` (including target rate, jitter, and dropped-frame count), `native-client:input`, and `native-client:stopped`. Stream feedback and rate changes travel as control messages inside the Noise-encrypted channel, so the relay and network see only ciphertext. Host input commands are used only by the remote session path; the local preview never arms input.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| `⌘K` | Search destinations, hotkeys, and recent connections |
| `⌘,` | Open Settings |
| `Esc` | Open/close the session menu |
| `⌘⇧S` | Toggle session performance stats |
| `⌘⌃F` | Toggle fullscreen |
| `⌘⇧D` | Disconnect the active session |

In a direct session, keyboard input is forwarded to the host except for these local shortcuts, which stay on the client. Remote input must be enabled by the host.

## Roadmap

The full, freshly re-laid plan lives in [ROADMAP.md](ROADMAP.md). The short version:
**Phase 4 — session essentials**: clipboard text sync implemented; real multi-monitor host
selection (host side) implemented; next are shared capture/encode fan-out with per-viewer rate
control, system audio, and gamepad input. **Phase 5 — accounts, friends and persistent presence** via a
self-hostable service. **Phase 6 — cross-platform (Windows/Linux), notarization, auto-update.**
**Phase 7 — parity and hardening** (virtual displays, HDR, file transfer, bandwidth probe,
permission profiles).


- **Phase 2 — Capture, encode, input (implemented on macOS).** ScreenCaptureKit capture, VideoToolbox HEVC encoding, hardware cursor, local preview, capture/encode telemetry, and a guarded CGEvent input backend. H.264 and non-macOS hosts to follow.
- **Phase 3 — Direct transport (implemented on macOS).** Authenticated, encrypted sessions between two ParsecPlus apps, hardware HEVC decode, a native Metal video surface at full stream resolution, real RTT/decode telemetry, and consent-gated input forwarding.
- **Phase 3.5 — Reachability foundation (implemented).** Bonjour host presence and nearby discovery, plus a self-hostable rendezvous relay that only forwards ciphertext. Still to come: NAT traversal (STUN/TURN) and multi-relay failover.
- **Phase 3.6 — Adaptive streaming (implemented).** Encrypted client health feedback, host-side encoder retargeting, blocking-write congestion response, and keyframe recovery for missing or undecodable frames.
- **Phase 3.7 — WebRTC transport (implemented on macOS).** SDP negotiation inside the Noise channel, ICE/UDP with a control data channel, automatic TCP fallback, and relay sessions kept on TCP. Still to come: STUN/TURN and removal of the host's 60 fps ceiling.
- **Phase 3.8 — RTP media path (implemented on macOS).** HEVC RTP packetization, transport-wide feedback, a delay-based congestion estimator, a paced send queue, NACK retransmission and FlexFEC-03 repair, with the ordered control channel kept on SCTP. Still to come: dynamic resolution scaling and a real two-Mac lossy-path measurement.
- **Phase 3.9 — internet reachability (implemented).** Configurable STUN/TURN servers for both roles, a pinned host media UDP port, and a STUN public-address/port-forward probe surfaced in the Host screen, with a documented two-Mac test procedure. Still to come: a startup bandwidth probe, TURN support folded into the self-hosted relay, and measured internet sessions.
- **Phase 3.10 — Tailscale join codes (implemented).** Interface/Tailscale address detection and a single-string join code so a friend can connect and be approved without port forwarding, STUN or a relay. Still to come: measured cross-city sessions and a persistent friend list (needs Phase 5 accounts).
- **Phase 3.11 — multi-viewer presence (implemented).** Concurrent direct viewers, two-minute persistent requests, a presence dock with per-viewer accept/reject, keyboard/mouse/gamepad switches and disconnect, categorised host settings, aspect-matched display capture and latency tuning (12 Mbps default, session-long GCC base delay, shallower pacer/video queues, RTT/loss trimming). Still to come: shared capture/encode fan-out, per-viewer rate policy, a physical-controller verification once the virtual-HID entitlement is available, and measured cross-city latency.
- **Phase 4 — session essentials (in progress).** Shared capture/encode fan-out, audio, clipboard text sync, gamepad input, and multi-monitor playback. Clipboard text sync (macOS pasteboard, opt-in, 64 KiB, echo-suppressed) is implemented in the backend; the rest follows.
- **Phase 5 — Accounts, friends and service layer.** Accounts, account-backed device presence, pairing, friends, teams, roles, and shared-computer access backed by a real API and end-to-end encrypted session setup.
- **Phase 5 — Product polish.** Multi-display sessions, clipboard and file transfer, audio streaming, gamepad passthrough, per-app profiles, configurable hotkeys, update delivery, and billing.

## Privacy

This preview stores workspace data only in the browser or app WebView on your machine. It makes no analytics or telemetry requests. Direct sessions connect peer to peer; with the STUN/TURN fields empty, ICE gathers local interface candidates only and no third-party service is contacted. If you configure STUN or TURN, that server sees your IP and the ICE traffic (a public STUN server also sees the media port mapping); TURN relays DTLS/SRTP-encrypted media without being able to read it. A join code contains the current single-use pairing code and the host's LAN/overlay address, so share it only with the intended friend; it stops working after that session. Tailscale traffic is encrypted by the overlay before ParsecPlus's own Noise/DTLS layers. Relay sessions send ciphertext through the relay you configure yourself. That relay sees only a room token and encrypted bytes, never the pairing code, Noise keys, video, or input, and no data is sent to a ParsecPlus-operated service (there isn't one). Stream health feedback is sent only to the paired host inside the encrypted channel. While hosting, ParsecPlus publishes a Bonjour advertisement containing the host name, platform, version, and a stable non-secret device ID; it never advertises the pairing code. Removing local data resets the workspace; `Settings → Advanced` includes a local reset confirmation flow.

## Disclaimer

ParsecPlus is an independent clean-room project. It is not affiliated with, endorsed by, or derived from Parsec Technologies. All interface design and code are original.
