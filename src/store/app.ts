import { create } from "zustand";
import i18n, { setLanguage } from "../i18n";
import { ApiError, api } from "../lib/api";
import type { AppInfo, BinItem, Drive, ProgressEvent, ScanSummary, Settings, ShadowCopy } from "../lib/types";

export type PageId = "start" | "recycle" | "versions" | "deleted" | "elsewhere" | "settings";

export interface Toast {
  id: number;
  tone: "positive" | "danger" | "warning" | "neutral";
  text: string;
}

interface AppState {
  page: PageId;
  ready: boolean;
  bootError: string | null;
  info: AppInfo | null;
  settings: Settings | null;
  drives: Drive[];
  /** Phones and cameras connected as media devices (no drive letter, cannot be scanned). */
  phones: string[];
  /** The drive the user says the files were on (`D:\`). */
  sourceRoot: string | null;
  bin: BinItem[] | null;
  /** Existing shadow copies; `null` until listed (needs administrator access). */
  shadows: ShadowCopy[] | null;
  adminBusy: boolean;
  scan: ScanSummary | null;
  /** Last destination folder, offered again in the next recovery. */
  destination: string | null;
  progress: Record<string, ProgressEvent | undefined>;
  toasts: Toast[];

  go: (page: PageId) => void;
  boot: () => Promise<void>;
  refreshDrives: () => Promise<void>;
  saveSettings: (patch: Partial<Settings>) => Promise<void>;
  setSource: (root: string | null) => void;
  startAdmin: () => Promise<boolean>;
  loadShadows: () => Promise<void>;
  loadBin: () => Promise<void>;
  setScan: (s: ScanSummary | null) => void;
  setDestination: (d: string | null) => void;
  setProgress: (e: ProgressEvent) => void;
  toast: (tone: Toast["tone"], text: string) => void;
  dismissToast: (id: number) => void;
}

let toastId = 0;

export function errorText(e: unknown): string {
  if (e instanceof ApiError) {
    const key = `errors.${e.code}`;
    if (!i18n.exists(key)) return e.message;
    const detail = ["refused", "unsupported", "limit", "other", "io", "windows"].includes(e.code) ? e.message : "";
    return `${i18n.t(key)} ${detail}`.trim();
  }
  return e instanceof Error ? e.message : String(e);
}

export function applyTheme(pref: string) {
  const root = document.documentElement;
  if (pref === "light" || pref === "dark") root.setAttribute("data-theme", pref);
  else root.removeAttribute("data-theme");
  try {
    localStorage.setItem("unlost:theme", pref);
  } catch {
    // Not fatal: the theme still applies for this session.
  }
}

export const useApp = create<AppState>((set, get) => ({
  page: "start",
  ready: false,
  bootError: null,
  info: null,
  settings: null,
  drives: [],
  phones: [],
  sourceRoot: null,
  bin: null,
  shadows: null,
  adminBusy: false,
  scan: null,
  destination: null,
  progress: {},
  toasts: [],

  go: (page) => set({ page }),

  boot: async () => {
    try {
      const [info, settings, drives] = await Promise.all([api.appInfo(), api.settingsGet(), api.drives()]);
      setLanguage(settings.language);
      applyTheme(settings.theme);
      set({ info, settings, drives, ready: true });
      if (info.adminReady) void get().loadShadows();
      void api.mediaDevices().then((phones) => set({ phones }), () => {});
    } catch (e) {
      set({ bootError: errorText(e), ready: true });
    }
  },

  refreshDrives: async () => {
    try {
      const [drives, phones] = await Promise.all([api.drives(), api.mediaDevices().catch(() => [])]);
      set({ drives, phones });
    } catch (e) {
      get().toast("danger", errorText(e));
    }
  },

  saveSettings: async (patch) => {
    const current = get().settings;
    if (!current) return;
    const next = { ...current, ...patch };
    set({ settings: next });
    if (patch.language) setLanguage(next.language);
    if (patch.theme) applyTheme(next.theme);
    try {
      set({ settings: await api.settingsSet(next) });
    } catch (e) {
      get().toast("danger", errorText(e));
    }
  },

  setSource: (root) => set({ sourceRoot: root }),

  startAdmin: async () => {
    set({ adminBusy: true });
    try {
      const ok = await api.adminStart();
      if (!ok) {
        get().toast("warning", i18n.t("admin.declined"));
        return false;
      }
      const info = get().info;
      if (info) set({ info: { ...info, adminReady: true } });
      void get().loadShadows();
      return true;
    } catch (e) {
      get().toast("danger", e instanceof ApiError && e.code === "other_user" ? i18n.t("admin.otherUser") : errorText(e));
      return false;
    } finally {
      set({ adminBusy: false });
    }
  },

  loadShadows: async () => {
    try {
      set({ shadows: await api.shadowList() });
    } catch (e) {
      if (e instanceof ApiError && e.code === "needs_admin") return;
      get().toast("danger", errorText(e));
    }
  },

  loadBin: async () => {
    try {
      set({ bin: await api.recycleList() });
    } catch (e) {
      set({ bin: [] });
      get().toast("danger", errorText(e));
    }
  },

  setScan: (scan) => set({ scan }),
  setDestination: (destination) => set({ destination }),
  setProgress: (e) => set({ progress: { ...get().progress, [e.kind]: e } }),

  toast: (tone, text) => {
    const id = ++toastId;
    set({ toasts: [...get().toasts, { id, tone, text }] });
    window.setTimeout(() => get().dismissToast(id), tone === "danger" ? 8000 : 4500);
  },
  dismissToast: (id) => set({ toasts: get().toasts.filter((t) => t.id !== id) }),
}));
