import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppInfo,
  BinItem,
  Drive,
  EntryView,
  GuardSource,
  GuardView,
  Page,
  Partition,
  ProgressEvent,
  Query,
  RecoverItems,
  RunSummary,
  ScanSummary,
  Settings,
  ShadowCopy,
  ShadowEntry,
  SourceSpec,
} from "./types";

/** Error with a stable code from the engine; `message` is the technical detail. */
export class ApiError extends Error {
  code: string;
  constructor(code: string, message: string) {
    super(message);
    this.code = code;
  }
}

export function isDesktop(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isDesktop()) throw new ApiError("desktop_only", "This view needs the Unlost desktop app.");
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    if (e && typeof e === "object" && "code" in e && "message" in e) {
      const p = e as { code: string; message: string };
      throw new ApiError(p.code, p.message);
    }
    throw new ApiError("other", String(e));
  }
}

export const api = {
  appInfo: () => call<AppInfo>("app_info"),
  settingsGet: () => call<Settings>("settings_get"),
  settingsSet: (settings: Settings) => call<Settings>("settings_set", { settings }),
  cancel: (kind: string) => call<void>("job_cancel", { kind }),
  reveal: (path: string) => call<void>("reveal_path", { path }),
  drives: () => call<Drive[]>("drives"),
  mediaDevices: () => call<string[]>("media_devices"),
  adminStart: () => call<boolean>("admin_start"),
  recycleList: () => call<BinItem[]>("recycle_list"),
  shadowList: () => call<ShadowCopy[]>("shadow_list"),
  shadowDir: (device: string, path: string) => call<ShadowEntry[]>("shadow_dir", { device, path }),
  imagePartitions: (path: string) => call<Partition[]>("image_partitions", { path }),
  /** `workspace`: a folder on another disk for the scan's database, when the app's data folder
   *  shares a disk with the source (the engine answers `workspace_same_disk` then). */
  scanStart: (source: SourceSpec, workspace?: string, confirmUnknown?: boolean) =>
    call<ScanSummary>("scan_start", { source, workspace: workspace ?? null, confirmUnknown: confirmUnknown ?? false }),
  scanQuery: (scan: number, query: Query) => call<Page>("scan_query", { scan, query }),
  scanEntries: (scan: number, records: number[]) => call<EntryView[]>("scan_entries", { scan, records }),
  scanForget: (scan: number) => call<void>("scan_forget", { scan }),
  guardCheck: (source: GuardSource, dest: string) => call<GuardView>("guard_check", { source, dest }),
  recover: (dest: string, confirmUnknown: boolean, items: RecoverItems) => call<RunSummary>("recover_start", { request: { dest, confirmUnknown, items } }),
};

export function onProgress(cb: (e: ProgressEvent) => void): Promise<UnlistenFn> {
  if (!isDesktop()) return Promise.resolve(() => {});
  return listen<ProgressEvent>("job-progress", (e) => cb(e.payload));
}

export async function pickFolder(): Promise<string | null> {
  if (!isDesktop()) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const res = await open({ directory: true, multiple: false });
  return typeof res === "string" ? res : null;
}

export async function pickImage(): Promise<string | null> {
  if (!isDesktop()) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const res = await open({ directory: false, multiple: false, filters: [{ name: "Disk images", extensions: ["vhdx", "img", "dd", "raw", "bin", "ima"] }] });
  return typeof res === "string" ? res : null;
}

/** Opens a help page in the user's browser. Only called from a click. */
export async function openUrl(url: string): Promise<void> {
  if (!isDesktop()) {
    window.open(url, "_blank", "noopener");
    return;
  }
  const { openUrl: open } = await import("@tauri-apps/plugin-opener");
  await open(url);
}
