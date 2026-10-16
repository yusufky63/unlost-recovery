// Locale-aware formatting. Sizes use binary units with the familiar "GB" labels, the way
// Windows Explorer shows them, so numbers match what the user sees elsewhere.

const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"];

export function formatBytes(bytes: number, locale = "en", digits?: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const exp = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), UNITS.length - 1);
  const value = bytes / 1024 ** exp;
  const d = digits ?? (exp === 0 ? 0 : value >= 100 ? 0 : 1);
  const n = new Intl.NumberFormat(locale, { minimumFractionDigits: d, maximumFractionDigits: d }).format(value);
  return `${n} ${UNITS[exp]}`;
}

export function formatCount(n: number, locale = "en"): string {
  return new Intl.NumberFormat(locale).format(n);
}

/** Unix seconds to a short date and time; empty for 0 (unknown). */
export function formatDateTime(unixSeconds: number, locale = "en"): string {
  if (!unixSeconds) return "";
  return new Intl.DateTimeFormat(locale, { year: "numeric", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }).format(new Date(unixSeconds * 1000));
}

export function formatDate(unixSeconds: number, locale = "en"): string {
  if (!unixSeconds) return "";
  return new Intl.DateTimeFormat(locale, { year: "numeric", month: "short", day: "numeric" }).format(new Date(unixSeconds * 1000));
}

/** Last path segment, for compact labels. */
export function baseName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const i = Math.max(trimmed.lastIndexOf("\\"), trimmed.lastIndexOf("/"));
  return i >= 0 ? trimmed.slice(i + 1) : trimmed;
}

export function parentPath(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const i = Math.max(trimmed.lastIndexOf("\\"), trimmed.lastIndexOf("/"));
  return i > 0 ? trimmed.slice(0, i) : trimmed;
}

/** `C:\` -> `C:` */
export function driveName(root: string): string {
  return root.replace(/\\+$/, "");
}

/** Joins a folder path inside a volume (`\a\b`) with a name. */
export function joinInner(folder: string, name: string): string {
  return folder === "\\" || folder === "" ? `\\${name}` : `${folder}\\${name}`;
}
