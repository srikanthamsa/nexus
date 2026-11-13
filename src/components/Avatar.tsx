import { cn, initials } from "../lib/utils";

const palette: Record<string, [string, string]> = {};

function cascadeColors(seed: string): [string, string] {
  if (palette[seed]) return palette[seed];
  const colors: Array<[string, string]> = [
    ["#7c6cff", "#c084fc"],
    ["#2dd4bf", "#38bdf8"],
    ["#fb923c", "#f472b6"],
    ["#4ade80", "#22d3ee"],
    ["#f472b6", "#a78bfa"],
    ["#facc15", "#fb7185"],
    ["#60a5fa", "#818cf8"],
  ];
  let hash = 0;
  for (let index = 0; index < seed.length; index += 1) {
    hash = (hash * 31 + seed.charCodeAt(index)) >>> 0;
  }
  const result = colors[hash % colors.length] as [string, string];
  palette[seed] = result;
  return result;
}

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
  const [from, to] = cascadeColors(name);
  return (
    <span
      className={cn("avatar", className)}
      style={{
        width: size,
        height: size,
        fontSize: Math.max(9, size * 0.34),
        background: `linear-gradient(135deg, ${from}, ${to})`,
      }}
      aria-hidden="true"
    >
      {initials(name)}
      {presence && <span className={cn("avatar__presence", `is-${presence}`)} />}
    </span>
  );
}
