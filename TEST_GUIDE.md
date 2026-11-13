# ParsecPlus internet test guide

A concrete procedure for testing a real ParsecPlus session between two Macs on different
networks — for example Visakhapatnam ↔ Bengaluru or Mumbai. Run the same build on both Macs.

The app is macOS-only today (the host and client both use ScreenCaptureKit/VideoToolbox/Metal).
The build is ad-hoc signed, not notarized, so the friend's Mac needs the usual
right-click → **Open**, or **System Settings → Privacy & Security → Open Anyway**.

## Quick path with Tailscale (recommended for this setup)

You already run Tailscale and your friend is on the same tailnet, so no port forwarding and no
STUN/TURN server is needed: Tailscale puts both Macs on the same virtual network, and ParsecPlus can
generate a direct connection string from the host's Tailscale address.

1. Both Macs run the same ParsecPlus build and are joined to the Tailscale network. No account or
   login is needed: on first launch choose **Set up this Mac** and enter a display name — this is a
   local profile that never leaves the device (there is no demo workspace or seeded data). You can
   change the name later under **Settings → Account** so the host's approval card is unambiguous. Confirm the
   host's tailnet IP with `tailscale ip -4` (this Mac: `100.125.242.28`) and check
   `tailscale status` says `Running`. On first
   launch, allow ParsecPlus under **System Settings → Privacy & Security → Local Network** and, if
   macOS asks, allow it to accept incoming connections in the firewall.
2. **Host:** remote access is always on — no toggle. Open **Host**; the **Remote access** card
   shows **Live** and the listener address. Open **Host → Network → Share with a friend**; the card
   detects Tailscale, shows the tailnet/status, and preselects the Tailscale address. Copy either
   the **invite code** (8 characters) or the **direct connection** string (starts with `PC1-`).
3. Send one of them to your friend out of band. The invite code stays valid until you press **New code**, so a saved computer can reconnect without re-entering it.
4. **Friend:** Computers → **Connect directly** → **Invite code** → paste → **Send request**. The
   direct connection string carries the Tailscale address, TCP port `39200`, and the invite code.
5. You get the **Approve / Reject** request on the host; the request also stays in the presence dock for two minutes. Approve whenever you are ready.
6. The stream starts. If ICE connects over the Tailscale interface the transport is **WebRTC · UDP**
   (RTP video with GCC/FEC); otherwise the encrypted TCP fallback runs over Tailscale. Both work;
   WebRTC is the low-latency path.
7. The invite/direct-connection code stays valid for saved computers until you rotate it; the
   client stores it after the first session and offers a one-click **Reconnect** next time. The presence dock (bottom right, all screens) lists
   waiting requests and connected viewers.

No accounts yet: a persistent friend list needs the Phase 5 service layer. The invite code or
direct connection string is the secure, accountless equivalent today — one string, one request,
one approval, stream.

### Troubleshooting the Tailscale path

| Symptom | Check |
| --- | --- |
| Friend cannot reach the host | `tailscale status` shows `Running` on both and an IP in the same tailnet; `tailscale ping <host's Tailscale IP>` from the friend's Mac |
| Connect times out | macOS firewall: allow **ParsecPlus** incoming connections; hosting must be on when the friend connects |
| Approve card never appears | The friend's client got past TCP but the host approval gate is on; the card is over any screen — also check the code was copied whole |
| Transport says TCP | ICE over the Tailscale interface did not connect; the session still works over TCP. Re-run with both Macs on the same Tailscale network, and allow incoming connections |
| Join code rejected | It is single-use and rotates; copy the current code from the Host screen again |
| No Tailscale address in the picker | Start Tailscale first (`tailscale status` shows `Running`), then reopen the Host screen; the app looks for `tailscale` in PATH, `/usr/local/bin`, `/opt/homebrew/bin`, and the app bundle |

## What the session needs

| Path | Port | Purpose |
| --- | --- | --- |
| TCP (host inbound) | 39200 (default) | Noise-encrypted signaling, then the whole session if WebRTC cannot connect |
| UDP (host inbound) | 39201 (configurable, Host → Network) | HEVC RTP video, GCC congestion control, FEC and NACK when WebRTC connects |
| UDP (outbound only) | ephemeral | The client needs no port forwarding |

The port table below applies to Path B (port forwarding). On the Tailscale path only TCP `39200` over the Tailscale interface is needed, with no forwarding. Only the **host** needs forwarded ports and a reachable public address. The client works from any
ordinary network, including mobile hotspots.

There is no default STUN/TURN server: with the fields empty, no third party is contacted. For an
internet session, put a STUN server in **Settings → Network → NAT traversal (STUN/TURN)** on
**both** Macs, for example:

```
stun:stun.l.google.com:19302
```

A public STUN server sees your public IP and the port your NAT mapped; that is what lets ICE
punch UDP through both routers. Self-host one if you prefer not to leak that metadata to a third
party. TURN is only needed when direct UDP is impossible (symmetric NAT / carrier-grade NAT);
see the fallbacks below.

