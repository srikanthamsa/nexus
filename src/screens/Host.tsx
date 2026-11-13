import { useEffect, useState, type ReactNode } from "react";
import {
  Activity,
  AlertTriangle,
  Check,
  Clipboard,
  Copy,
  Cpu,
  FolderUp,
  Gamepad2,
  Gauge,
  Globe2,
  Info,
  Keyboard,
  Link2,
  LockKeyhole,
  Monitor,
  MonitorUp,
  MousePointer2,
  Play,
  Radio,
  RefreshCw,
  Router,
  ShieldCheck,
  SlidersHorizontal,
  Users,
  Volume2,
  Wifi,
} from "lucide-react";
import { Button } from "../components/Button";
import { PageHeader } from "../components/PageHeader";
import { Segmented } from "../components/Segmented";
import { Select } from "../components/Select";
import { TextField } from "../components/TextField";
import { Toggle } from "../components/Toggle";
import { useToast } from "../components/Toast";
import {
  buildIceSettings,
  discoverPublicAddress,
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
  NativePublicAddress,
  NativeRelayState,
  NetworkInfo,
  PermissionKind,
  PermissionState,
  SystemInfo,
} from "../lib/types";
import { cn, copyToClipboard, formatDuration, friendlyDeviceName } from "../lib/utils";

const DEFAULT_HOST_TCP_PORT = 39200;
const DEFAULT_MEDIA_PORT = 39201;
const DEFAULT_PUBLIC_STUN = "stun:stun.l.google.com:19302";
const BITRATE_PRESETS = [8, 12, 20, 35] as const;
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

