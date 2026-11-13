import { useEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { Check } from "lucide-react";
import { cn } from "../lib/utils";

export function ConnectionArrival({
  eyebrow,
  title,
  meta,
  theme,
  onDone,
  duration = 1900,
}: {
  eyebrow: string;
  title: string;
  meta?: string;
  theme: string;
  onDone: () => void;
  duration?: number;
}) {
  const doneRef = useRef(onDone);
  doneRef.current = onDone;

  useEffect(() => {
    const timer = window.setTimeout(() => doneRef.current(), duration);
    return () => window.clearTimeout(timer);
  }, [duration]);

  return createPortal(
    <div
      className={cn("connection-arrival", `theme-${theme}`)}
      role="status"
      aria-live="polite"
    >
      <span className="connection-arrival__veil" aria-hidden="true" />
      <span className="connection-arrival__ring" aria-hidden="true" />
      <span className="connection-arrival__ring connection-arrival__ring--b" aria-hidden="true" />
      <span className="connection-arrival__ring connection-arrival__ring--c" aria-hidden="true" />
      <div className="connection-arrival__lockup">
        <span className="connection-arrival__mark">
          <Check size={17} strokeWidth={2.4} />
        </span>
        <p className="connection-arrival__eyebrow">{eyebrow}</p>
        <h2>{title}</h2>
        {meta && <p className="connection-arrival__meta">{meta}</p>}
      </div>
    </div>,
    document.body,
  );
}