## Build and install

```bash
npm run tauri build
# output: src-tauri/target/release/bundle/dmg/ParsecPlus_0.1.0_aarch64.dmg
#         src-tauri/target/release/bundle/macos/ParsecPlus.app
```

Send the DMG to your friend.

### macOS Gatekeeper (ad-hoc build)

The build is ad-hoc signed, not notarized with an Apple Developer ID, so macOS blocks it after a
download with messages like **“The application ‘ParsecPlus’ can’t be opened”** or “damaged”.
Fix it once on each Mac:

1. Drag **ParsecPlus** into **Applications** first (do not run it from the mounted DMG).
2. Open **Terminal** (Applications → Utilities → Terminal) and paste:

```bash
xattr -dr com.apple.quarantine /Applications/ParsecPlus.app
codesign --force --deep --sign - /Applications/ParsecPlus.app
open /Applications/ParsecPlus.app
```

3. Answer the macOS prompts (Screen Recording, Local Network, incoming connections) and relaunch
   if the app asks for it.

Alternative without Terminal: after one failed launch, open **System Settings → Privacy &
Security**, scroll to Security, and click **Open Anyway** for ParsecPlus.

The current DMG is ad-hoc signed with a valid bundle signature, so either route works once the
quarantine attribute is gone. An older copy transferred before signing was fixed reports
**“The application ‘ParsecPlus’ can’t be opened”** and is repaired by the same two commands (the
`codesign` line re-signs it locally). Never run the app from the mounted DMG; always copy it to
Applications first.

On both machines:

1. Install **ParsecPlus**, launch it once, then open **Host** and grant **Screen Recording** when
   asked; macOS requires a relaunch after the grant before capture works. Because development
   builds are ad-hoc signed, **each rebuilt app counts as a new app to macOS**: if Screen Recording
   was already on for an older copy, remove ParsecPlus from
   **System Settings → Privacy & Security → Screen Recording** (select it, click −) and add the
   new `/Applications/ParsecPlus.app` back, then relaunch. Otherwise the host loses capture
   permission while a friend connects, and the client reports a closed WebRTC session the moment
   you approve.
2. For remote input, also grant **Accessibility**, enable **Allow remote input** while hosting,
   and keep the session armed. Input is off by default.
3. Allow ParsecPlus through the macOS firewall if prompted (incoming connections).
4. Optional sanity check on the same Wi-Fi first: host **Host → Network**, second Mac →
   **Connect directly** with the host's LAN address and code. If that fails, fix permissions
   before adding the internet hop.

## Path B — direct internet with port forwarding + STUN

Use this when Tailscale is not available. The host needs router access and a reachable public IP.

## Step 1 — Host (in Vizag)

1. **Settings → Network → NAT traversal (STUN/TURN)**: enter the STUN server (same value on both
   Macs). Leave TURN empty unless you have a TURN server.
2. **Host → Network**: set **Media UDP port** to `39201` (default). The **Internet test** card
   shows the two ports to forward: **TCP · signaling** and **UDP · video**.
3. On the router, forward:
   - TCP `39200` → this Mac's LAN IP, port `39200`
   - UDP `39201` → this Mac's LAN IP, port `39201`
4. Back in **Host → Network**, press **Detect public address** (with hosting stopped, so the
   probe can bind the media port). It reports the public IP and the UDP port the STUN server saw.
   - If the mapped UDP port is `39201`, the UDP forward matches and direct video can work.
   - If it reports a different port, forward that external UDP port to internal `39201` instead.
   - If discovery fails, port forwarding or the STUN setting is wrong; fix it before testing.
5. Remote access is already live. Share, out of band:
   - the **public IP** shown by the internet test, plus TCP port `39200`
   - the invite code (**single use**, rotates when a session starts and ends)

## Step 2 — Friend (in Bengaluru/Mumbai)

1. **Settings → Network → NAT traversal (STUN/TURN)**: same STUN server as the host. TURN fields
   only if the host gave you TURN credentials.
2. **Computers → Connect directly**, enter `PUBLIC_IP:39200` and the invite code, then
   **Send request**.
3. The host gets an approval card and the request also waits in the presence dock. Accept when
   ready; requests persist for two minutes and the client waits longer than that.
4. Once connected, the client shows the remote desktop; the session HUD's performance panel
   shows the path, e.g. `Path: srflx, host → host, srflx`.

## Reading the result

- **Transport WebRTC over UDP** (`WebRTC · UDP`, path contains `srflx`) — the intended path:
  RTP video with GCC, pacing, FEC and NACK.
- **Transport TCP** — WebRTC could not connect and the session fell back to the Noise-encrypted
  TCP stream over port 39200. The session still works, but video uses the chunked TCP path with
  the delivery-ratio controller, not RTP/FEC. Common causes: UDP not forwarded, firewall,
  symmetric NAT, or a wrong/missing STUN server.
