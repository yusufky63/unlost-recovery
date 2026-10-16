// Shapes returned by the Rust commands (serde camelCase). Keep in sync with unlost-core.

export type State = "intactCopy" | "verified" | "likelyIntact" | "partial" | "zeroed" | "overwritten" | "metadataOnly" | "notRecoverableHere";

export const STATES: State[] = ["intactCopy", "verified", "likelyIntact", "partial", "zeroed", "overwritten", "metadataOnly", "notRecoverableHere"];

export type Reason =
  | { code: "recycleBin" }
  | { code: "shadowCopy"; created: number }
  | { code: "residentIntact" }
  | { code: "emptyFile" }
  | { code: "clustersFree"; clusters: number; sampled: number }
  | { code: "clustersFreeUnsampled"; clusters: number }
  | { code: "clustersReused"; reused: number; total: number }
  | { code: "allZero"; sampled: number }
  | { code: "headerMismatch"; expected: string }
  | { code: "noRuns" }
  | { code: "tornRecord" }
  | { code: "runsOutsideVolume" }
  | { code: "sizeImplausible" }
  | { code: "extensionMissing" }
  | { code: "tooFragmented"; runs: number }
  | { code: "encrypted" }
  | { code: "compressed" }
  | { code: "wofCompressed" }
  | { code: "cloudOnly" }
  | { code: "unreadable"; bytes: number }
  | { code: "someZero"; zero: number; total: number };

export interface Assessment {
  state: State;
  reason: Reason;
}

export type Media = "hdd" | "ssd" | "usbStick" | "sdCard" | "externalDisk" | "virtual" | "unknown";
export type Trim = "on" | "off" | "unknown";

export interface Drive {
  root: string;
  label: string;
  fileSystem: string;
  total: number;
  free: number;
  volume: string;
  disks: number[];
  system: boolean;
  removable: boolean;
  readable: boolean;
  media: Media;
  trim: Trim;
  model: string;
  binItems: number;
  binBytes: number;
}

export interface AppInfo {
  version: string;
  elevated: boolean;
  portable: boolean;
  adminReady: boolean;
  dataDir: string;
  appDisks: number[];
}

export interface Settings {
  language: string;
  theme: string;
}

export interface BinItem {
  id: string;
  drive: string;
  originalPath: string;
  name: string;
  size: number;
  deleted: number;
  isDir: boolean;
  present: boolean;
  dataPath: string;
}

export interface ShadowCopy {
  id: string;
  device: string;
  volume: string;
  roots: string[];
  created: number;
}

export interface ShadowEntry {
  name: string;
  isDir: boolean;
  size: number;
  modified: number;
}

export type FsKind = "ntfs" | "fat" | "exFat" | "refs" | "bitLocker" | "unknown";

export interface Partition {
  index: number;
  offset: number;
  length: number;
  scheme: "whole" | "mbr" | "gpt";
  kind: string;
  name: string;
  fs: FsKind;
}

export type SourceSpec = { kind: "volume"; root: string } | { kind: "image"; path: string; partition: number | null } | { kind: "shadow"; device: string; volume: string };

export interface ScanStats {
  records: number;
  inUse: number;
  deletedRecords: number;
  deletedFolders: number;
  torn: number;
  sampled: number;
  truncated: boolean;
  bitmapOk: boolean;
}

export interface ScanSummary {
  scan: number;
  source: SourceSpec;
  label: string;
  fs: FsKind;
  clusterSize: number;
  volumeBytes: number;
  files: number;
  stats: ScanStats;
  counts: Partial<Record<State, number>>;
}

export type Category = "photo" | "video" | "document" | "archive" | "audio" | "other";
export type PathStatus = "live" | "deletedParent" | "lost";

export interface EntryView {
  record: number;
  name: string;
  folder: string;
  pathStatus: PathStatus;
  size: number;
  created: number;
  modified: number;
  ext: string;
  category: Category;
  assessment: Assessment;
  links: number;
  streams: number;
}

export type SortKey = "state" | "name" | "folder" | "size" | "modified";

export interface Query {
  text: string;
  states: State[];
  category: Category | null;
  sort: SortKey;
  desc: boolean;
  offset: number;
  limit: number;
}

export interface Page {
  total: number;
  rows: EntryView[];
  counts: Partial<Record<State, number>>;
}

export interface DiskSet {
  disks: number[];
  unknown: string | null;
  network: boolean;
  volume: string;
}

export type Verdict = { verdict: "ok"; source: number[]; dest: number[] } | { verdict: "sameDisk"; disks: number[] } | { verdict: "unknown"; reason: string; network: boolean };

export interface GuardView {
  verdict: Verdict;
  source: DiskSet;
  dest: DiskSet;
}

export type GuardSource = { kind: "spec"; spec: SourceSpec } | { kind: "bin"; ids: string[] };

export interface ShadowPick {
  path: string;
  isDir: boolean;
}

export type RecoverItems =
  | { kind: "ntfs"; scan: number; source: SourceSpec; records: number[] }
  | { kind: "recycleBin"; ids: string[] }
  | { kind: "shadow"; device: string; volume: string; created: number; picks: ShadowPick[] };

export interface ReportLine {
  original: string;
  recovered: string;
  size: number;
  method: "recycleBin" | "shadowCopy" | "ntfsRecord";
  state: State;
  reasonCode: string;
  reason: string;
  assessment: Assessment;
  sha256: string;
  verifiedWrite: boolean;
  zeroClusters: number;
  error: string | null;
}

export interface RunSummary {
  folder: string;
  source: string;
  started: string;
  finished: string;
  written: number;
  failed: number;
  bytes: number;
  lines: ReportLine[];
  appVersion: string;
}

export interface ProgressEvent {
  kind: string;
  stage: string;
  done: number;
  total: number;
  found: number;
  bytes: number;
  current: string;
}
