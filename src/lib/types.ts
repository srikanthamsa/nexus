export type Screen = "computers" | "friends" | "host" | "teams" | "arcade" | "settings";

export type Accent = "violet" | "aurora" | "ember" | "ocean";

/** Local, account-less profile for this Mac. */
export interface User {
  id: string;
  name: string;
}

/** A real direct session that was established from this Mac. */
export interface RecentConnection {
  id: string;
  name: string;
  address: string;
  port: number;
  lastConnectedAt: number;
  transport: "direct" | "relay";
  /** Invite code used for the session, kept for one-click reconnect. */
  code?: string;
  /** Stream size from the last successful session, for card metadata. */
  width?: number;
  height?: number;
  favorite?: boolean;
}

export interface HostSettings {
  hosting: boolean;
  /** Zero-based ScreenCaptureKit shareable-display index selected for capture. */
  displayIndex: number;
  resolution: "match" | "1920x1080" | "2560x1440" | "3840x2160";
  refresh: "60" | "120" | "144" | "240";
  codec: "auto" | "h264" | "hevc" | "av1";
  bitrate: number;
  hdr: boolean;
  virtualDisplay: boolean;
  gamepad: boolean;
  clipboard: boolean;
  fileTransfer: boolean;
  hostAudio: boolean;
  requireApproval: boolean;
  lockCursor: boolean;
  adaptNetwork: boolean;
  relayFallback: boolean;
  relayAddress: string;
  /** UDP port used for direct media; forward it on the router for internet sessions. */
  mediaPort: number;
}

export interface AppSettings {
  accent: Accent;
  reduceMotion: boolean;
  compact: boolean;
  alwaysOnTop: boolean;
  showStats: boolean;
  autoConnect: boolean;
  decoder: "auto" | "hardware" | "software";
  audioOutput: string;
  volume: number;
  muteWhenMinimized: boolean;
  networkMode: "auto" | "direct" | "relay";
  bandwidthLimit: boolean;
  bandwidthCap: number;
  overlayHotkey: string;
  pushToTalk: boolean;
  /** Raw comma/whitespace separated server lists as typed by the user. */
  iceStunUrls: string;
  iceTurnUrls: string;
  iceTurnUsername: string;
  iceTurnCredential: string;
}

export interface PermissionState {
  screenRecording: boolean;
  accessibility: boolean;
  inputMonitoring: boolean;
}

export interface AppData {
  user: User | null;
  host: HostSettings;
  settings: AppSettings;
  permissions: PermissionState;
  recent: RecentConnection[];
}

export interface ToastOptions {
  title: string;
  description?: string;
  tone?: "default" | "success" | "error" | "info";
}

export type PermissionKind = "screen" | "accessibility" | "input";

export interface SystemInfo {
  hostname: string;
  os: string;
  osVersion: string;
  arch: string;
  model: string;
  chip: string;
  appVersion: string;
}

export interface NativeCaptureOptions {
  displayIndex: number | null;
  width: number;
  height: number;
  frameRate: number;
  bitrateMbps: number;
  previewFps: number;
  previewMaxWidth: number;
  showsCursor: boolean;
  /** Mirror copied text between host and viewer over the encrypted channel. */
  clipboard: boolean;
}

export interface NativeCaptureSummary {
  codec: string;
  display: string;
  width: number;
  height: number;
  frameRate: number;
  bitrateMbps: number;
  startedAt: number;
}

export interface NativeEngineStatus {
  running: boolean;
  summary: NativeCaptureSummary | null;
}

export interface NativeCaptureFrame {
  data: string;
  width: number;
  height: number;
  sequence: number;
  timestampMs: number;
  keyframe: boolean;
  encodedBytes: number;
  encodeMs: number;
}

export interface NativeCaptureStats {
  sequence: number;
  captureFps: number;
  previewFps: number;
  encodeMs: number;
  previewMs: number;
  encodedKbps: number;
  previewKbps: number;
  width: number;
  height: number;
  uptimeSecs: number;
}

export interface NativeCaptureStopped {
  reason: string;
  frames: number;
  uptimeSecs: number;
  failed: boolean;
}

export interface NativeInputStatus {
  armed: boolean;
  accessibility: boolean;
  platformSupported: boolean;
}

export type NativeMouseButton = "left" | "right" | "middle";

export type NativeRelayState = "connecting" | "waiting" | "paired" | "offline";

export interface NativeRelaySummary {
  address: string;
  invite: string;
  state: NativeRelayState;
  error: string | null;
}

