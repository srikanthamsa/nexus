import { useEffect, useState, type ReactNode } from "react";
import { DirectConnectModal } from "./components/DirectConnectModal";
import { HostConnectionLayer } from "./components/HostConnectionLayer";
import { PresenceDock } from "./components/PresenceDock";
import { Rail } from "./components/Rail";
import { SearchPalette } from "./components/SearchPalette";
import { Titlebar } from "./components/Titlebar";
import { Topbar } from "./components/Topbar";
import { ToastProvider, useToast } from "./components/Toast";
import {
  disconnectNativeClient,
  isTauri,
  setAlwaysOnTop,
  stopNativeCapture,
  stopNativeHost,
} from "./lib/native";
import { AppProvider, useApp } from "./lib/store";
import type {
  NativeCaptureStopped,
  NativeCaptureSummary,
  NativeClientStopped,
  NativeClientSummary,
  RecentConnection,
  Screen,
} from "./lib/types";
import { cn } from "./lib/utils";
import { ArcadeScreen } from "./screens/Arcade";
import { ComputersScreen } from "./screens/Computers";
import { FriendsScreen } from "./screens/Friends";
import { HostScreen } from "./screens/Host";
import { SessionScreen } from "./screens/Session";
import { SettingsScreen } from "./screens/Settings";
import { SignInScreen } from "./screens/SignIn";
import { TeamsScreen } from "./screens/Teams";

interface RemoteSession {
  summary: NativeClientSummary;
  address: string;
}

/** Splits a `host:port` endpoint so a real recent connection can be stored. */
function splitEndpoint(value: string): { address: string; port: number } {
  const trimmed = value.trim();
  const match = trimmed.match(/^(.*):(\d{2,5})$/);
  if (match) return { address: match[1], port: Number(match[2]) };
  return { address: trimmed, port: 39200 };
}

