import nexusMark from "../assets/nexus-mark.png";
import { cn } from "../lib/utils";

export function LogoMark({ size = 24, className }: { size?: number; className?: string }) {
  return (
    <img
      className={cn("logo-mark", className)}
      src={nexusMark}
      width={size}
      height={size}
      alt=""
      aria-hidden="true"
      draggable={false}
    />
  );
}

export function Logo({
  size = 24,
  showWordmark = true,
  className,
}: {
  size?: number;
  showWordmark?: boolean;
  className?: string;
}) {
  return (
    <span className={cn("logo", className)}>
      <LogoMark size={size} />
      {showWordmark && <span className="logo__word">Nexus</span>}
    </span>
  );
}
