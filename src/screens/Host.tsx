import { useEffect, useState, type ReactNode } from "react";
import { Warning as AlertTriangle, Check, Clipboard, Copy, GameController as Gamepad2, Info, Keyboard, LockKey as LockKeyhole, Monitor, MonitorArrowUp as MonitorUp, CursorClick as MousePointer2, Play, Broadcast as Radio, ArrowsClockwise as RefreshCw, ShieldCheck } from "@phosphor-icons/react";
import { Button } from "../components/Button";
import { PageHeader } from "../components/PageHeader";
import { Select } from "../components/Select";
import { Toggle } from "../components/Toggle";
import { useToast } from "../components/Toast";
import {
  buildIceSettings,
  listNativeDisplays,
  getNativeHostStatus,
  getNativePermissions,
  getSystemInfo,
  isTauri,
  listNativeNetworkAddresses,
  makeJoinCode,
  openPrivacyPane,
  requestNativePermission,
  rotateNativeHostCode,
  setNativeHostApproval,
  setNativeHostGamepad,
  setNativeHostInput,
  startNativeCapture,
  startNativeHost,
  stopNativeHost,
  subscribeNativeHost,
} from "../lib/native";
import { useApp } from "../lib/store";
import type {
  HostSettings,
  NativeCaptureSummary,
  NativeDisplayInfo,
  NativeDisplaySize,
  NativeHostStatus,
  PermissionKind,
  PermissionState,
  SystemInfo,
} from "../lib/types";
import { cn, copyToClipboard, friendlyDeviceName } from "../lib/utils";

const DEFAULT_HOST_TCP_PORT = 39200;
const DEFAULT_MEDIA_PORT = 39201;
const MATCH_CAPTURE_WIDTH = 1920;

/**
 * ScreenCaptureKit reports the logical display size (e.g. 1710 × 1107 on a
 * 16:10 panel). Capture at a sensible 1920 px width and derive the height from
 * the display aspect so the stream is not letterboxed or cropped.
 */
function matchCaptureDimensions(size: NativeDisplaySize): { width: number; height: number } {
  if (size.width <= 0 || size.height <= 0) return { width: 1920, height: 1080 };
  const rounded = Math.max(2, Math.round((MATCH_CAPTURE_WIDTH * size.height) / size.width));
  return {
    width: MATCH_CAPTURE_WIDTH,
    // The encoder normalises to an even height (e.g. 1243 → 1242).
    height: rounded % 2 === 0 ? rounded : rounded - 1,
  };
}

function clampMediaPort(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_MEDIA_PORT;
  return Math.min(65535, Math.max(1024, Math.trunc(value)));
}

function Panel({
  id,
  title,
  description,
  icon,
  aside,
  children,
  footnote,
}: {
  id?: string;
  title: string;
  description?: string;
  icon: ReactNode;
  aside?: ReactNode;
  children: ReactNode;
  footnote?: ReactNode;
}) {
  return (
    <section className="panel" id={id}>
      <div className="panel__head">
        <div className="panel__heading">
          <span className="panel__icon">{icon}</span>
          <div>
            <h2>{title}</h2>
            {description && <p>{description}</p>}
          </div>
        </div>
        {aside}
      </div>
      <div className="panel__body">{children}</div>
      {footnote && <div className="panel__footnote">{footnote}</div>}
    </section>
  );
}

function SettingRow({
  title,
  description,
  icon,
  children,
}: {
  title: string;
  description?: string;
  icon?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="setting-row">
      {icon && <span className="setting-row__icon">{icon}</span>}
      <div className="setting-row__copy">
        <p>{title}</p>
        {description && <span>{description}</span>}
      </div>
      <div className="setting-row__control">{children}</div>
    </div>
  );
}

const permissionMeta: Array<{
  kind: PermissionKind;
  key: keyof PermissionState;
  title: string;
  description: string;
  required: boolean;
}> = [
  {
    kind: "screen",
    key: "screenRecording",
    title: "Screen Recording",
    description: "Required to share this screen.",
    required: true,
  },
  {
    kind: "accessibility",
    key: "accessibility",
    title: "Accessibility",
    description: "Lets friends control the pointer and keyboard.",
    required: true,
  },
  {
    kind: "input",
    key: "inputMonitoring",
    title: "Input Monitoring",
    description: "Helps controllers work.",
    required: false,
  },
];