function AppRoot() {
  const { data, updateHost, addRecent } = useApp();
  const { push } = useToast();
  const [screen, setScreen] = useState<Screen>("computers");
  const [searchOpen, setSearchOpen] = useState(false);
  const [live, setLive] = useState<NativeCaptureSummary | null>(null);
  const [remote, setRemote] = useState<RemoteSession | null>(null);
  const [directOpen, setDirectOpen] = useState(false);
  const [directAddress, setDirectAddress] = useState("");
  const [directCode, setDirectCode] = useState("");

  useEffect(() => {
    void stopNativeCapture();
    void stopNativeHost().then(() => {
      if (isTauri()) updateHost({ hosting: false });
    });
  }, []);

  useEffect(() => {
    document.documentElement.dataset.accent = data.settings.accent;
  }, [data.settings.accent]);

  useEffect(() => {
    void setAlwaysOnTop(data.settings.alwaysOnTop);
  }, [data.settings.alwaysOnTop]);

  useEffect(() => {
    if (!data.user || live || remote) return;
    const handler = (event: KeyboardEvent) => {
      const meta = event.metaKey || event.ctrlKey;
      if (meta && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setSearchOpen(true);
      }
      if (meta && event.key === ",") {
        event.preventDefault();
        setScreen("settings");
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [data.user, live, remote]);

  const openDirect = (address = "", code = "") => {
    setDirectAddress(address);
    setDirectCode(code);
    setDirectOpen(true);
  };

  const directConnected = (
    summary: NativeClientSummary,
    address: string,
    code: string,
  ) => {
    const relay = summary.path === "relay";
    const endpoint = splitEndpoint(address);
    const recent: Omit<RecentConnection, "id" | "lastConnectedAt"> = {
      name: summary.hostName || endpoint.address || "Direct session",
      address: endpoint.address,
      port: endpoint.port,
      transport: relay ? "relay" : "direct",
      code,
      width: summary.width,
      height: summary.height,
    };
    addRecent(recent);
    setRemote({ summary, address });
  };

  const disconnect = () => {
    if (remote) {
      const relay = remote.summary.path === "relay";
      void disconnectNativeClient();
      setRemote(null);
        push({
        title: relay ? "Relay session ended" : "Direct session ended",
        description: "The encrypted channel is closed.",
        tone: "default",
      });
      return;
    }
    if (live) {
      void stopNativeCapture();
      setLive(null);
      push({
        title: "Live preview ended",
        description: "Screen capture and hardware encoding stopped.",
        tone: "default",
      });
      return;
    }
  };

  const startLivePreview = (summary: NativeCaptureSummary) => {
    setLive(summary);
    void stopNativeHost();
    updateHost({ hosting: false });
    push({
      title: "Live preview started",
      description: `${summary.width} × ${summary.height} · ${summary.codec} hardware encoding`,
      tone: "success",
    });
  };

  const liveStopped = (event: NativeCaptureStopped) => {
    setLive(null);
    push({
      title: event.failed ? "Live capture stopped" : "Live preview ended",
      description: event.failed
        ? event.reason
        : `Captured ${event.frames.toLocaleString()} frames before the engine stopped.`,
      tone: event.failed ? "error" : "default",
    });
  };

  const remoteStopped = (event: NativeClientStopped) => {
    const relay = remote?.summary.path === "relay";
    setRemote(null);
    push({
      title: event.failed
        ? relay
          ? "Relay session interrupted"
          : "Direct session interrupted"
        : relay
          ? "Relay session ended"
          : "Direct session ended",
      description: event.failed
        ? event.reason
        : `Received ${event.frames.toLocaleString()} frames before the host disconnected.`,
      tone: event.failed ? "error" : "default",
    });
  };

  const renderScreen = () => {
    switch (screen) {
      case "friends":
        return <FriendsScreen onDirectConnect={() => openDirect()} />;
      case "host":
        return <HostScreen onStartPreview={startLivePreview} />;
      case "teams":
        return <TeamsScreen />;
      case "arcade":
        return <ArcadeScreen />;
      case "settings":
        return <SettingsScreen />;
      default:
        return (
          <ComputersScreen
            onDirectConnect={openDirect}
            onNavigate={setScreen}
          />
        );
    }
  };

  const shellClasses = cn(
    "app",
    data.settings.compact && "is-compact",
    data.settings.reduceMotion && "reduce-motion",
    !data.user && "app--auth",
    (live || remote) && "app--session",
  );

  let content: ReactNode;

  if (!data.user) {
    content = (
      <div className={shellClasses}>
        <Titlebar />
        <SignInScreen />
      </div>
    );
  } else if (remote) {
    const { summary, address } = remote;
    content = (
      <div className={shellClasses}>
        <Titlebar title={`${summary.hostName || address} · Direct session`} />
        <SessionScreen
          remote={summary}
          remoteLabel={address}
          onDisconnect={disconnect}
          onRemoteStopped={remoteStopped}
        />
      </div>
    );
  } else if (live) {
    content = (
      <div className={shellClasses}>
        <Titlebar title="This Mac · Live preview" />
        <SessionScreen
          live={live}
          onDisconnect={disconnect}
          onLiveStopped={liveStopped}
        />
      </div>
    );
  } else {
    content = (
      <div className={shellClasses}>
        <Titlebar />
        <div className="app__body">
          <Rail
            screen={screen}
            onNavigate={setScreen}
            hosting={data.host.hosting}
            user={data.user}
          />
          <main className="workspace">
            <Topbar
              onSearch={() => setSearchOpen(true)}
              onNavigate={setScreen}
              hosting={data.host.hosting}
            />
            <div className="workspace__scroll">
              <div className="screen-stage" key={screen}>
                {renderScreen()}
              </div>
            </div>
          </main>
        </div>

        <SearchPalette
          open={searchOpen}
          onClose={() => setSearchOpen(false)}
          onNavigate={setScreen}
          onDirectConnect={openDirect}
        />

        <DirectConnectModal
          open={directOpen}
          onClose={() => setDirectOpen(false)}
          onConnected={directConnected}
          initialAddress={directAddress}
          initialCode={directCode}
          autoConnect={Boolean(directCode)}
        />
      </div>
    );
  }

  return (
    <>
      {content}
      {data.user && (
        <>
          <HostConnectionLayer />
          <PresenceDock compact={Boolean(live || remote)} />
        </>
      )}
    </>
  );
}

export default function App() {
  return (
    <ToastProvider>
      <AppProvider>
        <AppRoot />
      </AppProvider>
    </ToastProvider>
  );
}
