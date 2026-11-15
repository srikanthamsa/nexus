# ParsecPlus roadmap

A re-laid plan after the first real cross-city session (Vizag host ↔ Mumbai/Bengaluru client over
Tailscale) and the Phase 3.11 multi-viewer/presence batch.

## Where the app is today

- macOS host and macOS client, end-to-end encrypted.
- **No demo data**: the app is driven by a real local profile, real recent connections, real host
  status/viewers, and honest empty states. Friends/Teams are account-phase placeholders; Arcade is
  an explicit coming-soon section; unimplemented settings are disabled/planned rather than faked.
- Direct LAN/Tailscale internet sessions with an optional single-use join code
  (`PC1-…`), plus a self-hostable ciphertext relay and configurable STUN/TURN.
- WebRTC/SRTP+RTP media path: HEVC RTP, TWCC feedback, GCC-style congestion control, pacer,
  NACK, FlexFEC, TCP fallback, automatic keyframe recovery.
- Multi-viewer hosting (up to 8 direct viewers), a persistent presence dock with per-viewer
  keyboard/mouse/gamepad switches and disconnect, and two-minute pending requests.
- Screenshot-quality capture/encode/decode with hardware HEVC; a "Match display" aspect-correct
  preset; hardware cursor; input forwarding with an explicit arm + Accessibility gate.
- Clipboard text sync and client-side multi-display switching are implemented; controller
  passthrough code exists but is blocked by the macOS virtual-HID entitlement (see 4.5).
- No accounts, audio, file transfer, Windows support, notarization or auto-update yet.

## Phase 4 — Session essentials (next, current work)

Goal: make a session feel like a complete remote desktop before adding account infrastructure.
Ordered by user-visible value per unit of risk.

### 4.1 Clipboard text sync (in progress)
- macOS pasteboard bridge (`clipboard.rs`) with change-count polling (500 ms), 64 KiB cap,
  echo suppression, opt-in host setting.
- Host/session loop forwards text both ways; only plain text, no images/rich data. Settings label and
  first-use pasteboard note are in the app.
- Next: live test with a second viewer; confirm macOS pasteboard prompts are acceptable or default it off.

### 4.2 Multi-monitor host selection and client-side switching (implemented)
- `display_info.rs` lists every ScreenCaptureKit shareable display (`native_list_displays`), the
  Host → Video picker writes `HostSettings.displayIndex`, and hosting/live preview capture the
  chosen display with aspect-matched dimensions.
- Remote access is an explicit Host toggle (off at launch). Joining always goes through an
  explicit request/approval or a pasted invite/direct-connection string; the invite code stays
  valid for saved reconnect entries until manually rotated.
- Client-side switching is implemented: Welcome advertises the host's shareable displays, the
  session menu lists them, `RequestDisplay` swaps the host capture pipeline without reconnecting,
  and `DisplayChanged` updates the live resolution. Failed switches keep the old display. The list
  is sampled at connect time; displays added later need a reconnect. Input geometry is already
  normalised to the active stream.
- Next: sharing "all displays" and hotkey-based switching.

### 4.3 Shared capture/encode fan-out, per-viewer rate policy (next)
- Today every viewer opens its own ScreenCaptureKit stream and HEVC encoder; CPU scales with
  viewers. Move capture+encode into one broadcaster that fans encoded access units out to each
  viewer's transport.
- Per-viewer congestion state, keyframe-on-join, and a shared ceiling chosen from the slowest
  viewer (or independent simulcast later). This is the next implementation pass; it is isolated
  from the current UI build so a session cannot regress while it lands.

### 4.4 Audio
- System-audio capture alongside video, Opus over the WebRTC audio track, client playback with
  device selection and mute/volume, audio-only fallback, and latency/clock alignment.
- Host setting: share system audio; client setting: play remote audio. Settings currently hide
  audio controls until this lands.

### 4.5 Gamepad and input depth (controller passthrough implemented)
- Client-side W3C Gamepad API polling, changed-snapshot forwarding over the encrypted control
  channel, and host-side virtual HID gamepad injection are implemented. The host toggle and the
  per-viewer gamepad switch are real and enforced.
- The virtual pad now publishes an Xbox-style HID report (buttons first, hat, trigger axes, signed
  16-bit sticks) instead of a generic 6-axis layout, so macOS/SDL/Steam label LB/RB, LS/RS,
  Back/Start and Guide by their correct Xbox positions instead of shifting them. Report mapping
  tests cover the exact labels from the friend-controller feedback.
- Native macOS now prefers **OpenJoystickDriver** when available: ParsecPlus connects to its
  authenticated local endpoint, publishes one pad per viewer (four remote pads; the host's own
  physical controller is player 1), and does not create HID devices itself. This avoids SIP/AMFI
  changes and works on stock macOS. The in-process virtual-HID backend remains the fallback.
- **Platform gate / verification:** macOS requires the restricted
  `com.apple.developer.hid.virtual.device` entitlement to create a virtual HID device. The current
  ad-hoc-signed build has no entitlements, so the host cannot create the virtual pad and surfaces
  that in Host → Advanced. The report-mapping and protocol paths are unit-tested, but no physical
  controller has been verified end to end. Signing with the entitlement is required before a
  hardware test; injection then needs no code changes.
- Next: native GameController.framework polling (so input works when the webview is not focused),
  multiple simultaneous controllers per viewer, relative mouse, pen/tablet pressure, and
  clipboard/audio hotkeys.

## Phase 5 — Accounts, friends and persistent presence

Goal: remove the one-time join-code ritual for people you trust.
- Self-hostable service crate (like the relay) with: device registration, account login
  (password + device keys), friend requests, accepted-friend directory, presence, and
  short-lived connection tickets.
- Client UI: real Friends list with online state and one-click connect; host-side trusted-device
  list and per-friend permission profiles; optional "always allow this device".
- Security: device identity keys separate from the single-use pairing code; the service never
  sees media keys or plaintext; every session still ends in a Noise handshake + host approval
  unless a trusted profile explicitly skips it.
- Keep the code-only path working for guests and self-hosters.

## Phase 6 — Cross-platform and distribution

- Windows host + client (DXGI Desktop Duplication / Windows Graphics Capture, Media Foundation
  H.264/HEVC, WASAPI audio, SendInput), Linux client/host on X11/Wayland where practical.
- H.264 fallback for machines without HEVC encode/decode.
- Universal macOS build, Developer ID signing + notarization, auto-update, crash reporting,
  CI packaging, and a stable TCC identity so Screen Recording grants survive upgrades.

## Phase 7 — Parity and hardening

- Virtual/disconnected displays, HDR, 120 Hz capture where the panel allows it.
- File transfer, drag-and-drop, printer/redirected clipboard formats.
- Session recording, statistics history, bandwidth probe at connect, relay/TURN autoconfig.
- Permission profiles (view-only, control, file transfer), audit log, rate limits, fuzzing of
  the Noise/control/relay parsers, multi-tenant relay hardening.

## Near-term engineering chores

- Replace the ad-hoc signing with a stable development certificate (TCC grants currently reset on
  every rebuild).
- ~~Add an explicit host preflight for Screen Recording~~ (done: the host now rejects the connection
  with an actionable message before showing the approval card if the grant is missing).
- Keep the guided test flow in [TEST_GUIDE.md](TEST_GUIDE.md) current with each phase.
