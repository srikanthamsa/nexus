import type { AppData } from "./types";

/** Defaults for a fresh install: no profile, no connections, no seeded data. */
export const seedData: AppData = {
  user: null,
  recent: [],
  host: {
    hosting: false,
    displayIndex: 0,
    resolution: "match",
    refresh: "60",
    codec: "auto",
    bitrate: 12,
    hdr: false,
    virtualDisplay: false,
    gamepad: true,
    clipboard: true,
    fileTransfer: false,
    hostAudio: true,
    requireApproval: true,
    lockCursor: false,
    adaptNetwork: true,
    relayFallback: true,
    relayAddress: "",
    mediaPort: 39201,
  },
  settings: {
    accent: "violet",
    reduceMotion: false,
    compact: false,
    alwaysOnTop: false,
    showStats: true,
    autoConnect: false,
    decoder: "auto",
    audioOutput: "MacBook Pro Speakers",
    volume: 70,
    muteWhenMinimized: true,
    networkMode: "auto",
    bandwidthLimit: false,
    bandwidthCap: 50,
    overlayHotkey: "⌘ ⇧ O",
    pushToTalk: false,
    iceStunUrls: "",
    iceTurnUrls: "",
    iceTurnUsername: "",
    iceTurnCredential: "",
  },
  permissions: {
    screenRecording: false,
    accessibility: false,
    inputMonitoring: false,
  },
};

export const accents = [
  { id: "violet", label: "Ion", colors: ["#8b7cff", "#c084fc"] },
  { id: "aurora", label: "Aurora", colors: ["#3ddc97", "#43c7ff"] },
  { id: "ember", label: "Ember", colors: ["#ff9f5a", "#ff6b7a"] },
  { id: "ocean", label: "Ocean", colors: ["#4ea8ff", "#6ee7f9"] },
] as const;

export const hotkeys = [
  { keys: ["⌘", "⇧", "O"], label: "Show session overlay" },
  { keys: ["Esc"], label: "Open session menu" },
  { keys: ["⌘", "⇧", "S"], label: "Toggle performance stats" },
  { keys: ["⌘", "Ctrl", "F"], label: "Toggle fullscreen" },
  { keys: ["⌘", "⇧", "D"], label: "Disconnect session" },
  { keys: ["⌘", "K"], label: "Search screens and hotkeys" },
];