function firstServer(value: string): string | undefined {
  return value
    .split(/[\s,]+/)
    .map((entry) => entry.trim())
    .find(Boolean);
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
    description: "Required to capture and encode this display.",
    required: true,
  },
  {
    kind: "accessibility",
    key: "accessibility",
    title: "Accessibility",
    description: "Lets visitors control the pointer and keyboard.",
    required: true,
  },
  {
    kind: "input",
    key: "inputMonitoring",
    title: "Input Monitoring",
    description: "Recommended for gamepads and low-level input.",
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

function relayStateLabel(state: NativeRelayState): string {
  if (state === "connecting") return "Connecting";
  if (state === "waiting") return "Waiting";
  if (state === "paired") return "Connected";
  return "Offline";
}

export function HostScreen({
  onStartPreview,
}: {
  onStartPreview: (summary: NativeCaptureSummary) => void;
}) {
  const { data, updateHost } = useApp();
  const { push } = useToast();
  const [system, setSystem] = useState<SystemInfo | null>(null);
  const [nativeAvailable, setNativeAvailable] = useState(false);
  const [perms, setPerms] = useState<PermissionState>(data.permissions);
  const [granting, setGranting] = useState<PermissionKind | null>(null);
  const [pairCode, setPairCode] = useState("");
  const [codeLoading, setCodeLoading] = useState(true);
  const [previewStarting, setPreviewStarting] = useState(false);
  const [hostStatus, setHostStatus] = useState<NativeHostStatus | null>(null);
  const [displaySize, setDisplaySize] = useState<NativeDisplaySize | null>(null);
  const [displays, setDisplays] = useState<NativeDisplayInfo[]>([]);
  const [displayLoading, setDisplayLoading] = useState(false);
  const [displayError, setDisplayError] = useState<string | null>(null);
  const [allowRemoteInput, setAllowRemoteInput] = useState(false);
  const [detectingAddress, setDetectingAddress] = useState(false);
  const [publicAddress, setPublicAddress] = useState<NativePublicAddress | null>(null);
  const [detectError, setDetectError] = useState<string | null>(null);
  const [networkInfo, setNetworkInfo] = useState<NetworkInfo | null>(null);
  const [networkLoading, setNetworkLoading] = useState(false);
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
  const primaryViewer = viewers[0] ?? null;
  const hostError = native ? hostStatus?.lastError ?? null : null;
  const relay = native ? hostStatus?.summary?.relay ?? null : null;
  const nearbyAdvertised = native ? Boolean(hostStatus?.summary?.nearby) : false;
  const route =
    viewers.length > 0
      ? relay?.state === "paired"
        ? "Relay"
        : "Direct"
      : hostRunning
        ? "Listening"
        : "Idle";
  const relayAddress = host.relayAddress.trim();
  const mediaPort = clampMediaPort(host.mediaPort);
  const tcpPort = hostStatus?.summary?.port ?? DEFAULT_HOST_TCP_PORT;
  const shareAddresses = networkInfo?.addresses ?? [];
  const tailscale = networkInfo?.tailscale ?? null;
  const hostingCode = hostStatus?.summary?.code ?? "";
  const hostingPort = hostStatus?.summary?.port ?? DEFAULT_HOST_TCP_PORT;
  const relayMeta = relay
    ? relayStateLabel(relay.state)
    : host.relayFallback
      ? relayAddress
        ? hostRunning
          ? "Not started"
          : "Ready"
        : "No address"
      : "Off";
  const matchDimensions = displaySize ? matchCaptureDimensions(displaySize) : null;
  const resolutionLabel =
    host.resolution === "match"
      ? matchDimensions
        ? `${matchDimensions.width} × ${matchDimensions.height}`
        : "Native display"
      : host.resolution.replace("x", " × ");
  const bitrateOptions = [
    ...BITRATE_PRESETS.map((preset) => ({ value: String(preset), label: `${preset} Mbps` })),
    ...((BITRATE_PRESETS as readonly number[]).includes(host.bitrate)
      ? []
      : [{ value: String(host.bitrate), label: `Custom ${host.bitrate} Mbps` }]),
  ];

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
      if (status.running && status.summary) setPairCode(status.summary.code);
    };

    void subscribeNativeHost({ onStatus: apply }).then((stop) => {
      if (cancelled) stop();
      else unsubscribe = stop;
    });

    void getNativeHostStatus().then((status) => {
      if (cancelled || !status) return;
      apply(status);
      if (!status.running) setPairCode("");
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
        setNativeAvailable(true);
        setPerms(fresh);
      } else {
        setNativeAvailable(false);
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
    setNetworkLoading(true);
    void listNativeNetworkAddresses().then((info) => {
      if (cancelled) return;
      setNetworkLoading(false);
      if (!info) return;
      setNetworkInfo(info);
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
          "Run the Tauri build to open the encrypted listener, issue invite codes, and accept direct sessions.",
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
        relayAddress: host.relayFallback && relayAddress ? relayAddress : undefined,
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
      if (status.summary) setPairCode(status.summary.code);
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
    setPairCode("");
    updateHost({ hosting: false });
    push({
      title: "Hosting stopped",
      description: "The listener is closed and the current code no longer works.",
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
          title: "Approve ParsecPlus in System Settings",
          description: "Enable the toggle, then return here. Status refreshes automatically.",
          tone: "info",
        });
      } else {
        push({
          title: result ? "Permission granted" : "Permission requested",
          description: result
            ? "ParsecPlus can now use this capability."
            : "Approve ParsecPlus in System Settings, then return here.",
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

  const detectPublicAddress = async () => {
    setDetectingAddress(true);
    setDetectError(null);
    setPublicAddress(null);
    const stunUrl = firstServer(data.settings.iceStunUrls) ?? DEFAULT_PUBLIC_STUN;
    const result = await discoverPublicAddress(stunUrl, mediaPort);
    setDetectingAddress(false);
    if (!result.ok) {
      setDetectError(result.error);
      push({
        title: "Could not detect public address",
        description: result.error,
        tone: "error",
      });
      return;
    }
    setPublicAddress(result.value);
    push({
      title: "Public address discovered",
      description: `Public IP ${result.value.address}; your friend connects to ${result.value.address}:${tcpPort}.`,
      tone: "success",
    });
  };

  const regenerateCode = async () => {
    if (native) {
      if (!hostRunning) {
        push({
          title: "Start hosting first",
          description: "The listener issues the invite code.",
          tone: "info",
        });
        return;
      }
      setCodeLoading(true);
      const result = await rotateNativeHostCode();
      setCodeLoading(false);
      if (result.status) {
        setHostStatus(result.status);
        if (result.status.summary) setPairCode(result.status.summary.code);
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
      title: "Invite codes come from the host listener",
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

  const copyCode = async () => {
    if (!pairCode) return;
    const ok = await copyToClipboard(pairCode);
    push({
      title: ok ? "Invite code copied" : "Could not copy invite code",
      description: ok ? pairCode : "Copy the code manually instead.",
      tone: ok ? "success" : "error",
    });
  };

  const copyAddress = async () => {
    const summary = hostStatus?.summary;
    if (!summary) return;
    const value = `${summary.address}:${summary.port}`;
    const ok = await copyToClipboard(value);
    push({
      title: ok ? "Direct address copied" : "Could not copy address",
      description: value,
      tone: ok ? "success" : "error",
    });
  };

  const copyRelayInvite = async () => {
    if (!relay?.invite) return;
    const ok = await copyToClipboard(relay.invite);
    push({
      title: ok ? "Relay invite copied" : "Could not copy invite",
      description: ok ? "Share it with the invite code." : "Copy the invite manually instead.",
      tone: ok ? "success" : "error",
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
          "Run the Tauri build to capture this Mac with ScreenCaptureKit and hardware HEVC.",
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
        setNativeAvailable(true);
        setPerms(fresh);
        screenGranted = fresh.screenRecording;
      }
      if (!screenGranted) {
        setPreviewStarting(false);
        push({
          title: "Screen Recording permission needed",
          description:
            "Approve ParsecPlus in System Settings, then relaunch the app before starting a live preview.",
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

  const grantedCount = permissionMeta.filter((entry) => perms[entry.key]).length;

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
        <div className="host-hero__glow" aria-hidden="true" />
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
                ? hostStatus?.summary
                  ? `Encrypted listener on ${hostStatus.summary.address}:${hostStatus.summary.port}. Share an invite code when you want someone to join.`
                  : "Remote access is ready."
                : "Turn on remote access to open an encrypted listener other ParsecPlus devices can join."}
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
            <Cpu size={14} />
            {system
              ? [system.os, system.osVersion, system.chip].filter(Boolean).join(" · ")
              : native
                ? "ScreenCaptureKit · HEVC"
                : "Engine preview"}
          </span>
          <span>
            <MonitorUp size={14} />
            {hostRunning
              ? `${resolutionLabel} · ${Math.min(60, Number(host.refresh))} fps`
              : `Capture ${resolutionLabel}`}
          </span>
          <span>
            <ShieldCheck size={14} />
            {host.requireApproval ? "Approval required" : "Instant access"}
          </span>
          {hostRunning && (
            <span>
              <Users size={14} />
              {viewers.length} connected · {pendingRequests.length} waiting
            </span>
          )}
          {primaryViewer ? (
            <>
              <span>
                <Activity size={14} />
                {`${Math.round(primaryViewer.fps)} fps · ${primaryViewer.mbps.toFixed(1)} Mbps · ${formatDuration(primaryViewer.uptimeSecs)}`}
              </span>
              <span>
                <Gauge size={14} />
                {primaryViewer.adaptive
                  ? `Adaptive ${primaryViewer.bitrateMbps.toFixed(0)} / ${primaryViewer.ceilingMbps.toFixed(0)} Mbps`
                  : `Fixed ${primaryViewer.ceilingMbps.toFixed(0)} Mbps`}
              </span>
              {primaryViewer.icePath ? (
                <span>
                  <Router size={14} />
                  Path: {primaryViewer.icePath}
                </span>
              ) : null}
              <span>
                <MousePointer2 size={14} />
                {primaryViewer.keyboard || primaryViewer.mouse ? "Input allowed" : "View only"}
              </span>
              {viewers.length > 1 ? <span>+{viewers.length - 1} more</span> : null}
            </>
          ) : null}
          <Button
            size="sm"
            variant="secondary"
            className="host-hero__preview"
            leftIcon={<Play size={13} />}
            loading={previewStarting}
            onClick={() => void startPreview()}
            title={
              !native
                ? "Available in the ParsecPlus desktop app"
                : perms.screenRecording
                  ? "Capture this Mac and encode HEVC in hardware"
                  : "Screen Recording permission is required"
            }
          >
            Start live preview
          </Button>
        </div>

        <div className={cn("host-codes", hostRunning && "is-live")}>
          <div className="host-code">
            <div className="host-code__head">
              <span className="host-code__label">Invite code</span>
              <span className="mini-chip">
                <ShieldCheck size={11} />
                Reusable
              </span>
            </div>
            <div
              className={cn("host-code__value", (codeLoading || !pairCode) && "is-loading")}
            >
              {codeLoading ? "••••-••••" : pairCode || "————-————"}
            </div>
            <div className="host-code__actions">
              <Button
                size="sm"
                variant="secondary"
                leftIcon={<Copy size={14} />}
                onClick={() => void copyCode()}
                disabled={codeLoading || !pairCode}
              >
                Copy
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
            {hostRunning && hostStatus?.summary ? (
              <div className="host-code__address">
                <Link2 size={12} />
                <code>
                  {hostStatus.summary.address}:{hostStatus.summary.port}
                </code>
                <Button
                  size="sm"
                  variant="ghost"
                  leftIcon={<Copy size={12} />}
                  onClick={() => void copyAddress()}
                >
                  Copy
                </Button>
              </div>
            ) : null}
            <p className="host-code__note">
              {viewers.length > 0
                ? `${viewers.length === 1 ? (viewers[0]?.device ?? "A device") : `${viewers.length} devices`} connected. A fresh code is issued when the session ends.`
                : hostRunning
                  ? native
                    ? relay && relay.state !== "offline"
                      ? "Direct and relay routes are live. The code stays valid for saved computers until you rotate it."
                      : "This invite code stays valid for saved computers. Rotate it manually if it leaks."
                    : "Share this invite code with the person you want to join. It stays valid until you rotate it."
                  : native
                    ? "Remote access is off; an invite code appears here after you turn it on."
                    : "Invite codes are issued by the desktop app's host listener."}
            </p>
          </div>

          <div className="host-code">
            <div className="host-code__head">
              <span className="host-code__label">Direct connection</span>
              <span className="mini-chip">
                <Link2 size={11} />
                Tailscale-ready
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
            </div>
            <p className="host-code__note">
              {!hostRunning
                ? "Remote access is off; the direct connection string appears here after you turn it on."
                : !selectedAddress
                  ? "Choose a network address in Network."
                  : joinCode
                    ? "One string with the address, port and invite code. Send it to the person you want to join."
                    : (joinCodeError ?? "Generating direct connection…")}
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

      <div className="preview-notice">
        <Info size={16} />
        <p>
          <strong>Direct transport.</strong>{" "}
          {native
            ? "Pair another Mac with the code and direct address. Frames travel over an encrypted Noise channel; input stays view-only until you allow it."
            : "Hosting and direct connect run in the desktop app. This browser window can't open a listener."}
        </p>
      </div>

      <div className="host-sections">
          <Panel
            id="host-access"
            title="Access"
            description="Decide what visitors can do while they're connected."
            icon={<ShieldCheck size={18} />}
            aside={
              <span className={cn("chip", nativeAvailable && "chip--live")}>
                {nativeAvailable ? "Checked live" : "Desktop only"} · {grantedCount}/3
              </span>
            }
            footnote={
              native ? (
                <>
                  <ShieldCheck size={14} />
                  Requests appear over any screen. Declining or letting one expire rotates the
                  invite code. Permission status refreshes when you return to this window.
                </>
              ) : (
                <>
                  <Info size={14} />
                  macOS permissions and connection approval are available in the desktop app.
                </>
              )
            }
          >
            <div className="setting-list">
              <SettingRow
                title="Allow remote input"
                description={
                  native
                    ? hostRunning
                      ? "Let the paired device control this Mac. Requires Accessibility and stops when the session ends."
                      : "Turn on hosting to arm pointer and keyboard control."
                    : "Available in the desktop app's direct sessions."
                }
                icon={<MousePointer2 size={16} />}
              >
                <Toggle
                  checked={allowRemoteInput}
                  onChange={(value) => void toggleRemoteInput(value)}
                  label="Allow remote input"
                  disabled={!native || !hostRunning}
                />
              </SettingRow>
              <SettingRow
                title="Require approval"
                description={
                  native
                    ? "Review each device before its session starts. Requests stay open for up to two minutes."
                    : "Accept each connection before it starts."
                }
                icon={<ShieldCheck size={16} />}
              >
                <Toggle
                  checked={host.requireApproval}
                  onChange={(value) => void toggleApproval(value)}
                  label="Require approval"
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
            description="Capture size, motion, and encoding."
            icon={<MonitorUp size={18} />}
            footnote={
              native ? (
                <>
                  <Info size={14} />
                  Capture, bitrate, adaptive, and frame-rate changes apply the next time hosting
                  starts.
                </>
              ) : undefined
            }
          >
            <div className="field-grid">
              <Select
                id="host-display"
                label="Shared display"
                value={String(host.displayIndex)}
                onChange={(event) =>
                  updateHost({ displayIndex: Number(event.target.value) })
                }
                options={
                  displays.length > 0
                    ? displays.map((entry) => ({
                        value: String(entry.index),
                        label: `${entry.label} · ${entry.width} × ${entry.height}`,
                      }))
                    : [
                        {
                          value: String(host.displayIndex),
                          label: displayLoading
                            ? "Detecting displays…"
                            : displayError
                              ? "Displays unavailable"
                              : `Display ${host.displayIndex + 1}`,
                        },
                      ]
                }
              />
              <Select
                id="host-resolution"
                label="Resolution"
                value={host.resolution}
                onChange={(event) =>
                  updateHost({ resolution: event.target.value as typeof host.resolution })
                }
                options={[
                  {
                    value: "match",
                    label: matchDimensions
                      ? `Match display · ${matchDimensions.width} × ${matchDimensions.height}`
                      : "Match display · native",
                  },
                  { value: "1920x1080", label: "1920 × 1080 · 16:9" },
                  { value: "2560x1440", label: "2560 × 1440 · 16:9" },
                  { value: "3840x2160", label: "3840 × 2160 · 16:9" },
                ]}
              />
              <Select
                id="host-refresh"
                label="Refresh rate"
                value="60"
                onChange={() => {}}
                options={[{ value: "60", label: "60 Hz" }]}
                hint="This build caps capture at 60 fps; 120 Hz+ is planned."
              />
              <Select
                id="host-codec"
                label="Codec"
                value={host.codec === "hevc" ? "hevc" : "auto"}
                onChange={(event) =>
                  updateHost({ codec: event.target.value as typeof host.codec })
                }
                options={[
                  { value: "auto", label: "Auto (HEVC)" },
                  { value: "hevc", label: "HEVC" },
                ]}
                hint="HEVC is the only hardware encoder in this build. H.264 and AV1 are planned."
              />
            </div>

            {displayError ? (
              <p className="settings-note">
                {displayError}{" "}
                <Button size="sm" variant="ghost" onClick={() => void loadDisplays()}>
                  Retry
                </Button>
              </p>
            ) : (
              <p className="settings-note">
                ScreenCaptureKit lists every shareable display. The selected display is used the
                next time hosting or live preview starts.
              </p>
            )}

            <p className="settings-note">
              Capture size: <strong>{resolutionLabel}</strong>
              {host.resolution === "match"
                ? displaySize
                  ? ` · aspect-matched from ${displaySize.width} × ${displaySize.height}`
                  : " · measuring this display…"
                : ""}
            </p>

            <div className="settings-subhead">
              <Gauge size={14} />
              Bitrate
            </div>
            <Segmented
              size="sm"
              ariaLabel="Bitrate preset"
              value={String(host.bitrate)}
              onChange={(value) => updateHost({ bitrate: Number(value) })}
              options={bitrateOptions}
            />
            <p className="settings-note">
              Internet and Wi-Fi sessions should stay at 8–12 Mbps. Adaptive bitrate lowers the
              encoder target whenever the link falls behind.
            </p>

            <div className="setting-list">
              <SettingRow
                title="HDR"
                description="Not implemented yet: HDR capture and tone mapping are planned for a later phase."
              >
                <Toggle
                  checked={false}
                  disabled
                  onChange={() => {}}
                  label="HDR (planned)"
                />
              </SettingRow>
            </div>
          </Panel>

          <Panel
            id="host-network"
            title="Network"
            description="How ParsecPlus finds the fastest route."
            icon={<Activity size={18} />}
            footnote={
              native ? (
                <>
                  <Info size={14} />
                  Network changes apply the next time hosting starts.
                </>
              ) : undefined
            }
          >
            <div className="setting-list">
              <SettingRow
                title="Adaptive bitrate"
                description="Lower the encoder target when the encrypted link falls behind, then recover as it stabilizes."
              >
                <Toggle
                  checked={host.adaptNetwork}
                  onChange={(value) => updateHost({ adaptNetwork: value })}
                  label="Adaptive bitrate"
                />
              </SettingRow>
              <SettingRow
                title="Relay invite"
                description="Publish an encrypted relay route for devices that can't reach this Mac directly."
              >
                <Toggle
                  checked={host.relayFallback}
                  onChange={(value) => updateHost({ relayFallback: value })}
                  label="Relay invite"
                />
              </SettingRow>
            </div>

            {host.relayFallback ? (
              <TextField
                id="host-relay-address"
                containerClassName="network-relay-input"
                label="Relay address"
                placeholder="relay.example.com:39210"
                value={host.relayAddress}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                onChange={(event) => updateHost({ relayAddress: event.target.value })}
                hint="Self-hosted relay reachable by both devices. Leave blank for nearby-only hosting."
              />
            ) : null}

            <TextField
              id="host-media-port"
              containerClassName="network-relay-input"
              label="Media UDP port"
              type="number"
              min={1024}
              max={65535}
              value={host.mediaPort}
              onChange={(event) => {
                const raw = event.target.value.trim();
                if (!raw) return;
                const next = Number(raw);
                if (Number.isFinite(next)) updateHost({ mediaPort: next });
              }}
              onBlur={() => updateHost({ mediaPort: clampMediaPort(host.mediaPort) })}
              hint="Forward this UDP port on your router for direct video without a relay."
            />

            <div className="network-test">
              <div className="network-test__head">
                <span className="network-test__icon">
                  <Link2 size={20} />
                </span>
                <div>
                  <p>Share with a friend</p>
                  <span>One string carries this Mac's address, port, and reusable invite code.</span>
                </div>
              </div>
              {tailscale ? (
                <p className="settings-note">
                  Tailscale <strong>{tailscale.tailnet || tailscale.hostName || "tailnet"}</strong>{" "}
                  · {tailscale.status}
                  <br />
                  Your friend must be on this Tailscale tailnet.
                </p>
              ) : null}
              {shareAddresses.length > 0 ? (
                <div className="network-relay-input">
                  <Select
                    id="host-share-address"
                    label="Share this address"
                    value={selectedAddress}
                    onChange={(event) => setSelectedAddress(event.target.value)}
                    options={shareAddresses.map((entry) => ({
                      value: entry.address,
                      label: entry.label,
                    }))}
                  />
                </div>
              ) : (
                <p className="settings-note">
                  {networkLoading
                    ? "Looking for network addresses…"
                    : "No network addresses reported by this Mac."}
                </p>
              )}
              {hostRunning ? (
                joinCode ? (
                  <>
                    <div className="pairing-box__address">
                      <span className="pairing-box__address-label">
                        <Link2 size={12} />
                        Direct connection
                      </span>
                      <code title={joinCode}>{joinCode}</code>
                      <Button
                        size="sm"
                        variant="ghost"
                        leftIcon={joinCodeCopied ? <Check size={12} /> : <Copy size={12} />}
                        onClick={() => void copyJoinCode()}
                      >
                        {joinCodeCopied ? "Copied" : "Copy"}
                      </Button>
                    </div>
                    <p className="settings-note">
                      Your friend opens Computers → Direct connect → Invite code, pastes this, and
                      you get an Approve/Reject request. The code stays valid for one-click reconnects until you rotate it.
                    </p>
                  </>
                ) : (
                  <p className="settings-note">
                    {joinCodeLoading
                      ? "Generating direct connection…"
                      : (joinCodeError ?? "Waiting for the invite code…")}
                  </p>
                )
              ) : (
                <p className="settings-note">Turn on remote access to generate a direct connection.</p>
              )}
            </div>

            <div className="network-test">
              <div className="network-test__head">
                <span className="network-test__icon">
                  <Globe2 size={20} />
                </span>
                <div>
                  <p>Internet test</p>
                  <span>
                    Forward these on your router so a friend outside your network can reach this
                    Mac.
                  </span>
                </div>
                <Button
                  variant="secondary"
                  size="sm"
                  loading={detectingAddress}
                  leftIcon={!detectingAddress ? <Globe2 size={14} /> : undefined}
                  onClick={() => void detectPublicAddress()}
                  disabled={!native}
                >
                  Detect public address
                </Button>
              </div>
              <div
                className="network-test__grid"
                style={{ gridTemplateColumns: "repeat(2, 1fr)" }}
              >
                <div>
                  <span>TCP · signaling</span>
                  <strong>{tcpPort}</strong>
                </div>
                <div>
                  <span>UDP · video</span>
                  <strong>{mediaPort}</strong>
                </div>
              </div>
              <p className="settings-note">
                Forward TCP {tcpPort} and UDP {mediaPort} on your router, then share the host's
                public address with your friend.
              </p>
              {publicAddress ? (
                <>
                  <p className="settings-note">
                    Public IP: <strong>{publicAddress.address}</strong> — your friend connects to{" "}
                    <span className="mono-chip">
                      {publicAddress.address}:{tcpPort}
                    </span>{" "}
                    · local {publicAddress.localAddress}
                  </p>
                  <p className="settings-note">
                    UDP mapping seen by STUN: <strong>{publicAddress.port}</strong>
                    {publicAddress.port === mediaPort
                      ? " — the forward is working."
                      : ` — forward external UDP ${publicAddress.port} to internal ${mediaPort} on this Mac.`}
                  </p>
                </>
              ) : null}
              {detectError ? <p className="network-error">{detectError}</p> : null}
              {!data.settings.iceStunUrls.trim() ? (
                <p className="settings-note">
                  No STUN server configured; detection uses {DEFAULT_PUBLIC_STUN}. Add your own
                  in Settings → Network.
                </p>
              ) : null}
            </div>

            {relay ? (
              <div
                className={cn(
                  "pairing-box__address",
                  "pairing-box__address--relay",
                  relay.state === "offline" && "pairing-box__address--offline",
                )}
              >
                <span className="pairing-box__address-label">
                  <Router size={12} />
                  Relay invite
                </span>
                <code title={relay.invite}>{relay.invite}</code>
                <Button
                  size="sm"
                  variant="ghost"
                  leftIcon={<Copy size={12} />}
                  onClick={() => void copyRelayInvite()}
                >
                  Copy
                </Button>
              </div>
            ) : null}

            <div className="network-meta">
              <span>Host port</span>
              <span className="mono-chip">
                TCP {hostStatus?.summary?.port ?? 39200}
              </span>
              <span>Nearby</span>
              <span className="mini-chip">
                <Wifi size={11} />
                {nearbyAdvertised ? "Advertised" : "Off"}
              </span>
              <span>Relay</span>
              <span className="mini-chip">
                <Router size={11} />
                {relayMeta}
              </span>
              <span>Route</span>
              <span className="mini-chip">
                <Activity size={11} />
                {route}
              </span>
            </div>
            {relay?.state === "offline" && relay.error ? (
              <p className="network-error">{relay.error}</p>
            ) : null}
          </Panel>

          <Panel
            id="host-advanced"
            title="Advanced"
            description="Display and peripheral overrides that rarely change."
            icon={<SlidersHorizontal size={18} />}
          >
              <div className="setting-list">
                <SettingRow
                  title="Virtual display · planned"
                  description="Headless displays ship in a later phase."
                >
                  <Toggle checked={false} disabled onChange={() => {}} label="Virtual display (planned)" />
                </SettingRow>
                <SettingRow
                  title="Controller passthrough"
                  description="Forward a viewer's controller to this Mac as a virtual gamepad. Requires the host app to be signed with the virtual-HID entitlement."
                  icon={<Gamepad2 size={16} />}
                >
                  <Toggle
                    checked={host.gamepad}
                    onChange={(value) => void toggleGamepad(value)}
                    label="Controller passthrough"
                  />
                </SettingRow>
                {hostStatus?.gamepadError ? (
                  <p className="network-error">{hostStatus.gamepadError}</p>
                ) : null}
                <SettingRow
                  title="Clipboard text sync"
                  description="Share copied text both ways. macOS may ask for pasteboard access on first use."
                  icon={<Clipboard size={16} />}
                >
                  <Toggle
                    checked={host.clipboard}
                    onChange={(value) => updateHost({ clipboard: value })}
                    label="Clipboard text sync"
                  />
                </SettingRow>
                <SettingRow
                  title="File transfer · planned"
                  description="Drag-and-drop files ship in a later phase."
                  icon={<FolderUp size={16} />}
                >
                  <Toggle checked={false} disabled onChange={() => {}} label="File transfer (planned)" />
                </SettingRow>
                <SettingRow
                  title="Host audio · planned"
                  description="System-audio streaming ships in a later phase."
                  icon={<Volume2 size={16} />}
                >
                  <Toggle checked={false} disabled onChange={() => {}} label="Host audio (planned)" />
                </SettingRow>
                <SettingRow
                  title="Lock cursor to window · planned"
                  description="Pointer locking ships with the input-depth phase."
                  icon={<MousePointer2 size={16} />}
                >
                  <Toggle checked={false} disabled onChange={() => {}} label="Lock cursor (planned)" />
                </SettingRow>
              </div>
          </Panel>
      </div>
    </div>
  );
}
