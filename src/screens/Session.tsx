import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import {
  Activity,
  Gauge,
  Globe,
  Loader2,
  Maximize2,
  Minimize2,
  Monitor,
  MonitorPlay,
  MoreHorizontal,
  MousePointer2,
  Play,
  Power,
  X,
} from "lucide-react";
import { Button } from "../components/Button";
import { Kbd } from "../components/Kbd";
import { LogoMark } from "../components/Logo";
import { Modal } from "../components/Modal";
import { Sparkline } from "../components/Sparkline";
import { isModifierKey, keyEvent } from "../lib/keycodes";
import {
  attachNativeVideo,
  detachNativeVideo,
  getNativeEngineStatus,
  getNativeVideoSize,
  requestNativeClientDisplay,
  sendNativeClientInput,
  setFullscreen,
  subscribeNativeCapture,
  subscribeNativeClient,
  updateNativeVideoBounds,
} from "../lib/native";
import { useApp } from "../lib/store";
import type {
  NativeCaptureFrame,
  NativeCaptureStats,
  NativeCaptureStopped,
  NativeCaptureSummary,
  NativeClientStats,
  NativeClientStopped,
  NativeClientSummary,
  NativeClientVideo,
  NativeGamepadState,
} from "../lib/types";
import { cn, formatDuration } from "../lib/utils";

