import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";
import type {
  AppSettings,
  NativeCaptureFrame,
  NativeCaptureOptions,
  NativeCaptureStats,
  NativeCaptureStopped,
  NativeCaptureSummary,
  NativeClientStats,
  NativeClientDisplayEvent,
  NativeClientStatus,
  NativeClientStopped,
  NativeClientSummary,
  NativeClientVideo,
  NativeDiscoveredHost,
  NativeDisplayInfo,
  NativeDisplaySize,
  NativeEngineStatus,
  NativeHostStatus,
  NativeIceSettings,
  NativeInputStatus,
  NativeJoinCode,
  NativeMouseButton,
  NativePublicAddress,
  NativeRemoteInput,
  NativeVideoBounds,
  NetworkInfo,
  PermissionKind,
  PermissionState,
  SystemInfo,
} from "./types";

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function fallbackSystemInfo(): SystemInfo {
  const ua = navigator.userAgent;
  const os = ua.includes("Mac") ? "macOS" : ua.includes("Windows") ? "Windows" : "Linux";
  return {
    hostname: "This computer",
    os,
    osVersion: "Preview environment",
    arch: "—",
    model: "—",
    chip: "—",
    appVersion: "0.1.0",
  };
}

export async function getSystemInfo(): Promise<SystemInfo> {
  if (!isTauri()) return fallbackSystemInfo();
  try {
    return await invoke<SystemInfo>("system_info");
  } catch {
    return fallbackSystemInfo();
  }
}

export async function getNativePermissions(): Promise<PermissionState | null> {
  if (!isTauri()) return null;
  try {
    const status = await invoke<{
      screenRecording: boolean;
      accessibility: boolean;
      inputMonitoring: boolean;
      platformSupported: boolean;
    }>("permission_status");
    if (!status.platformSupported) return null;
    return {
      screenRecording: status.screenRecording,
      accessibility: status.accessibility,
      inputMonitoring: status.inputMonitoring,
    };
  } catch {
    return null;
  }
}

export async function requestNativePermission(kind: PermissionKind): Promise<boolean | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<boolean>("request_permission", { kind });
  } catch {
    return null;
  }
}

export async function openPrivacyPane(kind: PermissionKind | "microphone"): Promise<boolean> {
  if (!isTauri()) return false;
  try {
    await invoke("open_privacy_settings", { kind });
    return true;
  } catch {
    return false;
  }
}

export async function windowAction(action: "minimize" | "close" | "maximize"): Promise<boolean> {
  if (!isTauri()) return false;
  try {
    const window = getCurrentWindow();
    if (action === "minimize") await window.minimize();
    if (action === "close") await window.close();
    if (action === "maximize") await window.toggleMaximize();
    return true;
  } catch {
    return false;
  }
}

export async function setAlwaysOnTop(value: boolean): Promise<void> {
  if (!isTauri()) return;
  try {
    await getCurrentWindow().setAlwaysOnTop(value);
  } catch {
    // Ignore unsupported window states.
  }
}

export async function setFullscreen(value: boolean): Promise<boolean> {
  if (isTauri()) {
    try {
      await getCurrentWindow().setFullscreen(value);
      return value;
    } catch {
      return false;
    }
  }

  try {
    if (value && !document.fullscreenElement) {
      await document.documentElement.requestFullscreen();
    } else if (!value && document.fullscreenElement) {
      await document.exitFullscreen();
    }
    return Boolean(document.fullscreenElement);
  } catch {
    return Boolean(document.fullscreenElement);
  }
}

export async function openExternal(url: string): Promise<void> {
  if (isTauri()) {
    try {
      await openUrl(url);
      return;
    } catch {
      // Fall through to a browser tab.
    }
  }
  window.open(url, "_blank", "noopener,noreferrer");
}

export type NativeCaptureStart =
  | { ok: true; summary: NativeCaptureSummary }
  | { ok: false; error: string };

