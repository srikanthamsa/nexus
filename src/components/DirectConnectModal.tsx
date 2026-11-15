import { useCallback, useEffect, useRef, useState } from "react";
import {
  Check,
  KeyRound,
  Link2,
  Monitor,
  Radar,
  RefreshCw,
  ShieldCheck,
} from "lucide-react";
import {
  buildIceSettings,
  connectNativeClient,
  discoverNativeHosts,
  isTauri,
  parseJoinCode,
} from "../lib/native";
import { useApp } from "../lib/store";
import type { NativeClientSummary, NativeDiscoveredHost } from "../lib/types";
import {
  cn,
  hostEndpoint,
  isValidPairCode,
  isValidRelayInvite,
  normalizePairCode,
} from "../lib/utils";
import { Button } from "./Button";
import { ConnectWarp } from "./ConnectWarp";
import { Modal } from "./Modal";
import { Segmented } from "./Segmented";
import { TextField } from "./TextField";

type ConnectMode = "join" | "nearby" | "relay";

export function DirectConnectModal({
  open,
  onClose,
  onConnected,
  initialAddress = "",
  initialCode = "",
  autoConnect = false,
}: {
  open: boolean;
  onClose: () => void;
  onConnected: (summary: NativeClientSummary, address: string, code: string) => void;
  /** Prefills the Nearby address field, e.g. from a recent connection. */
  initialAddress?: string;
  /** Prefills the saved invite code for a one-click reconnect. */
  initialCode?: string;
  /** Reconnect immediately once the saved address/code are pre-filled. */
  autoConnect?: boolean;
}) {
  const { data } = useApp();
  const [mode, setMode] = useState<ConnectMode>("join");
  const [joinInput, setJoinInput] = useState("");
  const [address, setAddress] = useState("");
  const [invite, setInvite] = useState("");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [waiting, setWaiting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hosts, setHosts] = useState<NativeDiscoveredHost[]>([]);
  const [selectedHost, setSelectedHost] = useState<string | null>(null);
  const [scanning, setScanning] = useState(false);
  const [scanError, setScanError] = useState<string | null>(null);
  const [warping, setWarping] = useState(false);
  const warpTimer = useRef<number | null>(null);
  const autoStarted = useRef(false);
  const scanId = useRef(0);
  const native = isTauri();

  useEffect(
    () => () => {
      if (warpTimer.current !== null) window.clearTimeout(warpTimer.current);
    },
    [],
  );

  const scan = useCallback(async () => {
    if (!native) return;
    const id = scanId.current + 1;
    scanId.current = id;
    setScanning(true);
    setScanError(null);
    const result = await discoverNativeHosts();
    if (id !== scanId.current) return;
    setHosts(result.hosts);
    setScanError(result.error ?? null);
    setScanning(false);
  }, [native]);

  useEffect(() => {
    if (!open) {
      scanId.current += 1;
      return;
    }
    scanId.current += 1;
    autoStarted.current = false;
    setMode(initialAddress || initialCode ? "nearby" : "join");
    setJoinInput("");
    setAddress(initialAddress);
    setInvite("");
    setCode(initialCode);
    setSelectedHost(null);
    setBusy(false);
    setWaiting(false);
    setError(null);
    setHosts([]);
    setScanning(false);
    setScanError(null);
    // Paint the modal shell first; discovery is deferred one frame so opening
    // the dialog never waits on the native command round trip.
    const frame = window.requestAnimationFrame(() => {
      void scan();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [open, scan, initialAddress, initialCode]);

  useEffect(() => {
    if (!busy) {
      setWaiting(false);
      return;
    }
    const timer = window.setTimeout(() => setWaiting(true), 6000);
    return () => window.clearTimeout(timer);
  }, [busy]);

  const pickHost = (host: NativeDiscoveredHost) => {
    const endpoint = hostEndpoint(host.address, host.port);
    setAddress(endpoint);
    setSelectedHost(endpoint);
    setError(null);
  };

  const joinInputTrimmed = joinInput.trim();
  const canSubmit =
    native &&
    !busy &&
    (mode === "join"
      ? joinInputTrimmed.startsWith("PC1-") && joinInputTrimmed.length > 12
      : isValidPairCode(code) &&
        (mode === "nearby" ? Boolean(address.trim()) : isValidRelayInvite(invite)));

  const finishConnection = (
    summary: NativeClientSummary,
    target: string,
    usedCode: string,
  ) => {
    setBusy(false);
    // The warp is a success transition: it only appears after the encrypted
    // session is already established, never while approval is still pending.
    setWarping(true);
    if (warpTimer.current !== null) window.clearTimeout(warpTimer.current);
    warpTimer.current = window.setTimeout(() => {
      setWarping(false);
      onConnected(summary, target, usedCode);
      onClose();
    }, data.settings.reduceMotion ? 140 : 860);
  };

  const submit = async () => {
    if (!canSubmit) return;
    setBusy(true);
    setError(null);
    if (mode === "join") {
      const parsed = await parseJoinCode(joinInputTrimmed);
      if (!parsed.ok) {
        setBusy(false);
        setError(parsed.error);
        return;
      }
      const result = await connectNativeClient({
        address: parsed.value.address,
        port: parsed.value.port,
        code: parsed.value.code,
        ownerName: data.user?.name,
        ice: buildIceSettings(data.settings),
      });
      if (!result.ok) {
        setBusy(false);
        setError(result.error);
        return;
      }
      finishConnection(result.summary, parsed.value.address, parsed.value.code);
      return;
    }
    const result = await connectNativeClient(
      mode === "nearby"
        ? {
            address: address.trim(),
            code,
            ownerName: data.user?.name,
            ice: buildIceSettings(data.settings),
          }
        : {
            address: "",
            relayInvite: invite.trim(),
            code,
            ownerName: data.user?.name,
            ice: buildIceSettings(data.settings),
          },
    );
    if (!result.ok) {
      setBusy(false);
      setError(result.error);
      return;
    }
    const fallback = mode === "nearby" ? address.trim() : invite.trim().split("#")[0];
    finishConnection(result.summary, fallback, code);
  };

  useEffect(() => {
    if (!open || !autoConnect || !initialAddress || !initialCode) return;
    if (autoStarted.current || !canSubmit) return;
    autoStarted.current = true;
    void submit();
    // submit is intentionally fired once for a saved reconnect.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, autoConnect, initialAddress, initialCode, canSubmit]);

  return (
    <>
      {warping && (
        <ConnectWarp
          channel={mode === "relay" ? "relay" : mode === "nearby" ? "local" : "direct"}
          reduceMotion={data.settings.reduceMotion}
        />
      )}
      <Modal
        open={open}
      onClose={() => {
        if (!busy) onClose();
      }}
      title="Connect directly"
      description="Enter the code from the host. They approve the connection before it starts."
      size="sm"
      footer={
        <>
          <span className="modal__note">
            {native ? "No account required." : "Preview mode."}
          </span>
          <div className="modal__footer-actions">
            <Button variant="ghost" onClick={onClose} disabled={busy}>
              Cancel
            </Button>
            <Button
              variant="primary"
              className="btn--light"
              leftIcon={<Link2 size={15} />}
              loading={busy}
              disabled={!canSubmit}
              onClick={() => void submit()}
            >
              Connect
            </Button>
          </div>
        </>
      }
    >
      <div className="direct-connect">
        <Segmented
          className="direct-connect__tabs"
          ariaLabel="Connection route"
          value={mode}
          onChange={(value) => {
            if (!busy) setMode(value);
          }}
          options={[
            { value: "join", label: "Connection code", icon: <KeyRound size={13} /> },
            { value: "nearby", label: "Address", icon: <Radar size={13} /> },
          ]}
        />

        <div className="direct-connect__pane">
          {mode === "join" ? (
            <>
              <TextField
                autoFocus
                label="Connection code"
                placeholder="PC1-…"
                value={joinInput}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                onChange={(event) => {
                  setJoinInput(event.target.value);
                  setError(null);
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && canSubmit) void submit();
                }}
                hint="Paste the code from the host. They approve the connection before it starts."
                error={error ?? undefined}
              />

              <div className="relay-note">
                <span className="relay-note__icon">
                  <ShieldCheck size={17} />
                </span>
                <div>
                  <p>Ready to connect</p>
                  <span>
                    The code carries the host's address, port, and invite code. Your
                    friend approves the connection before streaming starts.
                  </span>
                </div>
              </div>
            </>
          ) : mode === "nearby" ? (
            <>
              <TextField
                autoFocus
                label="Host address"
                placeholder="192.168.1.20:39200"
                value={address}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                onChange={(event) => {
                  setAddress(event.target.value);
                  setSelectedHost(null);
                  setError(null);
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && isValidPairCode(code)) void submit();
                }}
                hint="Enter the address from the host. It looks like 100.x.x.x or a local address."
              />

              <div className="nearby">
                <div className="nearby__head">
                  <span className="nearby__label">
                    <Radar size={13} />
                    Nearby computers
                  </span>
                  <Button
                    size="sm"
                    variant="ghost"
                    leftIcon={
                      <RefreshCw size={13} className={scanning ? "spin" : undefined} />
                    }
                    onClick={() => void scan()}
                    disabled={scanning || !native}
                  >
                    Refresh
                  </Button>
                </div>
                <div className="nearby__list">
                  {hosts.length > 0 ? (
                    hosts.map((host) => {
                      const endpoint = hostEndpoint(host.address, host.port);
                      const selected = endpoint === selectedHost;
                      return (
                        <button
                          key={`${host.name}-${endpoint}`}
                          type="button"
                          className={cn("nearby-host", selected && "is-active")}
                          onClick={() => pickHost(host)}
                        >
                          <span className="nearby-host__icon">
                            <Monitor size={15} />
                          </span>
                          <span className="nearby-host__copy">
                            <span className="nearby-host__name">{host.name}</span>
                            <span className="nearby-host__meta">
                              {endpoint} · {host.platform}
                            </span>
                          </span>
                          {selected ? (
                            <Check size={14} />
                          ) : (
                            <span className="nearby-host__use">Use</span>
                          )}
                        </button>
                      );
                    })
                  ) : (
                    <div className="nearby-empty">
                      {!native ? (
                        <>
                          <Radar size={16} />
                          <p>Nearby discovery needs the desktop app.</p>
                          <span>You can still enter a host address manually.</span>
                        </>
                      ) : scanning ? (
                        <>
                          <div className="nearby-skeleton" aria-hidden="true">
                            <span />
                            <span />
                            <span />
                          </div>
                          <p>Searching for nearby computers…</p>
                        </>
                      ) : scanError ? (
                        <>
                          <Radar size={16} />
                          <p>{scanError}</p>
                          <button
                            type="button"
                            className="link-button"
                            onClick={() => void scan()}
                          >
                            Try again
                          </button>
                        </>
                      ) : (
                        <>
                          <Radar size={16} />
                          <p>No nearby computers yet.</p>
                          <span>
                            Turn on hosting on another Mac, or enter its address above.
                          </span>
                        </>
                      )}
                    </div>
                  )}
                </div>
              </div>

              <TextField
                label="Invite code"
                className="pair-code-input"
                placeholder="XXXX-XXXX"
                value={code}
                spellCheck={false}
                autoCapitalize="characters"
                autoCorrect="off"
                onChange={(event) => {
                  setCode(normalizePairCode(event.target.value));
                  setError(null);
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && canSubmit) void submit();
                }}
                error={error ?? undefined}
              />
            </>
          ) : (
            <>
              <TextField
                autoFocus
                label="Relay invite"
                className="relay-invite-input"
                placeholder="relay.example.com:39210#room"
                value={invite}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                onChange={(event) => {
                  setInvite(event.target.value);
                  setError(null);
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && isValidPairCode(code)) void submit();
                }}
                hint="Paste the full invite from the host, including the 32-character room code."
              />

              <div className="relay-note">
                <span className="relay-note__icon">
                  <ShieldCheck size={17} />
                </span>
                <div>
                  <p>End-to-end through the relay</p>
                  <span>
                    The relay pairs both devices and forwards encrypted bytes. Your invite code
                    and screen never reach it.
                  </span>
                </div>
              </div>

              <TextField
                label="Invite code"
                className="pair-code-input"
                placeholder="XXXX-XXXX"
                value={code}
                spellCheck={false}
                autoCapitalize="characters"
                autoCorrect="off"
                onChange={(event) => {
                  setCode(normalizePairCode(event.target.value));
                  setError(null);
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && canSubmit) void submit();
                }}
                error={error ?? undefined}
              />
            </>
          )}

          <div className="code-entry__security">
            <ShieldCheck size={15} />
            <span>Private connection.</span>
          </div>

          {busy && waiting && (
            <p className="direct-connect__waiting" role="status">
              Waiting for the host to approve this device…
            </p>
          )}
        </div>

        {!native && (
          <p className="direct-connect__fallback">
            Connecting needs the desktop app.
          </p>
        )}
      </div>
      </Modal>
    </>
  );
}
