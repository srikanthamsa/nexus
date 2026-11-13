import { useEffect, useState } from "react";
import {
  Clock3,
  Gamepad2,
  Keyboard,
  MousePointer2,
  Power,
  Radio,
  ShieldCheck,
  Wifi,
} from "lucide-react";
import {
  getNativeHostStatus,
  isTauri,
  kickHostViewer,
  respondToHostRequest,
  setHostPeerPermissions,
  subscribeNativeHost,
} from "../lib/native";
import type {
  NativeHostPendingConnection,
  NativeHostStatus,
  NativeViewer,
} from "../lib/types";
import { cn, formatDuration } from "../lib/utils";
import { Avatar } from "./Avatar";
import { Button } from "./Button";
import { Popover } from "./Popover";
import { Toggle } from "./Toggle";
import { useToast } from "./Toast";

function waitLabel(seconds: number): string {
  const safe = Math.max(0, Math.ceil(seconds));
  const minutes = Math.floor(safe / 60);
  const rest = safe % 60;
  return minutes > 0 ? `${minutes}m ${rest}s` : `${rest}s`;
}

function deviceName(entry: { owner: string; device: string }): string {
  return entry.owner || entry.device || "A ParsecPlus device";
}

export function PresenceDock({ compact = false }: { compact?: boolean }) {
  const { push } = useToast();
  const native = isTauri();
  const [status, setStatus] = useState<NativeHostStatus | null>(null);
  const [busyPeer, setBusyPeer] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [seenAt, setSeenAt] = useState<Record<string, number>>({});

  useEffect(() => {
    if (!native) return;
    let cancelled = false;
    let unsubscribe: (() => void) | undefined;
    void subscribeNativeHost({
      onStatus: (next) => {
        if (!cancelled) setStatus(next);
      },
    }).then((stop) => {
      if (cancelled) stop();
      else unsubscribe = stop;
    });
    void getNativeHostStatus().then((next) => {
      if (!cancelled && next) setStatus(next);
    });
    return () => {
      cancelled = true;
      unsubscribe?.();
    };
  }, [native]);

  const pending = status?.pending ?? [];
  const viewers = status?.viewers ?? [];
  const running = native && Boolean(status?.running);

  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [running]);

  useEffect(() => {
    setSeenAt((previous) => {
      const next: Record<string, number> = {};
      let changed = false;
      for (const request of pending) {
        next[request.peerId] = previous[request.peerId] ?? Date.now();
        if (!(request.peerId in previous)) changed = true;
      }
      if (Object.keys(previous).length !== Object.keys(next).length) changed = true;
      return changed ? next : previous;
    });
    // Requests appear and disappear through the native status stream.
  }, [status?.pending]);

  if (!running) return null;

  const respond = async (request: NativeHostPendingConnection, approve: boolean) => {
    setBusyPeer(request.peerId);
    const result = await respondToHostRequest(request.peerId, approve);
    setBusyPeer(null);
    if (result.error) {
      push({
        title: approve ? "Could not accept the connection" : "Could not reject the connection",
        description: result.error,
        tone: "error",
      });
      return;
    }
    if (result.status) setStatus(result.status);
    push({
      title: approve ? "Connection accepted" : "Connection rejected",
      description: approve
        ? `${deviceName(request)} can now view this Mac.`
        : `${deviceName(request)} was not admitted.`,
      tone: approve ? "success" : "default",
    });
  };

  const updatePermissions = async (
    viewer: NativeViewer,
    patch: Partial<Pick<NativeViewer, "keyboard" | "mouse" | "gamepad">>,
  ) => {
    const next = {
      keyboard: patch.keyboard ?? viewer.keyboard,
      mouse: patch.mouse ?? viewer.mouse,
      gamepad: patch.gamepad ?? viewer.gamepad,
    };
    setBusyPeer(viewer.peerId);
    const result = await setHostPeerPermissions(viewer.peerId, next);
    setBusyPeer(null);
    if (result.error) {
      push({
        title: "Could not update viewer access",
        description: result.error,
        tone: "error",
      });
      return;
    }
    if (result.status) {
      setStatus(result.status);
    } else {
      setStatus((current) =>
        current
          ? {
              ...current,
              viewers: current.viewers.map((entry) =>
                entry.peerId === viewer.peerId ? { ...entry, ...next } : entry,
              ),
            }
          : current,
      );
    }
  };

  const kick = async (viewer: NativeViewer) => {
    setBusyPeer(viewer.peerId);
    const result = await kickHostViewer(viewer.peerId);
    setBusyPeer(null);
    if (result.error) {
      push({ title: "Could not disconnect viewer", description: result.error, tone: "error" });
      return;
    }
    if (result.status) {
      setStatus(result.status);
    } else {
      setStatus((current) =>
        current
          ? {
              ...current,
              viewers: current.viewers.filter((entry) => entry.peerId !== viewer.peerId),
            }
          : current,
      );
    }
    push({
      title: "Viewer disconnected",
      description: `${deviceName(viewer)} was disconnected.`,
      tone: "default",
    });
  };

  const waitingSeconds = (request: NativeHostPendingConnection) => {
    const started = seenAt[request.peerId] ?? now;
    return Math.max(0, Math.round((request.expiresInMs - (now - started)) / 1000));
  };

  return (
    <div
      className={cn("presence-dock", compact && "is-compact")}
      role="region"
      aria-label="Host presence"
    >
      <div className="presence-dock__bar">
        <span className="presence-dock__hosting">
          <Radio size={13} />
          <span className="presence-dock__label">Hosting</span>
          <strong>
            {viewers.length}
            <span className="presence-dock__label"> connected</span>
          </strong>
          <span className="presence-dock__sep" aria-hidden="true" />
          <strong>
            {pending.length}
            <span className="presence-dock__label"> waiting</span>
          </strong>
        </span>

        {pending.map((request) => (
          <Popover
            key={request.peerId}
            ariaLabel={`Connection request from ${deviceName(request)}`}
            buttonClassName="presence-chip presence-chip--pending"
            panelClassName="presence-pop"
            width={286}
            button={
              <span
                className="presence-chip__avatar"
                title={`${deviceName(request)} · waiting · ${waitLabel(waitingSeconds(request))}`}
              >
                <Avatar name={deviceName(request)} size={compact ? 30 : 34} />
              </span>
            }
          >
            <div className="presence-pop__head">
              <div>
                <p>{deviceName(request)}</p>
                <span>
                  {request.device} · {request.peer || "pending peer"}
                </span>
              </div>
              <span className="presence-pop__badge is-waiting">
                <Clock3 size={11} />
                {waitLabel(waitingSeconds(request))}
              </span>
            </div>
            <p className="presence-pop__note">
              Verified your invite code and is asking to view this Mac.
            </p>
            <div className="presence-pop__actions">
              <Button
                variant="primary"
                size="sm"
                loading={busyPeer === request.peerId}
                onClick={() => void respond(request, true)}
              >
                Accept
              </Button>
              <Button
                variant="ghost"
                size="sm"
                disabled={busyPeer === request.peerId}
                onClick={() => void respond(request, false)}
              >
                Reject
              </Button>
            </div>
          </Popover>
        ))}

        {viewers.map((viewer) => {
          const viewOnly = !viewer.keyboard && !viewer.mouse;
          return (
            <Popover
              key={viewer.peerId}
              ariaLabel={`Connected viewer ${deviceName(viewer)}`}
              buttonClassName="presence-chip presence-chip--viewer"
              panelClassName="presence-pop"
              width={312}
              button={
                <span
                  className="presence-chip__avatar"
                  title={`${deviceName(viewer)} · connected · ${Math.round(viewer.fps)} fps`}
                >
                  <Avatar name={deviceName(viewer)} size={compact ? 30 : 34} presence="online" />
                </span>
              }
            >
              <div className="presence-pop__head">
                <div>
                  <p>{deviceName(viewer)}</p>
                  <span>
                    {viewer.device} · {viewer.peer}
                  </span>
                </div>
                <span className={cn("presence-pop__badge", viewOnly && "is-viewonly")}>
                  {viewOnly ? "View only" : "In control"}
                </span>
              </div>

              <div className="presence-pop__meta">
                <span>
                  <Wifi size={12} />
                  {viewer.transport === "webrtc" ? "WebRTC · UDP" : "TCP"}
                  {viewer.icePath ? ` · ${viewer.icePath}` : ""}
                </span>
                <span>
                  <Clock3 size={12} />
                  {formatDuration(viewer.uptimeSecs)}
                </span>
              </div>

              <div className="presence-pop__stats">
                <div>
                  <span>FPS</span>
                  <strong>{Math.round(viewer.fps)}</strong>
                </div>
                <div>
                  <span>Bitrate</span>
                  <strong>
                    {viewer.bitrateMbps.toFixed(0)}/{viewer.ceilingMbps.toFixed(0)}
                  </strong>
                </div>
                <div>
                  <span>Mbps</span>
                  <strong>{viewer.mbps.toFixed(1)}</strong>
                </div>
                <div>
                  <span>Jitter</span>
                  <strong>{Math.round(viewer.jitterMs)}</strong>
                </div>
              </div>

              <div className="presence-pop__perms">
                <span className="presence-pop__perms-label">
                  <ShieldCheck size={12} />
                  Access
                </span>
                <div className="presence-pop__perm">
                  <span>
                    <Keyboard size={13} />
                    Keyboard
                  </span>
                  <Toggle
                    size="sm"
                    label="Keyboard access"
                    checked={viewer.keyboard}
                    disabled={busyPeer === viewer.peerId}
                    onChange={(value) => void updatePermissions(viewer, { keyboard: value })}
                  />
                </div>
                <div className="presence-pop__perm">
                  <span>
                    <MousePointer2 size={13} />
                    Mouse
                  </span>
                  <Toggle
                    size="sm"
                    label="Mouse access"
                    checked={viewer.mouse}
                    disabled={busyPeer === viewer.peerId}
                    onChange={(value) => void updatePermissions(viewer, { mouse: value })}
                  />
                </div>
                <div className="presence-pop__perm">
                  <span>
                    <Gamepad2 size={13} />
                    Gamepad
                  </span>
                  <Toggle
                    size="sm"
                    label="Gamepad access"
                    checked={viewer.gamepad}
                    disabled={busyPeer === viewer.peerId}
                    onChange={(value) => void updatePermissions(viewer, { gamepad: value })}
                  />
                </div>
              </div>

              <Button
                variant="danger"
                size="sm"
                block
                leftIcon={<Power size={13} />}
                loading={busyPeer === viewer.peerId}
                onClick={() => void kick(viewer)}
              >
                Disconnect viewer
              </Button>
            </Popover>
          );
        })}

        {pending.length === 0 && viewers.length === 0 ? (
          <span className="presence-dock__quiet">Waiting for connection requests</span>
        ) : null}
      </div>
    </div>
  );
}
