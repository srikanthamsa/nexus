export function cn(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(" ");
}

export function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return "?";
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return `${parts[0][0]}${parts[parts.length - 1][0]}`.toUpperCase();
}

export function formatDuration(seconds: number): string {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = Math.floor(seconds % 60);
  if (hours > 0) {
    return `${hours}:${String(minutes).padStart(2, "0")}:${String(secs).padStart(2, "0")}`;
  }
  return `${minutes}:${String(secs).padStart(2, "0")}`;
}

export function normalizePairCode(value: string): string {
  const clean = value
    .toUpperCase()
    .replace(/[^A-Z0-9]/g, "")
    .slice(0, 8);
  return clean.length > 4 ? `${clean.slice(0, 4)}-${clean.slice(4)}` : clean;
}

export function isValidPairCode(value: string): boolean {
  return /^[A-Z0-9]{4}-[A-Z0-9]{4}$/.test(normalizePairCode(value));
}

export function isValidRelayInvite(value: string): boolean {
  const trimmed = value.trim();
  const hash = trimmed.lastIndexOf("#");
  if (hash <= 0) return false;
  const address = trimmed.slice(0, hash).trim();
  const room = trimmed.slice(hash + 1).trim();
  if (!address || address.includes("#")) return false;
  return /^[0-9a-f]{32}$/i.test(room);
}

export function hostEndpoint(address: string, port: number | null | undefined): string {
  if (!port) return address;
  const host = address.includes(":") && !address.startsWith("[") ? `[${address}]` : address;
  return `${host}:${port}`;
}

export function copyToClipboard(text: string): Promise<boolean> {
  if (navigator.clipboard?.writeText) {
    return navigator.clipboard.writeText(text).then(
      () => true,
      () => false,
    );
  }
  try {
    const area = document.createElement("textarea");
    area.value = text;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(area);
    return Promise.resolve(ok);
  } catch {
    return Promise.resolve(false);
  }
}

/**
 * Turns an OS hostname into a short, human device name:
 * `Srikants-MacBook-Air-4.local` → `Srikants-MacBook`, `Studio.local` → `Studio`.
 */
export function friendlyDeviceName(hostname: string | null | undefined): string {
  if (!hostname) return "This Mac";
  let name = hostname
    .trim()
    .replace(/\.local\.?$/i, "")
    .replace(/-\d+$/, "")
    .replace(/-(air|pro|max|studio)$/i, "")
    .replace(/[.\s]+$/, "");
  if (!name) name = "This Mac";
  return name;
}