export type NativeActionResult =
  | { ok: true }
  | { ok: false; error: string };

function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "The native operation failed.";
}

/** Splits a raw server list on commas and whitespace, trimming and dropping empties. */
function splitServerList(value: string): string[] {
  return value
    .split(/[\s,]+/)
    .map((entry) => entry.trim())
    .filter(Boolean);
}

/**
 * Builds the native ICE payload from the raw settings strings. Returns undefined
 * when nothing is configured, so the native side keeps its built-in defaults
 * (host candidates only) and contacts no third party.
 */
export function buildIceSettings(
  settings: Pick<
    AppSettings,
    "iceStunUrls" | "iceTurnUrls" | "iceTurnUsername" | "iceTurnCredential"
  >,
): NativeIceSettings | undefined {
  const stunUrls = splitServerList(settings.iceStunUrls);
  const turnUrls = splitServerList(settings.iceTurnUrls);
  const turnUsername = settings.iceTurnUsername.trim();
  const turnCredential = settings.iceTurnCredential.trim();
  if (stunUrls.length === 0 && turnUrls.length === 0 && !turnUsername && !turnCredential) {
    return undefined;
  }
  return { stunUrls, turnUrls, turnUsername, turnCredential };
}

export async function startNativeCapture(
  options: NativeCaptureOptions,
): Promise<NativeCaptureStart> {
  if (!isTauri()) {
    return {
      ok: false,
      error: "Live capture is available in the Nexus desktop app.",
    };
  }
  try {
    const summary = await invoke<NativeCaptureSummary>("native_engine_start", {
      options,
    });
    return { ok: true, summary };
  } catch (error) {
    return { ok: false, error: errorMessage(error) };
  }
}

export async function stopNativeCapture(): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("native_engine_stop");
  } catch {
    // The engine may already have stopped.
  }
}

export async function getNativeEngineStatus(): Promise<NativeEngineStatus | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<NativeEngineStatus>("native_engine_status");
  } catch {
    return null;
  }
}

export async function subscribeNativeCapture(handlers: {
  onFrame?: (frame: NativeCaptureFrame) => void;
  onStats?: (stats: NativeCaptureStats) => void;
  onStopped?: (event: NativeCaptureStopped) => void;
}): Promise<() => void> {
  if (!isTauri()) return () => {};
  try {
    const unlisteners = await Promise.all([
      listen<NativeCaptureFrame>("native-capture:frame", (event) =>
        handlers.onFrame?.(event.payload),
      ),
      listen<NativeCaptureStats>("native-capture:stats", (event) =>
        handlers.onStats?.(event.payload),
      ),
      listen<NativeCaptureStopped>("native-capture:stopped", (event) =>
        handlers.onStopped?.(event.payload),
      ),
    ]);
    return () => unlisteners.forEach((unlisten) => unlisten());
  } catch {
    return () => {};
  }
}

export async function getNativeInputStatus(): Promise<NativeInputStatus | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<NativeInputStatus>("native_input_status");
  } catch {
    return null;
  }
}

export async function armNativeInput(
  armed: boolean,
): Promise<{ status: NativeInputStatus | null; error?: string }> {
  if (!isTauri()) return { status: null, error: "Native input requires the desktop app." };
  try {
    const status = await invoke<NativeInputStatus>("native_input_arm", { armed });
    return { status };
  } catch (error) {
    return { status: null, error: errorMessage(error) };
  }
}

async function nativeInput(command: string, args: Record<string, unknown>): Promise<NativeActionResult> {
  if (!isTauri()) return { ok: false, error: "Native input requires the desktop app." };
  try {
    await invoke(command, args);
    return { ok: true };
  } catch (error) {
    return { ok: false, error: errorMessage(error) };
  }
}

export function moveNativePointer(x: number, y: number): Promise<NativeActionResult> {
  return nativeInput("native_input_pointer", { x, y });
}

