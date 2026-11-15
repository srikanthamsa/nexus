import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Warning as AlertTriangle, Check, Clock as Clock3, Devices as MonitorSmartphone, ShieldCheck, WifiHigh as Wifi } from "@phosphor-icons/react";
import {
  getNativeHostStatus,
  isTauri,
  respondToHostRequest,
  subscribeNativeHost,
} from "../lib/native";
import { useApp } from "../lib/store";
import type { NativeHostStatus } from "../lib/types";
import { cn } from "../lib/utils";
import { Button } from "./Button";
import { ConnectionArrival } from "./ConnectionArrival";
import { useToast } from "./Toast";

export interface HostApprovalRequest {
  peerId: string;
  peer: string;
  device: string;
  owner: string;
  expiresInMs: number;
}

type GatePhase = "request" | "securing" | "connected" | "expired" | "failed";

interface ArrivalNotice {
  eyebrow: string;
  title: string;
  meta: string;
  theme: string;
}

const transferSteps = [
  { label: "Approval granted" },
  { label: "Encrypted channel verified" },
  { label: "Video stream started" },
];

export function HostConnectionLayer() {
  const { data, updateHost } = useApp();
  const { push } = useToast();
  const native = isTauri();
  const [nativeRequest, setNativeRequest] = useState<HostApprovalRequest | null>(null);
  const [snapshot, setSnapshot] = useState<HostApprovalRequest | null>(null);
  const [sourceKey, setSourceKey] = useState<string | null>(null);
  const [phase, setPhase] = useState<GatePhase>("request");
  const [failure, setFailure] = useState<string | null>(null);
  const [closed, setClosed] = useState(true);
  const [startedAt, setStartedAt] = useState(0);
  const [now, setNow] = useState(() => Date.now());
  const [arrival, setArrival] = useState<ArrivalNotice | null>(null);
  const [systemReducedMotion] = useState(
    () => window.matchMedia("(prefers-reduced-motion: reduce)").matches,
  );
  const reduceMotion = data.settings.reduceMotion || systemReducedMotion;
  const reduceMotionRef = useRef(reduceMotion);
  reduceMotionRef.current = reduceMotion;
  const acceptedRef = useRef(false);
  const streamingRef = useRef<boolean | null>(null);
  const updateHostRef = useRef(updateHost);
  updateHostRef.current = updateHost;
  const pushRef = useRef(push);
  pushRef.current = push;

  const request = nativeRequest;
  const requestSourceKey = request
    ? `${request.peerId}|${request.device}`
    : null;

  useEffect(() => {
    if (!request || !requestSourceKey) return;
    if (requestSourceKey === sourceKey) return;
    setSourceKey(requestSourceKey);
    setSnapshot(request);
    setPhase("request");
    setFailure(null);
    setClosed(false);
    setStartedAt(Date.now());
    setNow(Date.now());
    acceptedRef.current = false;
  }, [request, requestSourceKey, sourceKey]);

  useEffect(() => {
    if (!snapshot || phase !== "request" || closed) return;
    if (request) return;
    setPhase("expired");
  }, [snapshot, phase, closed, request]);

  useEffect(() => {
    if (!snapshot || phase !== "request" || closed) return;
    const interval = window.setInterval(() => setNow(Date.now()), 100);
    return () => window.clearInterval(interval);
  }, [snapshot, phase, closed]);

  const remaining = snapshot ? Math.max(0, snapshot.expiresInMs - (now - startedAt)) : 0;
  const seconds = Math.max(0, Math.ceil(remaining / 1000));
  const progress = snapshot
    ? Math.max(0, Math.min(100, (remaining / Math.max(1, snapshot.expiresInMs)) * 100))
    : 0;

  useEffect(() => {
    if (!snapshot || phase !== "request" || closed) return;
    if (remaining > 0) return;
    setPhase("expired");
  }, [remaining, snapshot, phase, closed]);

  const closeRef = useRef<() => void>(() => {});
  closeRef.current = () => {
    setClosed(true);
    setSourceKey(null);
  };

  useEffect(() => {
    if (phase !== "connected") return;
    const timer = window.setTimeout(
      () => closeRef.current(),
      reduceMotion ? 1300 : 2900,
    );
    return () => window.clearTimeout(timer);
  }, [phase, reduceMotion]);

  useEffect(() => {
    if (phase !== "expired") return;
    const timer = window.setTimeout(() => closeRef.current(), 1600);
    return () => window.clearTimeout(timer);
  }, [phase]);

  useEffect(() => {
    if (phase !== "securing") return;
    const timer = window.setTimeout(() => {
      if (!acceptedRef.current) return;
      acceptedRef.current = false;
      setFailure(
        "The device did not start streaming. Check Screen Recording permission and try again.",
      );
      setPhase("failed");
    }, 12_000);
    return () => window.clearTimeout(timer);
  }, [phase]);

  useEffect(() => {
    if (!native) return;
    let cancelled = false;
    let unsubscribe: (() => void) | undefined;

    const apply = (status: NativeHostStatus) => {
      if (cancelled) return;
      updateHostRef.current({
        hosting: status.running,
        requireApproval: status.requireApproval,
      });

      const streaming = status.streaming || status.viewers.length > 0;
      const wasStreaming = streamingRef.current;
      streamingRef.current = streaming;
      if (wasStreaming === false && streaming) {
        if (acceptedRef.current) {
          acceptedRef.current = false;
          setPhase("connected");
        } else {
          const viewer = status.viewers[0];
          const title = viewer?.device || "A device connected";
          if (reduceMotionRef.current) {
            pushRef.current({
              title: "Device connected",
              description: `${title} joined over an encrypted direct path.`,
              tone: "success",
            });
          } else {
            setArrival({
              eyebrow: "Direct session started",
              title,
              meta: `${viewer?.peer ?? "Remote device"} · ${
                status.allowInput ? "Input allowed" : "View only"
              }`,
              theme: "nexus",
            });
          }
        }
      }

      const pendingRequest = status.pending[0];
      setNativeRequest(
        pendingRequest
          ? {
              peerId: pendingRequest.peerId,
              peer: pendingRequest.peer,
              device: pendingRequest.device,
              owner: pendingRequest.owner,
              expiresInMs: pendingRequest.expiresInMs,
            }
          : null,
      );
    };

    void subscribeNativeHost({ onStatus: apply }).then((stop) => {
      if (cancelled) stop();
      else unsubscribe = stop;
    });
    void getNativeHostStatus().then((status) => {
      if (!cancelled && status) apply(status);
    });

    return () => {
      cancelled = true;
      unsubscribe?.();
    };
  }, [native]);

  const accept = async () => {
    if (!snapshot || phase !== "request" || closed) return;
    setPhase("securing");
    acceptedRef.current = true;
    const result = await respondToHostRequest(snapshot.peerId, true);
    if (result.error) {
      acceptedRef.current = false;
      setFailure(result.error);
      setPhase("failed");
    }
  };

  const deny = async () => {
    if (!snapshot || phase !== "request") return;
    const declined = snapshot;
    setClosed(true);
    setSourceKey(null);
    acceptedRef.current = false;
    const result = await respondToHostRequest(declined.peerId, false);
    if (result.error) {
      pushRef.current({
        title: "Could not decline the request",
        description: result.error,
        tone: "error",
      });
      return;
    }
    pushRef.current({
      title: "Connection declined",
      description: `${declined.owner || declined.device} was not admitted. The invite code was rotated.`,
      tone: "default",
    });
  };

  const denyRef = useRef(deny);
  denyRef.current = deny;

  useEffect(() => {
    if (!snapshot || phase !== "request" || closed) return;
    const handler = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        void denyRef.current();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [snapshot, phase, closed]);

  const visible =
    Boolean(snapshot) && !closed && (phase !== "request" || Boolean(request));
  const connected = phase === "connected";
  const title = snapshot?.owner || snapshot?.device || "A Nexus device";

  const cardRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!visible) return;
    const previouslyFocused = document.activeElement as HTMLElement | null;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Tab" || !cardRef.current) return;
      const focusable = Array.from(
        cardRef.current.querySelectorAll<HTMLElement>(
          'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])',
        ),
      ).filter((element) => element.offsetParent !== null);
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      previouslyFocused?.focus?.();
    };
  }, [visible]);

  return (
    <>
      {visible &&
        snapshot &&
        createPortal(
          <div className={cn("host-gate", `is-${phase}`)}>
            <div className="host-gate__veil" aria-hidden="true" />
            <section
              ref={cardRef}
              className="host-gate__card"
              role="dialog"
              aria-modal="true"
              aria-labelledby="host-gate-title"
            >
              {phase === "request" ? (
                <>
                  <div className="host-gate__head">
                    <p className="host-gate__eyebrow">
                      <span className="pulse-dot" />
                      Incoming connection
                    </p>
                    <span className="host-gate__timer">{seconds}s</span>
                  </div>

                  <div className="host-gate__identity">
                    <span className="host-gate__device-icon">
                      <MonitorSmartphone size={22} />
                    </span>
                    <div>
                      <h2 id="host-gate-title">{title}</h2>
                      <p>
                        <span>{snapshot.device}</span>
                        <span className="dot-separator">·</span>
                        <Wifi size={11} />
                        <span>{snapshot.peer || "Direct path"}</span>
                      </p>
                    </div>
                  </div>

                  <p className="host-gate__desc">
                    This device verified your invite code and is asking to
                    view this Mac.
                  </p>

                  <div className="host-gate__security">
                    <span>
                      <ShieldCheck size={13} />
                      Code verified
                    </span>
                    <span className="dot-separator">·</span>
                    <span>Noise XX · ChaCha20-Poly1305</span>
                  </div>

                  <div
                    className="host-gate__countdown"
                    role="progressbar"
                    aria-label="Time left to respond"
                    aria-valuemin={0}
                    aria-valuemax={snapshot.expiresInMs}
                    aria-valuenow={remaining}
                  >
                    <span style={{ width: `${progress}%` }} />
                  </div>

                  <div className="host-gate__actions">
                    <Button variant="ghost" autoFocus onClick={() => void deny()}>
                      Decline
                    </Button>
                    <Button variant="primary" onClick={() => void accept()}>
                      Accept connection
                    </Button>
                  </div>

                  <p className="host-gate__note">
                    Only this device can use the code · expires in {seconds}s
                  </p>
                </>
              ) : phase === "expired" ? (
                <div className="host-gate__state">
                  <span className="host-gate__state-icon">
                    <Clock3 size={20} />
                  </span>
                  <p className="host-gate__eyebrow">Request expired</p>
                  <h2 id="host-gate-title">No response was sent</h2>
                  <p className="host-gate__desc">
                    The invite code was rotated. Start a new session when the device
                    is ready.
                  </p>
                </div>
              ) : phase === "failed" ? (
                <div className="host-gate__state">
                  <span className="host-gate__state-icon is-danger">
                    <AlertTriangle size={20} />
                  </span>
                  <p className="host-gate__eyebrow">Connection interrupted</p>
                  <h2 id="host-gate-title">The session did not start</h2>
                  <p className="host-gate__desc">{failure}</p>
                  <Button
                    variant="secondary"
                    size="sm"
                    onClick={() => closeRef.current()}
                  >
                    Close
                  </Button>
                </div>
              ) : (
                <div className="host-gate__transfer">
                  <span className="host-gate__mark">
                    <span className="host-gate__mark-ring" aria-hidden="true" />
                    <span
                      className="host-gate__mark-ring host-gate__mark-ring--b"
                      aria-hidden="true"
                    />
                    <span className="host-gate__mark-icon">
                      {connected ? (
                        <Check size={22} weight="bold" />
                      ) : (
                        <MonitorSmartphone size={22} />
                      )}
                    </span>
                  </span>
                  <p className="host-gate__eyebrow">
                    {connected ? "Session secured" : "Request approved"}
                  </p>
                  <h2 id="host-gate-title">
                    {connected ? "You’re connected" : "Securing the session"}
                  </h2>
                  <p className="host-gate__desc">
                    {connected
                      ? `${title} can now view this Mac over the encrypted direct path.`
                      : `Verifying the channel with ${title} and starting the encrypted stream.`}
                  </p>
                  <ol className="host-gate__steps">
                    {transferSteps.map((step, index) => {
                      const state = connected
                        ? "done"
                        : index === 0
                          ? "done"
                          : index === 1
                            ? "active"
                            : "pending";
                      return (
                        <li key={step.label} className={cn("host-gate__step", `is-${state}`)}>
                          <span className="host-gate__step-marker">
                            {state === "done" ? (
                              <Check size={11} />
                            ) : state === "active" ? (
                              <span className="spinner" />
                            ) : (
                              index + 1
                            )}
                          </span>
                          {step.label}
                        </li>
                      );
                    })}
                  </ol>
                  {connected && (
                    <p className="host-gate__note">
                      View only · Enable input any time from Host settings.
                    </p>
                  )}
                </div>
              )}
            </section>
          </div>,
          document.body,
        )}

      {arrival && <ConnectionArrival {...arrival} onDone={() => setArrival(null)} />}
    </>
  );
}
