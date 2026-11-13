# Testing ParsecPlus on one Mac

You do **not** need a second Mac, a friend, or a VM for most of the app. The app has no
single-instance lock, so you can run **two copies of ParsecPlus side by side**: one as the
host, one as the client. The client copy is effectively a “simulated Mac” — it uses the same
real host listener, real ScreenCaptureKit capture, real HEVC encode/decode, real Noise
handshake, real approval request and real remote session.

This validates: host start/stop, invite/direct-connection codes, approval, WebRTC/TCP path,
video, input, clipboard, session HUD, display switching (if you have two displays), and
disconnect/reconnect. It does **not** validate cross-machine NAT, Tailscale, a real relay, or
two different GPUs — for those you need a second physical machine (or a macOS VM).

## Recommended: two app copies on this Mac

1. Install the current DMG to `/Applications/ParsecPlus.app` first.
2. Quit ParsecPlus if it is running.
3. Make a second copy (do this in Terminal):

   ```bash
   ditto /Applications/ParsecPlus.app /Applications/ParsecPlus-LocalTest.app
   ```

   Or run the helper:

   ```bash
   ./scripts/local-test.sh
   ```

4. Open the second copy as a separate process:

   ```bash
   open -n /Applications/ParsecPlus-LocalTest.app
   ```

   If LaunchServices refuses to open a duplicate, launch the binary directly:

   ```bash
   /Applications/ParsecPlus-LocalTest.app/Contents/MacOS/parsecplus &
   ```

5. **Host app** (the original): finish **Set up this Mac** if needed. Open **Host**, turn
   **Remote access** on. It shows **Live**. The **Remote access** card shows the invite code and
   the direct connection string; copy the **direct connection**.
6. **Client app** (the copy): open **Computers → Direct connect → Invite code**, paste the direct
   connection, and press **Send request**.
7. **Host app**: the request appears in the bottom-right presence dock / approval card. Accept it.
8. The client app opens the real session and shows this Mac’s screen. Move the mouse/keyboard in
   the client app; if input is allowed, the host sees it. Copy text on either side to test
   clipboard sync.
9. Use the session menu on the client to check performance stats, fullscreen, and — if this Mac has
   more than one display — **Shared display** to switch the host’s captured display live.
10. Disconnect from the client, then repeat to test a second viewer, or turn remote access off in
    the host.

### Important details

- Only **one** copy should host at a time. Both share TCP port `39200`; if both try to host, one
  gets a bind error. The second copy is only a client.
- Both copies share the same local profile/preferences (same bundle id and WebView data). That is
  fine for testing; a name change in one may need a window reload to show in the other.
- Screen Recording permission only matters for the **host** copy. If the host copy is the duplicate,
  grant Screen Recording to that copy separately (TCC identifies the app bundle); if you use the
  original as host, its existing grant is enough. Re-grant after every rebuilt ad-hoc signature.
- Use the **direct connection** string or `127.0.0.1:39200` with the invite code. A join code that
  contains this Mac’s LAN/Tailscale address also works locally because the address is reachable.
- If you have no external display, the **Shared display** switcher stays hidden (only one display).
- macOS may show “ParsecPlus is already running” if you use Finder’s normal Open; always use
  `open -n` or the binary launch above.
- Delete the copy when done:

  ```bash
  rm -rf /Applications/ParsecPlus-LocalTest.app
  ```

## Alternative: a macOS VM

A macOS guest VM (UTM/Parallels/VMware Fusion on Apple Silicon) can act as a second machine, but it
needs a macOS installer and a second Screen Recording grant, and it is much slower for video. This
is only worth it if you specifically need to test a second OS instance. Linux/Windows clients are
not built yet, so a Linux VM cannot act as a ParsecPlus client today.

## Alternative: automated protocol tests

The Rust test suite already covers the real transport pieces without a UI:

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

It exercises the Noise handshake, WebRTC/DTLS media loopback, RTP/FEC/NACK, relay encryption,
adaptive feedback and display-switch protocol. It does not exercise the macOS screen-capture TCC
path or the two-window approval UI — that is what the two-copy test above is for.

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| Second copy won’t open | Use `open -n` or launch `Contents/MacOS/parsecplus` directly |
| “Could not start remote access” in host | Port 39200 is already in use (other copy hosting); turn remote access off in one copy |
| Client connects but no video | Host needs Screen Recording; remove/re-add ParsecPlus in System Settings → Privacy & Security → Screen Recording and relaunch that copy |
| Request never appears | Server and client are on different app copies; the approval card is in the host copy’s window |
| Display switcher missing | Only one display is attached |
| Copy looks unsigned/quarantined | `xattr -dr com.apple.quarantine /Applications/ParsecPlus-LocalTest.app` and `codesign --force --deep --sign - /Applications/ParsecPlus-LocalTest.app` |