export interface NativeHostSummary {
  address: string;
  port: number;
  code: string;
  startedAt: number;
  relay: NativeRelaySummary | null;
  nearby: boolean;
}

export interface NativeHostPendingConnection {
  peerId: string;
  peer: string;
  device: string;
  owner: string;
  expiresInMs: number;
}

export interface NativeViewer {
  peerId: string;
  peer: string;
  device: string;
  owner: string;
  connectedAt: number;
  transport: "webrtc" | "tcp";
  keyboard: boolean;
  mouse: boolean;
  gamepad: boolean;
  fps: number;
  mbps: number;
  frames: number;
  uptimeSecs: number;
  bitrateMbps: number;
  ceilingMbps: number;
  adaptive: boolean;
  deliveryRatio: number;
  jitterMs: number;
  icePath?: string | null;
}

export interface NativeHostStatus {
  running: boolean;
  streaming: boolean;
  allowInput: boolean;
  /** Whether controller passthrough is enabled for this host run. */
  gamepad: boolean;
  /** Why the virtual gamepad could not be created, if it failed. */
  gamepadError: string | null;
  requireApproval: boolean;
  pending: NativeHostPendingConnection[];
  viewers: NativeViewer[];
  summary: NativeHostSummary | null;
  lastError: string | null;
}

export interface NativeDisplaySize {
  width: number;
  height: number;
}

export interface NativeDisplayInfo {
  index: number;
  label: string;
  width: number;
  height: number;
}

export interface NativeClientSummary {
  peer: string;
  hostName: string;
  width: number;
  height: number;
  allowInput: boolean;
  connectedAt: number;
  path: "direct" | "relay";
  transport: "tcp" | "webrtc";
  /** Host's shareable displays, sampled at connect time. */
  displays: NativeDisplayInfo[];
}

export interface NativeClientDisplayEvent {
  index: number;
  width: number;
  height: number;
}

export interface NativeClientStatus {
  connected: boolean;
  summary: NativeClientSummary | null;
  allowInput: boolean;
  lastError: string | null;
}

export interface NativeClientVideo {
  width: number;
  height: number;
}

export interface NativeVideoBounds {
  x: number;
  y: number;
  width: number;
  height: number;
  scale: number;
}

export interface NativeClientStats {
  sequence: number;
  fps: number;
  mbps: number;
  decodeMs: number;
  encodeMs: number;
  rttMs: number;
  frames: number;
  width: number;
  height: number;
  uptimeSecs: number;
  streamMbps: number;
  ceilingMbps: number;
  adaptive: boolean;
  jitterMs: number;
  dropped: number;
  /** Selected ICE candidate pair, e.g. "srflx → host". Optional until negotiated. */
  icePath?: string;
}

export interface NativeDiscoveredHost {
  name: string;
  address: string;
  port: number;
  platform: string;
  version: string;
}

export interface NativeIceSettings {
  stunUrls: string[];
  turnUrls: string[];
  turnUsername: string;
  turnCredential: string;
}

export interface NativePublicAddress {
  address: string;
  port: number;
  localAddress: string;
}

export interface ZeroTierInfo {
  networkId: string;
  name: string;
  status: string;
}

export interface NetworkAddress {
  interface: string;
  address: string;
  label: string;
  zeroTier: boolean;
}

export interface NetworkInfo {
  addresses: NetworkAddress[];
  zeroTier: ZeroTierInfo | null;
}

/** Decoded contents of a `PC1-…` join code. */
export interface NativeJoinCode {
  address: string;
  port: number;
  code: string;
}

export interface NativeClientStopped {
  reason: string;
  failed: boolean;
  frames: number;
  uptimeSecs: number;
}

export interface NativeGamepadState {
  /** Controller slot on the client, used to keep multiple pads apart. */
  slot: number;
  name: string;
  /** `false` releases the virtual device for this slot. */
  connected: boolean;
  /** Sticks in -1..1, standard W3C Gamepad mapping. */
  axes: number[];
  /** Buttons/triggers in 0..1, standard W3C Gamepad mapping. */
  buttons: number[];
}

export type NativeRemoteInput =
  | { type: "pointer"; x: number; y: number }
  | { type: "button"; button: NativeMouseButton; pressed: boolean }
  | { type: "scroll"; deltaX: number; deltaY: number }
  | { type: "text"; text: string }
  | { type: "key"; keyCode: number; pressed: boolean; flags: Array<"shift" | "control" | "option" | "command"> }
  | { type: "gamepad"; state: NativeGamepadState };
