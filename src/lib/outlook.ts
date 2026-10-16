// Per-drive outlook: one chance level plus the facts behind it, as translation keys. Pure, so
// the rules are unit tested.

import type { Drive, ShadowCopy } from "./types";

export type Chance = "good" | "fair" | "low" | "unknown";

export type NoteKey = "hdd" | "trimOn" | "trimUnknown" | "removable" | "system" | "appHere" | "locked" | "refs" | "fat" | "virtual" | "binHas";

export interface Note {
  key: NoteKey;
  tone: "positive" | "warning" | "danger" | "neutral";
  params?: Record<string, string | number>;
}

export interface Outlook {
  chance: Chance;
  notes: Note[];
  /** Deleted-file scans work on this drive's file system. */
  scannable: boolean;
}

const RANK: Chance[] = ["unknown", "low", "fair", "good"];

function lower(a: Chance, b: Chance): Chance {
  return RANK.indexOf(a) < RANK.indexOf(b) ? a : b;
}

export function isNtfs(fs: string): boolean {
  return fs.toUpperCase() === "NTFS";
}

export function outlook(d: Drive, appDisks: number[]): Outlook {
  const notes: Note[] = [];
  if (!d.readable) {
    return { chance: "unknown", notes: [{ key: "locked", tone: "danger" }], scannable: false };
  }
  let chance: Chance = "fair";
  const fs = d.fileSystem.toUpperCase();
  if (d.media === "hdd") {
    chance = "good";
    notes.push({ key: "hdd", tone: "positive" });
  } else if (d.media === "usbStick" || d.media === "sdCard") {
    chance = "good";
    notes.push({ key: "removable", tone: "positive" });
  } else if (d.media === "virtual") {
    notes.push({ key: "virtual", tone: "neutral" });
  } else if (d.trim === "on") {
    chance = "low";
    notes.push({ key: "trimOn", tone: "danger" });
  } else if (d.media === "ssd" || d.media === "externalDisk") {
    if (d.trim === "off") chance = "good";
    else notes.push({ key: "trimUnknown", tone: "warning" });
  }
  if (fs === "REFS") {
    chance = lower(chance, "low");
    notes.push({ key: "refs", tone: "warning" });
  } else if (fs.startsWith("FAT") || fs === "EXFAT") {
    notes.push({ key: "fat", tone: "warning" });
  }
  if (d.system) {
    chance = lower(chance, "fair");
    notes.push({ key: "system", tone: "warning" });
  }
  if (d.disks.some((n) => appDisks.includes(n))) {
    notes.push({ key: "appHere", tone: "warning" });
  }
  if (d.binItems > 0) {
    notes.unshift({ key: "binHas", tone: "positive", params: { count: d.binItems } });
  }
  return { chance, notes, scannable: isNtfs(d.fileSystem) };
}

/** Shadow copies per drive root, `null` when they could not be listed. */
export function shadowCounts(copies: ShadowCopy[] | null): Record<string, number> | null {
  if (!copies) return null;
  const out: Record<string, number> = {};
  for (const c of copies) for (const r of c.roots) out[r.toUpperCase()] = (out[r.toUpperCase()] ?? 0) + 1;
  return out;
}

export function chanceTone(c: Chance): "positive" | "warning" | "danger" | "neutral" {
  return c === "good" ? "positive" : c === "fair" ? "warning" : c === "low" ? "danger" : "neutral";
}