export function SessionScreen({
  live = null,
  remote = null,
  remoteLabel = "",
  onDisconnect,
  onLiveStopped,
  onRemoteStopped,
  onStartPreview,
  onConnectDirect,
}: {
  live?: NativeCaptureSummary | null;
  remote?: NativeClientSummary | null;
  remoteLabel?: string;
  onDisconnect: () => void;
  onLiveStopped?: (event: NativeCaptureStopped) => void;
  onRemoteStopped?: (event: NativeClientStopped) => void;
  onStartPreview?: () => void;
  onConnectDirect?: () => void;
}) {
  const { data } = useApp();
  const localPreview = Boolean(live);
  const remoteMode = Boolean(remote);
  const sessionName = live
    ? "This Mac"
    : remote?.hostName || remoteLabel || "Direct session";
  const [elapsed, setElapsed] = useState(0);
  const [samples, setSamples] = useState<number[]>([]);
  const [statsOpen, setStatsOpen] = useState(data.settings.showStats);
  const [menuOpen, setMenuOpen] = useState(false);
  const [hudVisible, setHudVisible] = useState(true);
  const [fullscreen, setFullscreenState] = useState(false);
  const [liveFrame, setLiveFrame] = useState<NativeCaptureFrame | null>(null);
  const [liveStats, setLiveStats] = useState<NativeCaptureStats | null>(null);
  const [videoSize, setVideoSize] = useState<NativeClientVideo | null>(null);
  const [videoError, setVideoError] = useState<string | null>(null);
  const [surfaceAttempt, setSurfaceAttempt] = useState(0);
  const [remoteStats, setRemoteStats] = useState<NativeClientStats | null>(null);
  const [remoteInput, setRemoteInput] = useState(remote?.allowInput ?? false);
  const [displayIndex, setDisplayIndex] = useState<number | null>(null);
  const hideTimer = useRef<number | undefined>(undefined);
  const ending = useRef(false);
  const stoppedHandler = useRef(onLiveStopped);
  const remoteStoppedHandler = useRef(onRemoteStopped);
  const stageRef = useRef<HTMLDivElement | null>(null);
  const surfaceToken = useRef(0);
  const pendingPointer = useRef<{ x: number; y: number } | null>(null);
  const pointerFrame = useRef<number | undefined>(undefined);
  const pressedKeys = useRef<Set<string>>(new Set());
  const codec = live
    ? live.codec
    : data.host.codec === "auto"
      ? "HEVC"
      : data.host.codec.toUpperCase();
  const displayFps = remoteMode ? (remoteStats?.fps ?? 0) : (liveStats?.captureFps ?? 0);
  const displayLatency = remoteMode
    ? (remoteStats?.rttMs ?? 0)
    : (liveStats?.encodeMs ?? liveFrame?.encodeMs ?? 0);
  const displayBitrate = remoteMode
    ? (remoteStats?.mbps ?? 0)
    : (liveStats?.encodedKbps ?? 0) / 1000;
  const sessionSeconds = remoteMode
    ? (remoteStats?.uptimeSecs ?? elapsed)
    : (liveStats?.uptimeSecs ?? elapsed);
  const stageWidth = videoSize?.width ?? remote?.width ?? 0;
  const stageHeight = videoSize?.height ?? remote?.height ?? 0;
  const videoLive = videoSize !== null;

  useEffect(() => {
    stoppedHandler.current = onLiveStopped;
    remoteStoppedHandler.current = onRemoteStopped;
  }, [onLiveStopped, onRemoteStopped]);

  useEffect(() => {
    const interval = window.setInterval(() => {
      setElapsed((value) => value + 1);
    }, 1000);
    return () => window.clearInterval(interval);
  }, []);

  useEffect(() => {
    if (!live) return;
    let disposed = false;
    let unsubscribe: (() => void) | undefined;
    setSamples([]);

    void subscribeNativeCapture({
      onFrame: (frame) => {
        if (!disposed) setLiveFrame(frame);
      },
      onStats: (stats) => {
        if (disposed) return;
        setLiveStats(stats);
        setSamples((current) => [...current.slice(-31), Math.max(0.4, stats.encodeMs)]);
      },
      onStopped: (event) => {
        if (!disposed && !ending.current) stoppedHandler.current?.(event);
      },
    }).then(async (stopListening) => {
      if (disposed) {
        stopListening();
        return;
      }
      unsubscribe = stopListening;
      const status = await getNativeEngineStatus();
      if (!disposed && status && !status.running) {
        stoppedHandler.current?.({
          reason: "The native capture engine is not running.",
          frames: 0,
          uptimeSecs: 0,
          failed: true,
        });
      }
    });

    return () => {
      disposed = true;
      unsubscribe?.();
    };
  }, [live]);

  useEffect(() => {
    if (!remote) return;
    setRemoteInput(remote.allowInput);
    setVideoSize(null);
    setVideoError(null);
    setRemoteStats(null);
    setSamples([]);
    let disposed = false;
    let unsubscribe: (() => void) | undefined;

    void subscribeNativeClient({
      onVideo: (video) => {
        if (!disposed) setVideoSize(video);
      },
      onVideoError: (message) => {
        if (!disposed) setVideoError(message);
      },
      onStats: (stats) => {
        if (disposed) return;
        setRemoteStats(stats);
        setSamples((current) => [...current.slice(-31), Math.max(0.4, stats.decodeMs)]);
      },
      onInput: (event) => {
        if (!disposed) setRemoteInput(event.enabled);
      },
      onDisplay: (event) => {
        if (!disposed) setDisplayIndex(event.index);
      },
      onStopped: (event) => {
        if (!disposed && !ending.current) remoteStoppedHandler.current?.(event);
      },
    }).then(async (stopListening) => {
      if (disposed) {
        stopListening();
        return;
      }
      unsubscribe = stopListening;
      const size = await getNativeVideoSize();
      if (!disposed && size) setVideoSize(size);
    });

    return () => {
      disposed = true;
      unsubscribe?.();
    };
  }, [remote]);

  useEffect(() => {
    if (!remoteMode) return;
    const root = document.documentElement;
    root.classList.add("has-native-video");
    return () => root.classList.remove("has-native-video");
  }, [remoteMode]);

  useEffect(() => {
    if (!remote) return;
    const token = ++surfaceToken.current;
    let disposed = false;
    let attached = false;
    let attaching: Promise<void> | null = null;
    let lastBounds = "";
    let observer: ResizeObserver | null = null;

    const sync = () => {
      const stage = stageRef.current;
      if (!stage) return;
      const rect = stage.getBoundingClientRect();
      if (rect.width < 1 || rect.height < 1) return;
      const bounds = {
        x: rect.left,
        y: rect.top,
        width: rect.width,
        height: rect.height,
        scale: window.devicePixelRatio || 1,
      };
      const key = `${bounds.x}:${bounds.y}:${bounds.width}:${bounds.height}:${bounds.scale}`;
      if (key === lastBounds) return;
      lastBounds = key;
      if (attached) {
        void updateNativeVideoBounds(bounds).catch(() => {});
        return;
      }
      if (attaching) return;
      attaching = attachNativeVideo(bounds)
        .then(() => {
          attached = true;
          if (disposed) {
            attached = false;
            if (surfaceToken.current === token) void detachNativeVideo();
          }
        })
        .catch((error) => {
          lastBounds = "";
          if (!disposed) {
            setVideoError(error instanceof Error ? error.message : String(error));
          }
        })
        .finally(() => {
          attaching = null;
        });
    };

    const stage = stageRef.current;
    if (stage) {
      observer = new ResizeObserver(sync);
      observer.observe(stage);
    }
    sync();
    window.addEventListener("resize", sync);
    window.visualViewport?.addEventListener("resize", sync);

    return () => {
      disposed = true;
      observer?.disconnect();
      window.removeEventListener("resize", sync);
      window.visualViewport?.removeEventListener("resize", sync);
      const release = () => {
        if (surfaceToken.current !== token) return;
        void detachNativeVideo();
      };
      if (attaching) {
        void attaching.finally(() => window.setTimeout(release, 0));
      } else if (attached) {
        window.setTimeout(release, 0);
      }
    };
  }, [remote, remoteMode, surfaceAttempt]);

  const retrySurface = () => {
    setVideoError(null);
    setSurfaceAttempt((value) => value + 1);
  };

  useEffect(() => {
    if (remoteInput) return;
    pressedKeys.current.clear();
  }, [remoteInput]);

  useEffect(() => {
    if (!remote || !remoteInput) return;
    const stage = stageRef.current;
    if (!stage) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const scale = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? 400 : 1;
      void sendNativeClientInput({
        type: "scroll",
        deltaX: event.deltaX * scale,
        deltaY: event.deltaY * scale,
      });
    };
    stage.addEventListener("wheel", onWheel, { passive: false });
    return () => stage.removeEventListener("wheel", onWheel);
  }, [remote, remoteInput]);

  useEffect(() => {
    if (!remote || !remoteInput || menuOpen) return;

    const isEditable = (target: EventTarget | null) =>
      target instanceof HTMLElement &&
      (target.tagName === "INPUT" ||
        target.tagName === "TEXTAREA" ||
        target.isContentEditable);

    const releaseAll = () => {
      for (const code of pressedKeys.current) {
        const payload = keyEvent(code, false, {
          shiftKey: false,
          ctrlKey: false,
          altKey: false,
          metaKey: false,
        });
        if (payload) void sendNativeClientInput(payload);
      }
      pressedKeys.current.clear();
    };

    const onKeyDown = (event: KeyboardEvent) => {
      if (isEditable(event.target)) return;
      if (event.key === "Escape") return;
      const meta = event.metaKey || event.ctrlKey;
      if (meta && event.shiftKey && ["s", "d"].includes(event.key.toLowerCase())) return;
      if (event.metaKey && event.ctrlKey && event.key.toLowerCase() === "f") return;
      if (isModifierKey(event.code)) return;

      if (!meta && !event.altKey && event.key.length === 1) {
        event.preventDefault();
        void sendNativeClientInput({ type: "text", text: event.key });
        return;
      }

      const payload = keyEvent(event.code, true, event);
      if (!payload) return;
      event.preventDefault();
      pressedKeys.current.add(event.code);
      void sendNativeClientInput(payload);
    };

    const onKeyUp = (event: KeyboardEvent) => {
      if (!pressedKeys.current.has(event.code)) return;
      pressedKeys.current.delete(event.code);
      const payload = keyEvent(event.code, false, event);
      if (!payload) return;
      event.preventDefault();
      void sendNativeClientInput(payload);
    };

    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("blur", releaseAll);
    return () => {
      releaseAll();
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("blur", releaseAll);
    };
  }, [remote, remoteInput, menuOpen]);

  // Forward physical controllers to the host. Controller passthrough is
  // independent from keyboard/mouse consent: the host gates each viewer's pad
  // separately, so poll whenever a remote session is active.
  useEffect(() => {
    if (!remote) return;
    if (typeof navigator === "undefined" || typeof navigator.getGamepads !== "function") {
      return;
    }

    const active = new Map<number, string>();
    const lastSent = new Map<number, string>();
    let frame = 0;
    let disposed = false;

    const snapshot = (pad: Gamepad): NativeGamepadState => ({
      slot: pad.index,
      name: pad.id.slice(0, 64),
      connected: true,
      axes: Array.from(pad.axes, (value) =>
        Number.isFinite(value) ? Math.max(-1, Math.min(1, value)) : 0,
      ),
      buttons: Array.from(pad.buttons, (button) =>
        Number.isFinite(button.value) ? Math.max(0, Math.min(1, button.value)) : 0,
      ),
    });

    const release = (slot: number) => {
      void sendNativeClientInput({
        type: "gamepad",
        state: { slot, name: "", connected: false, axes: [], buttons: [] },
      });
    };

    const poll = () => {
      if (disposed) return;
      const seen = new Set<number>();
      for (const pad of navigator.getGamepads()) {
        if (!pad || !pad.connected) continue;
        seen.add(pad.index);
        const state = snapshot(pad);
        const key = JSON.stringify(state);
        if (lastSent.get(pad.index) === key) continue;
        lastSent.set(pad.index, key);
        active.set(pad.index, state.name);
        void sendNativeClientInput({ type: "gamepad", state });
      }
      for (const slot of Array.from(active.keys())) {
        if (seen.has(slot)) continue;
        active.delete(slot);
        lastSent.delete(slot);
        release(slot);
      }
      frame = window.requestAnimationFrame(poll);
    };

    frame = window.requestAnimationFrame(poll);
    return () => {
      disposed = true;
      window.cancelAnimationFrame(frame);
      for (const slot of active.keys()) release(slot);
      active.clear();
      lastSent.clear();
    };
  }, [remote, remoteInput]);

  const remotePoint = (clientX: number, clientY: number) => {
    const stage = stageRef.current;
    if (!stage || !stageWidth || !stageHeight) return null;
    const rect = stage.getBoundingClientRect();
    const scale = Math.min(rect.width / stageWidth, rect.height / stageHeight);
    const width = stageWidth * scale;
    const height = stageHeight * scale;
    const x = (clientX - rect.left - (rect.width - width) / 2) / width;
    const y = (clientY - rect.top - (rect.height - height) / 2) / height;
    if (x < 0 || x > 1 || y < 0 || y > 1) return null;
    return { x, y };
  };

  const queuePointer = (x: number, y: number) => {
    pendingPointer.current = { x, y };
    if (pointerFrame.current !== undefined) return;
    pointerFrame.current = window.requestAnimationFrame(() => {
      pointerFrame.current = undefined;
      const point = pendingPointer.current;
      pendingPointer.current = null;
      if (point) void sendNativeClientInput({ type: "pointer", ...point });
    });
  };

  const remoteButton = (button: number) =>
    button === 0 ? "left" : button === 1 ? "middle" : button === 2 ? "right" : null;

  const onRemotePointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!remoteInput) return;
    const point = remotePoint(event.clientX, event.clientY);
    if (point) queuePointer(point.x, point.y);
  };

  const onRemotePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!remoteInput) return;
    event.currentTarget.focus();
    const point = remotePoint(event.clientX, event.clientY);
    if (point) {
      pendingPointer.current = null;
      void sendNativeClientInput({ type: "pointer", ...point });
    }
    const button = remoteButton(event.button);
    if (!button) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    void sendNativeClientInput({ type: "button", button, pressed: true });
  };

  const onRemotePointerUp = (event: ReactPointerEvent<HTMLDivElement>) => {
    const button = remoteButton(event.button);
    if (!button) return;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    void sendNativeClientInput({ type: "button", button, pressed: false });
  };

  useEffect(() => {
    const schedule = () => {
      window.clearTimeout(hideTimer.current);
      if (menuOpen) return;
      hideTimer.current = window.setTimeout(() => setHudVisible(false), 3000);
    };
    const wake = () => {
      setHudVisible(true);
      schedule();
    };
    wake();
    window.addEventListener("mousemove", wake);
    return () => {
      window.removeEventListener("mousemove", wake);
      window.clearTimeout(hideTimer.current);
    };
  }, [menuOpen]);

  const toggleFullscreen = async () => {
    const applied = await setFullscreen(!fullscreen);
    setFullscreenState(applied);
  };

  const disconnect = () => {
    ending.current = true;
    if (fullscreen) void setFullscreen(false);
    setMenuOpen(false);
    onDisconnect();
  };

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      const meta = event.metaKey || event.ctrlKey;
      if (event.key === "Escape") {
        event.preventDefault();
        setMenuOpen((value) => !value);
        return;
      }
      if (meta && event.shiftKey && event.key.toLowerCase() === "s") {
        event.preventDefault();
        setStatsOpen((value) => !value);
      }
      if (meta && event.shiftKey && event.key.toLowerCase() === "d") {
        event.preventDefault();
        disconnect();
      }
      if (meta && event.ctrlKey && event.key.toLowerCase() === "f") {
        event.preventDefault();
        void toggleFullscreen();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  });

  const wakeHud = () => setHudVisible(true);

  if (!live && !remote) {
    return (
      <div className="session session--empty">
        <div className="live-stage__empty">
          <span className="live-stage__pulse" aria-hidden="true">
            <MonitorPlay size={24} />
          </span>
          <p>No active session</p>
          <span>Start a live preview of this Mac, or connect directly to another computer.</span>
          <div className="empty-state__actions">
            {onStartPreview ? (
              <Button variant="secondary" leftIcon={<Play size={15} />} onClick={onStartPreview}>
                Start live preview
              </Button>
            ) : null}
            {onConnectDirect ? (
              <Button variant="primary" onClick={onConnectDirect}>
                Connect directly
              </Button>
            ) : null}
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="session">
      <div
        className={cn("remote", (live || remoteMode) && "remote--live", remoteMode && "remote--native")}
        onMouseDown={wakeHud}
        onMouseMove={wakeHud}
      >
        {remote ? (
          <div
            ref={stageRef}
            className={cn(
              "live-stage",
              "remote-stage",
              "remote-stage--native",
              !videoLive && "is-waiting",
              remoteInput && !videoError && "remote-stage--control",
            )}
            tabIndex={0}
            role="application"
            aria-label={`${sessionName} remote screen`}
            onPointerMove={onRemotePointerMove}
            onPointerDown={onRemotePointerDown}
            onPointerUp={onRemotePointerUp}
            onPointerCancel={onRemotePointerUp}
            onContextMenu={(event) => event.preventDefault()}
          >
            {videoError ? (
              <div
                className="live-stage__empty remote-stage__error"
                role="alert"
                onPointerDown={(event) => event.stopPropagation()}
              >
                <span className="live-stage__pulse" aria-hidden="true">
                  <MonitorPlay size={24} />
                </span>
                <p>Native video could not start</p>
                <span>{videoError}</span>
                <Button
                  variant="secondary"
                  size="sm"
                  className="remote-stage__retry"
                  onClick={retrySurface}
                >
                  Retry video surface
                </Button>
              </div>
            ) : !videoLive ? (
              <div className="live-stage__empty">
                <span className="live-stage__pulse" aria-hidden="true">
                  <MonitorPlay size={24} />
                </span>
                <p>Waiting for {sessionName}</p>
                <span>
                  The encrypted channel is open. The first hardware-decoded frame is on its way.
                </span>
              </div>
            ) : null}

            {!remoteInput && (
              <span className="remote-stage__hint">
                <MousePointer2 size={12} />
                View only · enable remote input on the host
              </span>
            )}

            <div className="live-stage__bar">
              <span className="live-stage__tag">
                <span className="presence-dot is-online" />
                {remote.path === "relay" ? "Relay" : "Direct"}
              </span>
              <span className="live-stage__display">{remote.hostName}</span>
              <span className="live-stage__spec">
                {remote.path === "relay" ? remote.peer.replace(/^relay:/, "") : remote.peer}
              </span>
              <span className="live-stage__spec">
                {remote.transport === "webrtc" ? "WebRTC · UDP" : "TCP"}
              </span>
              <span className="live-stage__spec">
                {stageWidth} × {stageHeight}
              </span>
              {remoteStats ? (
                <>
                  <span className="live-stage__spec">{Math.round(displayFps)} fps</span>
                  <span className="live-stage__spec">{Math.round(displayLatency)} ms RTT</span>
                  <span
                    className="live-stage__spec"
                    title={
                      remoteStats.adaptive
                        ? `Adaptive target ${remoteStats.streamMbps.toFixed(0)} of ${remoteStats.ceilingMbps.toFixed(0)} Mbps`
                        : undefined
                    }
                  >
                    {displayBitrate.toFixed(1)} Mbps
                  </span>
                </>
              ) : (
                <span className="live-stage__spec live-stage__warming">
                  <Loader2 size={11} className="spin" />
                  Negotiating video
                </span>
              )}
              <span className={cn("live-stage__local", !remoteInput && "is-muted")}>
                {remoteInput ? "Control" : "View only"}
              </span>
            </div>
          </div>
        ) : live ? (
          <div className="live-stage">
            {liveFrame ? (
              <img
                className="live-stage__frame"
                src={liveFrame.data}
                alt="Live capture of this Mac"
                draggable={false}
              />
            ) : (
              <div className="live-stage__empty">
                <span className="live-stage__pulse" aria-hidden="true">
                  <MonitorPlay size={24} />
                </span>
                <p>Waiting for the first frame</p>
                <span>ScreenCaptureKit sends a frame when this display changes.</span>
              </div>
            )}

            <div className="live-stage__bar">
              <span className="live-stage__tag">
                <span className="presence-dot is-online" />
                Live preview
              </span>
              <span className="live-stage__display">{live.display}</span>
              <span className="live-stage__spec">
                {live.width} × {live.height}
              </span>
              {liveStats ? (
                <>
                  <span className="live-stage__spec">{Math.round(displayFps)} fps</span>
                  <span className="live-stage__spec">
                    {displayLatency.toFixed(1)} ms encode
                  </span>
                </>
              ) : (
                <span className="live-stage__spec live-stage__warming">
                  <Loader2 size={11} className="spin" />
                  Starting encoder
                </span>
              )}
              <span className="live-stage__local">On-device</span>
            </div>
          </div>
        ) : null}

        <div className={cn("session-hud", hudVisible && "is-visible")}>
          <div className="session-hud__identity">
            <LogoMark size={20} />
            <span className="session-hud__name">{sessionName}</span>
            {localPreview && <span className="live-label">Local</span>}
            {remoteMode && (
              <span className="live-label">{remote?.path === "relay" ? "Relay" : "Direct"}</span>
            )}
            {remoteMode && (
              <span className={cn("input-badge", remoteInput ? "is-on" : "is-off")}>
                <MousePointer2 size={10} />
                {remoteInput ? "Control" : "View only"}
              </span>
            )}
          </div>
          <div className="session-hud__metrics">
            <span
              title={
                remoteMode
                  ? "Round-trip latency over the encrypted channel"
                  : "Hardware encode time"
              }
            >
              <Activity size={13} />
              {remoteMode ? `${Math.round(displayLatency)} ms` : `${displayLatency.toFixed(1)} ms`}
            </span>
            <span>{Math.round(displayFps)} fps</span>
            <span
              className="session-hud__bitrate"
              title={
                remoteMode && remoteStats?.adaptive
                  ? `Adaptive ${remoteStats.streamMbps.toFixed(0)} of ${remoteStats.ceilingMbps.toFixed(0)} Mbps`
                  : undefined
              }
            >
              {displayBitrate.toFixed(1)} Mbps
            </span>
          </div>
          <div className="session-hud__actions">
            <button
              type="button"
              className={cn("hud-button", statsOpen && "is-active")}
              aria-label="Toggle performance stats"
              title="Performance stats (⌘⇧S)"
              onClick={() => setStatsOpen((value) => !value)}
            >
              <Activity size={15} />
            </button>
            <button
              type="button"
              className="hud-button"
              aria-label={fullscreen ? "Exit fullscreen" : "Enter fullscreen"}
              title={fullscreen ? "Exit fullscreen" : "Fullscreen (⌘⌃F)"}
              onClick={() => void toggleFullscreen()}
            >
              {fullscreen ? <Minimize2 size={15} /> : <Maximize2 size={15} />}
            </button>
            <button
              type="button"
              className="hud-button"
              aria-label="Open session menu"
              title="Session menu (Esc)"
              onClick={() => setMenuOpen(true)}
            >
              <MoreHorizontal size={16} />
            </button>
            <span className="session-hud__divider" />
            <button
              type="button"
              className="hud-button is-danger"
              aria-label={remoteMode ? "End direct session" : "End live preview"}
              title={localPreview ? "End live preview" : "Disconnect"}
              onClick={disconnect}
            >
              <Power size={15} />
            </button>
          </div>
        </div>

        {statsOpen && (
          <aside className="session-stats" aria-label="Performance statistics">
            <div className="session-stats__head">
              <div>
                <span className="live-label">{remoteMode ? "Direct" : "Live"}</span>
                <h3>{remoteMode ? "Remote session" : "Capture engine"}</h3>
              </div>
              <button
                type="button"
                className="hud-button"
                aria-label="Hide performance stats"
                onClick={() => setStatsOpen(false)}
              >
                <X size={14} />
              </button>
            </div>
            <div className="session-stats__chart">
              {samples.length > 1 ? (
                <Sparkline values={samples} height={64} />
              ) : (
                <p className="settings-note">Waiting for telemetry…</p>
              )}
            </div>
            <div className="session-stats__grid">
              <div>
                <span>{remoteMode ? "Round trip" : "Encode"}</span>
                <strong>
                  {remoteMode ? `${Math.round(displayLatency)} ms` : `${displayLatency.toFixed(1)} ms`}
                </strong>
              </div>
              <div>
                <span>{remoteMode ? "Bitrate" : "Output"}</span>
                <strong>{displayBitrate.toFixed(1)}</strong>
              </div>
              <div>
                <span>Frame rate</span>
                <strong>{Math.round(displayFps)} fps</strong>
              </div>
              <div>
                <span>{remoteMode ? "Decode" : "Preview"}</span>
                <strong>
                  {remoteMode
                    ? `${(remoteStats?.decodeMs ?? 0).toFixed(1)} ms`
                    : `${(liveStats?.previewMs ?? 0).toFixed(1)} ms`}
                </strong>
              </div>
            </div>
            {remoteMode && remoteStats?.adaptive ? (
              <div className="session-stats__adaptive">
                <span>
                  <Gauge size={11} />
                  Adaptive
                </span>
                <strong>
                  {remoteStats.streamMbps.toFixed(0)} / {remoteStats.ceilingMbps.toFixed(0)} Mbps
                </strong>
                <span>{Math.round(remoteStats.jitterMs)} ms jitter</span>
              </div>
            ) : null}
            {remoteMode && remoteStats?.icePath ? (
              <div className="session-stats__adaptive">
                <span>
                  <Globe size={11} />
                  Path
                </span>
                <strong>{remoteStats.icePath}</strong>
              </div>
            ) : null}
            <div className="session-stats__foot">
              <span>
                {codec} · Hardware
                {live
                  ? ` · ${live.width}×${live.height}`
                  : remote
                    ? ` · ${stageWidth}×${stageHeight}`
                    : ""}
              </span>
              <span>{localPreview ? "On-device" : "Encrypted"}</span>
            </div>
          </aside>
        )}
      </div>

      <Modal
        open={menuOpen}
        onClose={() => setMenuOpen(false)}
        title={localPreview ? "Preview menu" : "Session menu"}
        description={`${sessionName} · ${formatDuration(sessionSeconds)} ${
          localPreview ? "local preview" : "connected"
        }`}
        size="sm"
        footer={
          <>
            <span className="modal__note">
              <Kbd>esc</Kbd> to close
            </span>
            <span className="session-modal__secure">
              {localPreview ? "Frames stay on this Mac" : "End-to-end encrypted"}
            </span>
          </>
        }
      >
        <div className="session-menu">
          <div className="session-menu__summary">
            <div className="session-menu__glyph">
              <LogoMark size={24} />
            </div>
            <div>
              <p>{sessionName}</p>
              <span>
                {localPreview
                  ? `On-device · ${codec} hardware encode`
                  : `${Math.round(displayLatency)} ms RTT · ${Math.round(displayFps)} fps · ${displayBitrate.toFixed(1)} Mbps`}
              </span>
            </div>
            <span className="status-badge is-online">
              <span className="status-badge__dot" />
              {remoteMode ? "Encrypted" : "On-device"}
            </span>
          </div>

          <Button
            variant="primary"
            block
            leftIcon={<Play size={15} />}
            onClick={() => setMenuOpen(false)}
          >
            {localPreview ? "Resume preview" : "Resume session"}
          </Button>

          {remoteMode && remote && (remote.displays?.length ?? 0) > 1 ? (
            <div className="session-menu__displays">
              <p className="session-menu__section-label">Shared display</p>
              <div className="session-displays">
                {(remote.displays ?? []).map((display) => (
                  <button
                    type="button"
                    key={display.index}
                    className={cn(
                      "session-display",
                      displayIndex === display.index && "is-active",
                    )}
                    onClick={() => {
                      setDisplayIndex(display.index);
                      void requestNativeClientDisplay(display.index);
                    }}
                  >
                    <Monitor size={15} />
                    <span>{display.label}</span>
                    <small>
                      {display.width} × {display.height}
                    </small>
                  </button>
                ))}
              </div>
            </div>
          ) : null}

          <div className="session-menu__list">
            <button
              type="button"
              className="session-action"
              onClick={() => setStatsOpen((value) => !value)}
            >
              <span className="session-action__icon">
                <Activity size={17} />
              </span>
              <span className="session-action__copy">
                <strong>Performance stats</strong>
                <small>
                  {remoteMode
                    ? "Round-trip latency, bitrate, and decode timing"
                    : "Capture, encode, and preview timing"}
                </small>
              </span>
              <span className="session-action__state">{statsOpen ? "On" : "Off"}</span>
            </button>
            <button
              type="button"
              className="session-action"
              onClick={() => void toggleFullscreen()}
            >
              <span className="session-action__icon">
                <Maximize2 size={17} />
              </span>
              <span className="session-action__copy">
                <strong>{fullscreen ? "Exit fullscreen" : "Enter fullscreen"}</strong>
                <small>
                  {localPreview
                    ? "Fill this display with the captured screen"
                    : "Fill this display with the remote screen"}
                </small>
              </span>
              <span className="session-action__state">
                <Kbd>⌘</Kbd>
                <Kbd>⌃</Kbd>
                <Kbd>F</Kbd>
              </span>
            </button>
            <button type="button" className="session-action is-danger" onClick={disconnect}>
              <span className="session-action__icon">
                <Power size={17} />
              </span>
              <span className="session-action__copy">
                <strong>{localPreview ? "End preview" : "Disconnect"}</strong>
                <small>
                  {localPreview
                    ? "Stop capture and return to host settings"
                    : "End this session and return to computers"}
                </small>
              </span>
              <span className="session-action__state">⌘⇧D</span>
            </button>
          </div>
        </div>
      </Modal>
    </div>
  );
}