export function pressNativeButton(
  button: NativeMouseButton,
  pressed: boolean,
): Promise<NativeActionResult> {
  return nativeInput("native_input_button", { button, pressed });
}

export function scrollNative(deltaX: number, deltaY: number): Promise<NativeActionResult> {
  return nativeInput("native_input_scroll", { deltaX, deltaY });
}

export function typeNativeText(text: string): Promise<NativeActionResult> {
  return nativeInput("native_input_text", { text });
}

export async function startNativeHost(input: {
  options: NativeCaptureOptions;
  port?: number;
  code?: string;
  requireApproval: boolean;
  adaptiveBitrate?: boolean;
  relayAddress?: string;
  ice?: NativeIceSettings;
  mediaPort?: number;
  gamepad?: boolean;
}): Promise<NativeHostStatus | { error: string }> {
  if (!isTauri()) return { error: "Direct hosting is available in the Nexus desktop app." };
  try {
    return await invoke<NativeHostStatus>("native_host_start", {
      options: input.options,
      port: input.port,
      code: input.code,
      requireApproval: input.requireApproval,
      adaptiveBitrate: input.adaptiveBitrate,
      relayAddress: input.relayAddress,
      ice: input.ice,
      mediaPort: input.mediaPort,
      gamepad: input.gamepad,
    });
  } catch (error) {
    return { error: errorMessage(error) };
  }
}

export async function stopNativeHost(): Promise<NativeHostStatus | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<NativeHostStatus>("native_host_stop");
  } catch {
    return null;
  }
}

export async function getNativeHostStatus(): Promise<NativeHostStatus | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<NativeHostStatus>("native_host_status");
  } catch {
    return null;
  }
}

export async function rotateNativeHostCode(): Promise<{
  status?: NativeHostStatus;
  error?: string;
}> {
  if (!isTauri()) return { error: "Direct hosting is available in the Nexus desktop app." };
  try {
    const status = await invoke<NativeHostStatus>("native_host_rotate_code");
    return { status };
  } catch (error) {
    return { error: errorMessage(error) };
  }
}

export async function setNativeHostGamepad(
  allow: boolean,
): Promise<{ status?: NativeHostStatus; error?: string }> {
  if (!isTauri()) return { error: "Controller passthrough requires the desktop app." };
  try {
    const status = await invoke<NativeHostStatus>("native_host_set_gamepad", { allow });
    return { status };
  } catch (error) {
    return { error: errorMessage(error) };
  }
}

export async function setNativeHostInput(
  allow: boolean,
): Promise<{ status?: NativeHostStatus; error?: string }> {
  if (!isTauri()) return { error: "Remote input requires the desktop app." };
  try {
    const status = await invoke<NativeHostStatus>("native_host_set_input", { allow });
    return { status };
  } catch (error) {
    return { error: errorMessage(error) };
  }
}

export async function setNativeHostApproval(
  require: boolean,
): Promise<{ status?: NativeHostStatus; error?: string }> {
  if (!isTauri()) return { error: "Approval settings require the desktop app." };
  try {
    const status = await invoke<NativeHostStatus>("native_host_set_approval", { require });
    return { status };
  } catch (error) {
    return { error: errorMessage(error) };
  }
}

export async function respondToHostRequest(
  peerId: string,
  approve: boolean,
): Promise<{ status?: NativeHostStatus; error?: string }> {
  if (!isTauri()) return { error: "Connection requests require the desktop app." };
  try {
    const status = await invoke<NativeHostStatus>("native_host_respond", { peerId, approve });
    return { status };
  } catch (error) {
    return { error: errorMessage(error) };
  }
}

export async function setHostPeerPermissions(
  peerId: string,
  permissions: { keyboard: boolean; mouse: boolean; gamepad: boolean },
): Promise<{ status?: NativeHostStatus; error?: string }> {
  if (!isTauri()) return { error: "Viewer permissions require the desktop app." };
  try {
    const status = await invoke<NativeHostStatus>("native_host_set_peer_permissions", {
      peerId,
      keyboard: permissions.keyboard,
      mouse: permissions.mouse,
      gamepad: permissions.gamepad,
    });
    return { status };
  } catch (error) {
    return { error: errorMessage(error) };
  }
}

