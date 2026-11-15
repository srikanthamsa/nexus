import { cn, initials } from "../lib/utils";

export function Avatar({
  name,
  size = 34,
  presence,
  className,
}: {
  name: string;
  size?: number;
  presence?: "online" | "in-session" | "away" | "offline";
  className?: string;
}) {
  return (
    <span
      className={cn("avatar", className)}
      style={{
        width: size,
        height: size,
        fontSize: Math.max(9, size * 0.34),
      }}
      aria-hidden="true"
    >
      {initials(name)}
      {presence && <span className={cn("avatar__presence", `is-${presence}`)} />}
    </span>
  );
}
