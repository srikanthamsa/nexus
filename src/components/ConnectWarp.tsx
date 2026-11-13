import { createPortal } from "react-dom";
import { WarpField } from "./WarpField";

/**
 * The recovered, settled warp transition: the original 420-star field with the
 * slow-then-fast 2.4 power curve. It is mounted only after the encrypted
 * session is established.
 */
export function ConnectWarp({
  channel,
  reduceMotion,
}: {
  channel: "direct" | "local" | "relay";
  reduceMotion?: boolean;
}) {
  const label =
    channel === "relay"
      ? "Entering relay session"
      : channel === "local"
        ? "Entering local session"
        : "Entering direct session";

  return createPortal(
    <div className="connect-warp theme-violet" aria-hidden="true">
      {!reduceMotion && <WarpField />}
      <span className="connect-warp__label">{label}</span>
    </div>,
    document.body,
  );
}