function permissionIcon(kind: PermissionKind) {
  if (kind === "screen") return <MonitorUp size={17} />;
  if (kind === "accessibility") return <MousePointer2 size={17} />;
  return <Keyboard size={17} />;
}

function captureDimensions(resolution: HostSettings["resolution"]) {
  if (resolution === "2560x1440") return { width: 2560, height: 1440 };
  if (resolution === "3840x2160") return { width: 3840, height: 2160 };
  return { width: 1920, height: 1080 };
}

export function HostScreen({
  onStartPreview,
}: {
  onStartPreview: (summary: NativeCaptureSummary) => void;
}) {
  const { data, updateHost } = useApp();
  const { push } = useToast();
  const [system, setSystem] = useState<SystemInfo | null>(null);
  const [perms, setPerms] = useState<PermissionState>(data.permissions);
  const [granting, setGranting] = useState<PermissionKind | null>(null);
  const [codeLoading, setCodeLoading] = useState(true);
  const [previewStarting, setPreviewStarting] = useState(false);
  const [hostStatus, setHostStatus] = useState<NativeHostStatus | null>(null);
  const [displaySize, setDisplaySize] = useState<NativeDisplaySize | null>(null);
  const [displays, setDisplays] = useState<NativeDisplayInfo[]>([]);
  const [displayLoading, setDisplayLoading] = useState(false);
  const [displayError, setDisplayError] = useState<string | null>(null);
  const [allowRemoteInput, setAllowRemoteInput] = useState(false);
  const [selectedAddress, setSelectedAddress] = useState("");
  const [joinCode, setJoinCode] = useState("");
  const [joinCodeLoading, setJoinCodeLoading] = useState(false);
  const [joinCodeError, setJoinCodeError] = useState<string | null>(null);
  const [joinCodeCopied, setJoinCodeCopied] = useState(false);
  const host = data.host;
  const native = isTauri();
  const hostRunning = native ? Boolean(hostStatus?.running) : host.hosting;
  const viewers = hostStatus?.viewers ?? [];
  const pendingRequests = hostStatus?.pending ?? [];
  const hostError = native ? hostStatus?.lastError ?? null : null;
  const mediaPort = clampMediaPort(host.mediaPort);
  const hostingCode = hostStatus?.summary?.code ?? "";
  const hostingPort = hostStatus?.summary?.port ?? DEFAULT_HOST_TCP_PORT;


  useEffect(() => {
    let cancelled = false;
    void getSystemInfo().then((info) => {
      if (!cancelled) setSystem(info);
    });
    setCodeLoading(false);
    return () => {
      cancelled = true;
    };
  }, [native]);

  useEffect(() => {
    if (!native) return;
    let cancelled = false;
    let unsubscribe: (() => void) | undefined;

    const apply = (status: NativeHostStatus) => {
      if (cancelled) return;
      setHostStatus(status);
      setAllowRemoteInput(status.allowInput);
      updateHost({ hosting: status.running, requireApproval: status.requireApproval });
    };

    void subscribeNativeHost({ onStatus: apply }).then((stop) => {
      if (cancelled) stop();
      else unsubscribe = stop;
    });

    void getNativeHostStatus().then((status) => {
      if (cancelled || !status) return;
      apply(status);
    });

    return () => {
      cancelled = true;
      unsubscribe?.();
    };
  }, [native]);

  const selectedDisplay =
    displays.find((entry) => entry.index === host.displayIndex) ?? displays[0] ?? null;

  const loadDisplays = async () => {
    if (!native) return;
    setDisplayLoading(true);
    setDisplayError(null);
    const list = await listNativeDisplays();
    setDisplayLoading(false);
    if (!list) {
      setDisplayError("Could not read this Mac's displays. Check Screen Recording access.");
      return;
    }
    setDisplays(list);
    const preferred =
      list.find((entry) => entry.index === host.displayIndex) ?? list[0] ?? null;
    if (preferred) {
      setDisplaySize({ width: preferred.width, height: preferred.height });
    }
  };

  useEffect(() => {
    if (!native) return;
    let cancelled = false;
    setDisplayLoading(true);
    void listNativeDisplays().then((list) => {
      if (cancelled) return;
      setDisplayLoading(false);
      if (!list) {
        setDisplayError("Could not read this Mac's displays. Check Screen Recording access.");
        return;
      }
      setDisplays(list);
      const preferred = list[0];
      if (preferred) {
        setDisplaySize({ width: preferred.width, height: preferred.height });
      }
    });
    return () => {
      cancelled = true;
    };
  }, [native]);

  useEffect(() => {
    if (!selectedDisplay) return;
    setDisplaySize({ width: selectedDisplay.width, height: selectedDisplay.height });
  }, [selectedDisplay?.index, selectedDisplay?.width, selectedDisplay?.height]);

  useEffect(() => {
    let cancelled = false;
    const refresh = async () => {
      const fresh = await getNativePermissions();
      if (cancelled) return;
      if (fresh) {
        setPerms(fresh);
      } else {
        setPerms(data.permissions);
      }
    };
    void refresh();
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    return () => {
      cancelled = true;
      window.removeEventListener("focus", onFocus);
    };
  }, [data.permissions]);

  useEffect(() => {
    let cancelled = false;
    void listNativeNetworkAddresses().then((info) => {
      if (cancelled) return;
      if (!info) return;
      const preferred = info.addresses.find((entry) => entry.tailscale) ?? info.addresses[0];
      if (preferred) setSelectedAddress((current) => current || preferred.address);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!native || !hostRunning || !hostingCode || !selectedAddress) {
      setJoinCode("");
      setJoinCodeError(null);
      return;
    }
    let cancelled = false;
    setJoinCodeLoading(true);
    setJoinCodeError(null);
    void makeJoinCode(selectedAddress, hostingPort, hostingCode).then((result) => {
      if (cancelled) return;
      setJoinCodeLoading(false);
      if (!result.ok) {
        setJoinCode("");
        setJoinCodeError(result.error);
        return;
      }
      setJoinCode(result.value);
    });
    return () => {
      cancelled = true;
    };
  }, [native, hostRunning, hostingCode, hostingPort, selectedAddress]);

  const resolveCaptureDimensions = async (): Promise<{ width: number; height: number }> => {
    if (host.resolution !== "match") return captureDimensions(host.resolution);
    const size = displaySize;
    if (!size || size.width <= 0 || size.height <= 0) return { width: 1920, height: 1080 };
    if (!displaySize) setDisplaySize(size);
    return matchCaptureDimensions(size);
  };

  const toggleHosting = async (value: boolean) => {
    if (!native) {
      push({
        title: "Hosting needs the desktop app",
        description:
          "Start hosting from the desktop app.",
        tone: "info",
      });
      void value;
      return;
    }

    if (value) {
      const dimensions = await resolveCaptureDimensions();
      const status = await startNativeHost({
        options: {
          displayIndex: selectedDisplay?.index ?? null,
          width: dimensions.width,
          height: dimensions.height,
          frameRate: Math.min(60, Number(host.refresh)),
          bitrateMbps: host.bitrate,
          previewFps: 12,
          previewMaxWidth: 1600,
          showsCursor: true,
          clipboard: host.clipboard,
        },
        requireApproval: host.requireApproval,
        gamepad: host.gamepad,
        adaptiveBitrate: host.adaptNetwork,
        relayAddress: undefined,
        ice: buildIceSettings(data.settings),
        mediaPort,
      });
      if ("error" in status) {
        updateHost({ hosting: false });
        push({
          title: "Could not start hosting",
          description: status.error,
          tone: "error",
        });
        return;
      }
      setHostStatus(status);
      setAllowRemoteInput(status.allowInput);
      updateHost({ hosting: true });
      push({
        title: "Hosting enabled",
        description: status.summary
          ? `Listening on ${status.summary.address}:${status.summary.port}.`
          : "This Mac can now accept direct connections.",
        tone: "success",
      });
      if (!perms.screenRecording) {
        push({
          title: "Screen Recording still needed",
          description:
            "A device can pair, but capture starts only after you grant Screen Recording.",
          tone: "info",
        });
      }
      return;
    }

    const status = await stopNativeHost();
    setHostStatus(status);
    setAllowRemoteInput(false);
    updateHost({ hosting: false });
    push({
      title: "Hosting stopped",
      description: "Remote access is off. The old code no longer works.",
      tone: "default",
    });
  };

  const grant = async (kind: PermissionKind) => {
    setGranting(kind);
    if (native) {
      const result = await requestNativePermission(kind);
      await new Promise((resolve) => window.setTimeout(resolve, 500));
      const fresh = await getNativePermissions();
      if (fresh) setPerms(fresh);
      if (kind === "accessibility") {
        push({
          title: "Approve Nexus in System Settings",
          description: "Enable the toggle, then return here. Status refreshes automatically.",
          tone: "info",
        });
      } else {
        push({
          title: result ? "Permission granted" : "Permission requested",
          description: result
            ? "Nexus can now use this capability."
            : "Approve Nexus in System Settings, then return here.",
          tone: result ? "success" : "info",
        });
      }
    } else {
      setGranting(null);
      push({
        title: "Permissions are checked by macOS",
        description:
          "Run the desktop app to grant Screen Recording, Accessibility, and Input Monitoring.",
        tone: "info",
      });
    }
    setGranting(null);
  };

  const openSettings = async (kind: PermissionKind) => {
    const opened = await openPrivacyPane(kind);
    if (!opened) {
      push({
        title: "System Settings",
        description: "Privacy controls open in the native macOS build.",
        tone: "info",
      });
    }
  };

  const regenerateCode = async () => {
    if (native) {
      if (!hostRunning) {
        push({
          title: "Start hosting first",
          description: "A code appears after hosting starts.",
          tone: "info",
        });
        return;
      }
      setCodeLoading(true);
      const result = await rotateNativeHostCode();
      setCodeLoading(false);
      if (result.status) {
        setHostStatus(result.status);
        push({ title: "New invite code ready", tone: "success" });
      } else {
        push({
          title: "Could not rotate the code",
          description: result.error,
          tone: "error",
        });
      }
      return;
    }

    setCodeLoading(true);
    push({
      title: "Start hosting for a code",
      description: "Run the desktop app and start hosting to rotate a real code.",
      tone: "info",
    });
    setCodeLoading(false);
  };

  const copyJoinCode = async () => {
    if (!joinCode) return;
    const ok = await copyToClipboard(joinCode);
    if (!ok) {
      push({ title: "Could not copy the direct connection", tone: "error" });
      return;
    }
    setJoinCodeCopied(true);
    window.setTimeout(() => setJoinCodeCopied(false), 1800);
    push({
      title: "Direct connection copied",
      description: "Send it to your friend.",
      tone: "success",
    });
  };

  const toggleRemoteInput = async (value: boolean) => {
    if (!native) {
      push({
        title: "Remote input needs the desktop app",
        description: "Direct sessions run in the native macOS build.",
        tone: "info",
      });
      return;
    }
    if (!hostRunning) {
      push({
        title: "Start hosting first",
        description: "Remote input is armed per host session.",
        tone: "info",
      });
      return;
    }
    const result = await setNativeHostInput(value);
    if (result.error) {
      push({
        title: value ? "Could not enable remote input" : "Could not disable remote input",
        description: result.error,
        tone: "error",
      });
      return;
    }
    if (result.status) {
      setHostStatus(result.status);
      setAllowRemoteInput(result.status.allowInput);
    }
    push({
      title: value ? "Remote input enabled" : "Remote input disabled",
      description: value
        ? "The connected device can control this Mac. Input stops when the session ends."
        : "Remote devices can watch the screen but cannot control it.",
      tone: value ? "success" : "default",
    });
  };

  const toggleGamepad = async (value: boolean) => {
    if (!native) {
      updateHost({ gamepad: value });
      push({
        title: value ? "Controller passthrough enabled" : "Controller passthrough disabled",
        description: "Controllers are forwarded in the native macOS build.",
        tone: "info",
      });
      return;
    }
    if (!hostRunning) {
      updateHost({ gamepad: value });
      push({
        title: value ? "Controller passthrough enabled" : "Controller passthrough disabled",
        description: "The setting applies the next time hosting starts.",
        tone: "default",
      });
      return;
    }
    const result = await setNativeHostGamepad(value);
    if (result.error) {
      push({
        title: "Could not update controller passthrough",
        description: result.error,
        tone: "error",
      });
      return;
    }
    updateHost({ gamepad: value });
    if (result.status) setHostStatus(result.status);
    if (value && result.status?.gamepadError) {
      push({
        title: "Virtual gamepad unavailable",
        description: result.status.gamepadError,
        tone: "error",
      });
      return;
    }
    push({
      title: value ? "Controller passthrough enabled" : "Controller passthrough disabled",
      description: value
        ? "A connected viewer's controller appears on this Mac as a virtual gamepad."
        : "Controller input from viewers is ignored.",
      tone: value ? "success" : "default",
    });
  };

  const toggleApproval = async (value: boolean) => {
    if (!native || !hostRunning) {
      updateHost({ requireApproval: value });
      if (!native) {
        push({
          title: value ? "Approval required" : "Connections auto-admit",
          description: value
            ? "Incoming devices wait for your decision before the session starts."
            : "A device with a valid invite code connects without a prompt.",
          tone: value ? "success" : "default",
        });
      }
      return;
    }
    const result = await setNativeHostApproval(value);
    if (result.error) {
      push({
        title: "Could not update approval",
        description: result.error,
        tone: "error",
      });
      return;
    }
    if (result.status) setHostStatus(result.status);
    updateHost({ requireApproval: value });
    push({
      title: value ? "Approval required" : "Connections auto-admit",
      description: value
        ? "Incoming devices wait up to 15 seconds for your decision."
        : "The current invite code is enough to start a session.",
      tone: value ? "success" : "default",
    });
  };

  const startPreview = async () => {    if (!native) {
      push({
        title: "Live preview needs the desktop app",
        description:
          "Use the desktop app to share this screen.",
        tone: "info",
      });
      return;
    }

    setPreviewStarting(true);
    let screenGranted = perms.screenRecording;
    if (!screenGranted) {
      await requestNativePermission("screen");
      await new Promise((resolve) => window.setTimeout(resolve, 500));
      const fresh = await getNativePermissions();
      if (fresh) {
        setPerms(fresh);
        screenGranted = fresh.screenRecording;
      }
      if (!screenGranted) {
        setPreviewStarting(false);
        push({
          title: "Screen Recording permission needed",
          description:
            "Approve Nexus in System Settings, then relaunch the app before starting a live preview.",
          tone: "info",
        });
        return;
      }
    }

    const dimensions = await resolveCaptureDimensions();
    const result = await startNativeCapture({
      displayIndex: selectedDisplay?.index ?? null,
      width: dimensions.width,
      height: dimensions.height,
      frameRate: Math.min(60, Number(host.refresh)),
      bitrateMbps: host.bitrate,
      previewFps: 12,
      previewMaxWidth: 1280,
      showsCursor: true,
      clipboard: host.clipboard,
    });
    setPreviewStarting(false);

    if (!result.ok) {
      push({
        title: "Could not start live capture",
        description: result.error,
        tone: "error",
      });
      return;
    }

    onStartPreview(result.summary);
  };


  return (
    <div className="screen">
      <PageHeader
        title="Host"
        description="Make this Mac available to you and the people you trust."
        meta={
          viewers.length > 0 || pendingRequests.length > 0 ? (
            <span className={cn("meta-pill", hostRunning && "is-live")}>
              <span className={cn("presence-dot", hostRunning ? "is-online" : "is-idle")} />
              {viewers.length > 0
                ? `${viewers.length} connected`
                : `${pendingRequests.length} waiting`}
            </span>
          ) : undefined
        }
      />

      <section className={cn("host-hero", hostRunning && "is-live")}>
        <div className="host-hero__content">
          <div className="host-hero__icon">
            <Radio size={24} />
          </div>
          <div className="host-hero__copy">
            <div className="host-hero__title">
              <h2>Remote access</h2>
              <span className={cn("status-badge", hostRunning ? "is-online" : "is-idle")}>
                <span className="status-badge__dot" />
                {hostRunning ? "Live" : "Off"}
              </span>
            </div>
            <p>
              {hostRunning
                ? "Friends can join with the code below."
                : "Turn this on when you want friends to join."}
            </p>
          </div>
          <Toggle
            checked={hostRunning}
            onChange={(value) => void toggleHosting(value)}
            label="Remote access"
          />
        </div>

        <div className="host-hero__stats">
          <span>
            <Monitor size={14} />
            {data.user?.name || friendlyDeviceName(system?.hostname)}
          </span>
          <span>
            <MonitorUp size={14} />
            {displaySize ? `${displaySize.width} × ${displaySize.height}` : "This display"}
          </span>
          <Button
            size="sm"
            variant="secondary"
            className="host-hero__preview"
            leftIcon={<Play size={13} />}
            loading={previewStarting}
            onClick={() => void startPreview()}
            title={!native ? "Available in the desktop app" : "Preview this Mac"}
          >
            Live preview
          </Button>
        </div>

        <div className={cn("host-codes", hostRunning && "is-live")}>
          <div className="host-code">
            <div className="host-code__head">
              <span className="host-code__label">Connection code</span>
              <span className="mini-chip">
                <ShieldCheck size={11} />
                Reusable
              </span>
            </div>
            <div className={cn("host-code__value", (joinCodeLoading || !joinCode) && "is-loading")}>
              {joinCodeLoading ? "••••••-••••••" : joinCode || "——————-——————"}
            </div>
            <div className="host-code__actions">
              <Button
                size="sm"
                variant="secondary"
                leftIcon={joinCodeCopied ? <Check size={14} /> : <Copy size={14} />}
                onClick={() => void copyJoinCode()}
                disabled={!joinCode}
              >
                {joinCodeCopied ? "Copied" : "Copy"}
              </Button>
              <Button
                size="sm"
                variant="ghost"
                leftIcon={<RefreshCw size={14} className={codeLoading ? "spin" : undefined} />}
                onClick={() => void regenerateCode()}
                disabled={codeLoading || (native && !hostRunning)}
              >
                New code
              </Button>
            </div>
            <p className="host-code__note">
              {!hostRunning
                ? "Turn on remote access to get a code."
                : joinCode
                  ? "Send this to a friend. They paste it in Nexus."
                  : (joinCodeError ?? "Creating code…")}
            </p>
          </div>
        </div>
      </section>

      {hostError && (
        <div className="preview-notice preview-notice--error">
          <AlertTriangle size={16} />
          <p>
            <strong>Host notice.</strong> {hostError}
          </p>
        </div>
      )}

      <div className="host-sections">
          <Panel
            id="host-access"
            title="Access"
            description="What friends can do while connected."
            icon={<ShieldCheck size={18} />}
            footnote={
              native ? (
                <>
                  <ShieldCheck size={14} />
                  You can change these while hosting.
                </>
              ) : (
                <>
                  <Info size={14} />
                  Available in the desktop app.
                </>
              )
            }
          >
            <div className="setting-list">
              <SettingRow
                title="Keyboard and mouse"
                description="Let friends control this Mac. Off by default."
                icon={<MousePointer2 size={16} />}
              >
                <Toggle
                  checked={allowRemoteInput}
                  onChange={(value) => void toggleRemoteInput(value)}
                  label="Keyboard and mouse"
                  disabled={!native || !hostRunning}
                />
              </SettingRow>
              <SettingRow
                title="Controllers"
                description="Each friend gets their own controller. On by default."
                icon={<Gamepad2 size={16} />}
              >
                <Toggle
                  checked={host.gamepad}
                  onChange={(value) => void toggleGamepad(value)}
                  label="Controllers"
                  disabled={!native || !hostRunning}
                />
              </SettingRow>
              {hostStatus?.gamepadError ? (
                <p className="settings-note">Controllers aren't available right now.</p>
              ) : null}
              <SettingRow
                title="Clipboard"
                description="Share copied text both ways."
                icon={<Clipboard size={16} />}
              >
                <Toggle
                  checked={host.clipboard}
                  onChange={(value) => updateHost({ clipboard: value })}
                  label="Clipboard"
                />
              </SettingRow>
              <SettingRow
                title="Ask before connecting"
                description="Ask you before anyone connects."
                icon={<ShieldCheck size={16} />}
              >
                <Toggle
                  checked={host.requireApproval}
                  onChange={(value) => void toggleApproval(value)}
                  label="Ask before connecting"
                />
              </SettingRow>
            </div>

            <div className="settings-subhead">
              <LockKeyhole size={14} />
              macOS permissions
            </div>
            <div className="permission-list">
              {permissionMeta.map((entry) => {
                const granted = perms[entry.key];
                return (
                  <div className="permission-row" key={entry.kind}>
                    <span className={cn("permission-row__icon", granted && "is-granted")}>
                      {permissionIcon(entry.kind)}
                    </span>
                    <div className="permission-row__copy">
                      <p>
                        {entry.title}
                        <span className={cn("required-tag", entry.required && "is-required")}>
                          {entry.required ? "Required" : "Recommended"}
                        </span>
                      </p>
                      <span>{entry.description}</span>
                    </div>
                    <span className={cn("permission-status", granted && "is-granted")}>
                      {granted ? <Check size={13} /> : null}
                      {granted ? "Granted" : "Needed"}
                    </span>
                    {granted ? (
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => void openSettings(entry.kind)}
                      >
                        Open settings
                      </Button>
                    ) : (
                      <Button
                        size="sm"
                        variant="secondary"
                        loading={granting === entry.kind}
                        onClick={() => void grant(entry.kind)}
                      >
                        {native && entry.kind === "accessibility" ? "Open settings" : "Grant"}
                      </Button>
                    )}
                  </div>
                );
              })}
            </div>
          </Panel>

          <Panel
            id="host-video"
            title="Video"
            description="What friends see."
            icon={<Monitor size={18} />}
            footnote={
              native ? (
                <>
                  <Info size={14} />
                  Changes apply next time you start hosting.
                </>
              ) : undefined
            }
          >
            <div className="field-grid">
              <Select
                id="host-display"
                label="Display"
                value={String(host.displayIndex)}
                onChange={(event) =>
                  updateHost({ displayIndex: Number(event.target.value) })
                }
                options={
                  displays.length > 0
                    ? displays.map((entry) => ({
                        value: String(entry.index),
                        label: entry.label,
                      }))
                    : [
                        {
                          value: String(host.displayIndex),
                          label: displayLoading
                            ? "Looking for displays"
                            : displayError
                              ? "Display unavailable"
                              : `Display ${host.displayIndex + 1}`,
                        },
                      ]
                }
              />
              <Select
                id="host-quality"
                label="Quality"
                value={host.bitrate >= 20 ? "20" : host.bitrate <= 8 ? "8" : "12"}
                onChange={(event) => updateHost({ bitrate: Number(event.target.value) })}
                options={[
                  { value: "8", label: "Data saver" },
                  { value: "12", label: "Balanced" },
                  { value: "20", label: "High" },
                ]}
              />
            </div>

            {displayError ? (
              <p className="settings-note">
                {displayError}{" "}
                <Button size="sm" variant="ghost" onClick={() => void loadDisplays()}>
                  Try again
                </Button>
              </p>
            ) : (
              <p className="settings-note">
                The selected display is shared when hosting starts.
              </p>
            )}
          </Panel>

      </div>
    </div>
  );
}
