import { cn } from "../lib/utils";

export function LogoMark({ size = 24, className }: { size?: number; className?: string }) {
  return (
    <svg
      className={cn("logo-mark", className)}
      width={size}
      height={size}
      viewBox="0 0 32 32"
      fill="none"
      aria-hidden="true"
    >
      <rect width="32" height="32" rx="9.5" fill="var(--accent)" />
      <path
        d="M10.5 10.25 17.25 16l-6.75 5.75"
        stroke="white"
        strokeWidth="2.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M16.25 10.25 23 16l-6.75 5.75"
        stroke="white"
        strokeOpacity="0.52"
        strokeWidth="2.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
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
      {showWordmark && (
        <span className="logo__word">
          Parsec<span className="logo__plus">Plus</span>
        </span>
      )}
    </span>
  );
}