- **No connection at all** — TCP 39200 never reached the host: port forward, macOS firewall, ISP
  carrier-grade NAT, or a wrong public IP/code. Verify from the friend's Mac with
  `nc -vz PUBLIC_IP 39200`.
- **Path shows `relay`** — media is going through a configured TURN server (still encrypted);
  expected on networks where direct UDP is blocked.

Useful numbers on the HUD: fps, actual Mbps, adaptive target/ceiling, jitter, RTT, dropped frames.
The Host screen shows the sending side's target and the connected peer.

## Multiple viewers

Direct hosting allows up to eight concurrent viewers; each gets its own session thread, transport
and encoder.

1. With one viewer connected, copy a **fresh join code** from **Host → Network → Share with a
   friend** (admission rotates the code, so the old one is dead).
2. The second friend pastes it under **Connect directly → Join code**. You do not have to stop the
   first session.
3. The waiting request appears in the **presence dock** at the bottom right of every screen
   (pulsing avatar). Click it → **Accept connection** / **Reject**.
4. Click a connected viewer's avatar to open their popover: device, peer address, transport and
   ICE path, fps/bitrate/jitter, **Keyboard / Mouse / Gamepad** switches, and **Disconnect**.
   Turning keyboard and mouse off makes that viewer view-only immediately; Disconnect sends a
   clean Bye. Relay rooms still accept one viewer at a time.

### Clipboard

Plain-text clipboard sync is shared both ways during a session when **Host → Advanced → Clipboard
text sync** is on. macOS may show a pasteboard-access prompt the first time; approve it (or turn
the toggle off). Only text is exchanged, capped at 64 KiB; images and rich formats are not synced.

## Session quality and latency

Defaults are tuned for a home internet/Wi-Fi path: **12 Mbps** host bitrate with adaptive rate
control, and "Match display" captures the full display at its real aspect (on this Mac 1710×1107
logical → **1920×1243**) instead of forcing 16:9, so the remote picture is not letterboxed or
cropped.

- On Wi-Fi or across the internet use **8–12 Mbps** (Host → Video); use 20–35 Mbps only on a LAN
  or a fast wired path.
- A high RTT that grows during a session usually means the host's **uplink is queued**: lower the
  bitrate first, then check the host's Wi-Fi signal and that nobody is saturating the upload.
- The host now trims the encoder when the receiver reports rising RTT or frame loss, keeps the
  session's minimum path delay for GCC's queue signal, and uses shallower pacer/video queues.
- Watch the client HUD: fps, actual Mbps, adaptive target/ceiling, jitter, RTT, dropped frames.
  Reduced resolution/refresh in Host → Video also lowers encode and uplink load.

## If port forwarding is not possible

- **TURN server**: run coturn on a small public VPS (UDP 3478 plus a relay port range), then enter
  `turn:your-server:3478` plus username/credential in **Settings → Network** on both Macs. Media
  stays end-to-end encrypted; the TURN server relays UDP when direct fails.
- **Self-hosted relay** (TCP, ciphertext only): run the crate on a public VPS
  `cargo run --manifest-path relay/Cargo.toml --release -- --bind 0.0.0.0 --port 39210`, open TCP
  39210, set **Host → Network → Relay address** to `vps:39210`, enable **Relay invite**, and send
  the invite. Relay sessions deliberately stay on TCP, so they do not exercise the RTP path.
- **Tailscale or another VPN overlay** on both Macs: connect directly to the host's tailnet/overlay IP and
  port 39200. No app changes are needed, and it is the quickest way to get a real cross-city
  session when routers cannot be configured.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| `Detect public address` fails | STUN URL format (`stun:host:port`), UDP egress, hosting stopped so the media port is free |
| Mapped port differs from 39201 | Forward the mapped external port → internal 39201, or set that external port in the router and re-detect |
| WebRTC always falls back to TCP | UDP 39201 not forwarded, macOS firewall, or ISP CGNAT; test `nc -vz` for TCP and a UDP echo/TURN service for UDP |
| Approval expires during connect | Requests persist for two minutes; accept from the card or the presence dock. A declined or expired request rotates the code, so share a fresh join code |
| Session ends immediately after accepting | Host logs in Host → status error; screen-recording permission revoked, or encoder failed |
| Works on LAN, fails across cities | Almost always missing port forward/public IP, missing STUN, or CGNAT; compare `Path` values before/after |

The first real cross-city session (Tailscale direct path) ran at ~19 Mbps with a 338 ms RTT while
the old 35 Mbps ceiling left a standing queue on the host's uplink; the 12 Mbps default, smaller
pacer/video queues, session-long GCC base delay and RTT/loss trimming exist to fix exactly that.
Treat a first run's numbers as observations; the values that matter are RTT, jitter, dropped
frames and the chosen path.