export async function kickHostViewer(
  peerId: string,
): Promise<{ status?: NativeHostStatus; error?: string }> {
  if (!isTauri()) return { error: "Disconnecting a viewer requires the desktop app." };
  try {
    const status = await invoke<NativeHostStatus>("native_host_kick", { peerId });
    return { status };
  } catch (error) {
    return { error: errorMessage(error) };
  }
}

/** Native backing-pixel size of the main display, for the "Match display" preset. */
export async function listNativeDisplays(): Promise<NativeDisplayInfo[] | null> {
  if (!isTauri()) return null;
  try {
    const displays = await invoke<NativeDisplayInfo[]>("native_list_displays");
    return displays.length > 0 ? displays : null;
  } catch {
    return null;
  }
}

export async function getNativeDisplaySize(): Promise<NativeDisplaySize | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<NativeDisplaySize>("native_display_size");
  } catch {
    return null;
  }
}

export async function subscribeNativeHost(handlers: {
  onStatus?: (status: NativeHostStatus) => void;
}): Promise<() => void> {
  if (!isTauri()) return () => {};
  try {
    const unlisteners = await Promise.all([
      listen<NativeHostStatus>("native-host:status", (event) =>
        handlers.onStatus?.(event.payload),
      ),
    ]);
    return () => unlisteners.forEach((unlisten) => unlisten());
  } catch {
    return () => {};
  }
}

export type NativeClientConnectResult =
  | { ok: true; summary: NativeClientSummary }
  | { ok: false; error: string };

export async function connectNativeClient(input: {
  address: string;
  port?: number;
  code: string;
  ownerName?: string;
  relayInvite?: string;
  ice?: NativeIceSettings;
}): Promise<NativeClientConnectResult> {
  if (!isTauri()) {
    return { ok: false, error: "Direct sessions are available in the Nexus desktop app." };
  }
  try {
    const summary = await invoke<NativeClientSummary>("native_client_connect", {
      address: input.address,
      port: input.port,
      code: input.code,
      ownerName: input.ownerName,
      relayInvite: input.relayInvite,
      ice: input.ice,
    });
    return { ok: true, summary };
  } catch (error) {
    return { ok: false, error: errorMessage(error) };
  }
}

export type NativePublicAddressResult =
  | { ok: true; value: NativePublicAddress }
  | { ok: false; error: string };

export async function discoverPublicAddress(
  stunUrl: string,
  localPort?: number,
): Promise<NativePublicAddressResult> {
  if (!isTauri()) {
    return { ok: false, error: "Public address discovery needs the desktop app." };
  }
  try {
    const value = await invoke<NativePublicAddress>("native_discover_public_address", {
      stunUrl,
      localPort,
    });
    return { ok: true, value };
  } catch (error) {
    return { ok: false, error: errorMessage(error) };
  }
}

export type NativeJoinCodeResult =
  | { ok: true; value: string }
  | { ok: false; error: string };

export type NativeJoinCodeParseResult =
  | { ok: true; value: NativeJoinCode }
  | { ok: false; error: string };

/**
 * Lists the local network addresses the host can share, preferring addresses
 * that belong to a Tailscale interface. Returns null outside the desktop app.
 */
export async function listNativeNetworkAddresses(): Promise<NetworkInfo | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<NetworkInfo>("native_network_addresses");
  } catch {
    return null;
  }
}

/** Packs an address, TCP port, and pairing code into one shareable join code. */
export async function makeJoinCode(
  address: string,
  port: number,
  code: string,
): Promise<NativeJoinCodeResult> {
  if (!isTauri()) return { ok: false, error: "Join codes need the desktop app." };
  try {
    const value = await invoke<string>("native_make_join_code", { address, port, code });
    return { ok: true, value };
  } catch (error) {
    return { ok: false, error: errorMessage(error) };
  }
}

/** Decodes a `PC1-…` join code back into its address, port, and pairing code. */
export async function parseJoinCode(joinCode: string): Promise<NativeJoinCodeParseResult> {
  if (!isTauri()) return { ok: false, error: "Join codes need the desktop app." };
  try {
    const value = await invoke<NativeJoinCode>("native_parse_join_code", { joinCode });
    return { ok: true, value };
  } catch (error) {
    return { ok: false, error: errorMessage(error) };
  }
}

export async function discoverNativeHosts(
  timeoutMs?: number,
): Promise<{ hosts: NativeDiscoveredHost[]; error?: string }> {
  if (!isTauri()) {
    return { hosts: [], error: "Nearby discovery needs the desktop app." };
  }
  try {
    const hosts = await invoke<NativeDiscoveredHost[]>("native_discover_hosts", {
      timeoutMs,
    });
    return { hosts };
  } catch (error) {
    return { hosts: [], error: errorMessage(error) };
  }
}

export async function disconnectNativeClient(): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("native_client_disconnect");
  } catch {
    // The session may already have ended.
  }
}

export async function getNativeClientStatus(): Promise<NativeClientStatus | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<NativeClientStatus>("native_client_status");
  } catch {
    return null;
  }
}

export async function sendNativeClientInput(event: NativeRemoteInput): Promise<boolean> {
  if (!isTauri()) return false;
  try {
    await invoke("native_client_input", { event });
    return true;
  } catch {
    return false;
  }
}

export async function requestNativeClientDisplay(index: number): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("native_client_request_display", { index });
  } catch {
    // Best effort; the host ignores unknown or unavailable displays.
  }
}

export async function requestNativeClientKeyframe(): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("native_client_request_keyframe");
  } catch {
    // Best effort: the host also emits periodic keyframes.
  }
}

export async function getNativeVideoSize(): Promise<NativeClientVideo | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<NativeClientVideo | null>("native_video_size");
  } catch {
    return null;
  }
}

export async function attachNativeVideo(bounds: NativeVideoBounds): Promise<void> {
  if (!isTauri()) return;
  await invoke("native_video_attach", { bounds });
}

export async function updateNativeVideoBounds(bounds: NativeVideoBounds): Promise<void> {
  if (!isTauri()) return;
  await invoke("native_video_bounds", { bounds });
}

export async function detachNativeVideo(): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("native_video_detach");
  } catch {
    // The surface may already be gone.
  }
}

export async function subscribeNativeClient(handlers: {
  onVideo?: (video: NativeClientVideo) => void;
  onVideoError?: (message: string) => void;
  onStats?: (stats: NativeClientStats) => void;
  onStopped?: (event: NativeClientStopped) => void;
  onInput?: (event: { enabled: boolean }) => void;
  onDisplay?: (event: NativeClientDisplayEvent) => void;
}): Promise<() => void> {
  if (!isTauri()) return () => {};
  try {
    const unlisteners = await Promise.all([
      listen<NativeClientVideo>("native-client:video", (event) =>
        handlers.onVideo?.(event.payload),
      ),
      listen<{ message: string }>("native-video:error", (event) =>
        handlers.onVideoError?.(event.payload.message),
      ),
      listen<NativeClientStats>("native-client:stats", (event) =>
        handlers.onStats?.(event.payload),
      ),
      listen<NativeClientStopped>("native-client:stopped", (event) =>
        handlers.onStopped?.(event.payload),
      ),
      listen<{ enabled: boolean }>("native-client:input", (event) =>
        handlers.onInput?.(event.payload),
      ),
      listen<NativeClientDisplayEvent>("native-client:display", (event) =>
        handlers.onDisplay?.(event.payload),
      ),
    ]);
    return () => unlisteners.forEach((unlisten) => unlisten());
  } catch {
    return () => {};
  }
}
