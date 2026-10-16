import { Cloud, FileSearch, History, LifeBuoy, type LucideIcon, Settings as SettingsIcon, ShieldCheck, ShieldOff, Trash2, X } from "lucide-react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { cx } from "../lib/cx";
import { useApp, type PageId } from "../store/app";

interface NavItem {
  id: PageId;
  icon: LucideIcon;
}

const SECTIONS: { label: string | null; items: NavItem[] }[] = [
  { label: null, items: [{ id: "start", icon: LifeBuoy }] },
  {
    label: "nav.recover",
    items: [
      { id: "recycle", icon: Trash2 },
      { id: "versions", icon: History },
      { id: "elsewhere", icon: Cloud },
      { id: "deleted", icon: FileSearch },
    ],
  },
];

export function Logo({ size = 26 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 1024 1024" aria-hidden>
      <rect width="1024" height="1024" rx="224" fill="var(--primary-fill)" />
      <path d="M308 292V548a204 204 0 0 0 408 0V292" fill="none" stroke="var(--primary-contrast)" strokeWidth="108" strokeLinecap="round" />
      <circle cx="512" cy="356" r="80" fill="var(--primary-contrast)" />
    </svg>
  );
}

function NavButton({ item, active, onClick }: { item: NavItem; active: boolean; onClick: () => void }) {
  const { t } = useTranslation();
  const Icon = item.icon;
  return (
    <button
      type="button"
      onClick={onClick}
      aria-current={active ? "page" : undefined}
      className={cx(
        "rail flex h-9 w-full items-center gap-2.5 rounded-md pl-3 pr-2 text-left text-[14px] transition-colors transition-fast",
        active ? "rail-active bg-primary-soft font-medium text-ink" : "text-ink-secondary hover:bg-surface-muted hover:text-ink",
      )}
    >
      <Icon size={17} strokeWidth={1.75} className={active ? "text-primary" : ""} aria-hidden />
      <span className="truncate">{t(`nav.${item.id}`)}</span>
    </button>
  );
}

function Sidebar() {
  const { t } = useTranslation();
  const page = useApp((s) => s.page);
  const go = useApp((s) => s.go);
  const info = useApp((s) => s.info);
  return (
    <aside className="flex h-full w-[232px] shrink-0 flex-col border-r border-line bg-surface">
      <div className="flex h-14 items-center gap-2.5 px-4">
        <Logo />
        <div className="leading-tight">
          <div className="display text-[16px] text-ink">{t("app.name")}</div>
          <div className="font-mono text-[11px] uppercase tracking-[0.12em] text-ink-muted">Recovery</div>
        </div>
      </div>
      <nav className="flex-1 space-y-4 overflow-y-auto px-3 pb-3 pt-1" aria-label="Main">
        {SECTIONS.map((s, i) => (
          <div key={i}>
            {s.label && <div className="label mb-1.5 px-3">{t(s.label)}</div>}
            <div className="space-y-0.5">
              {s.items.map((item) => (
                <NavButton key={item.id} item={item} active={page === item.id} onClick={() => go(item.id)} />
              ))}
            </div>
          </div>
        ))}
      </nav>
      <div className="space-y-0.5 border-t border-line px-3 py-3">
        <NavButton item={{ id: "settings", icon: SettingsIcon }} active={page === "settings"} onClick={() => go("settings")} />
      </div>
      <div className="flex items-center gap-1.5 border-t border-line px-4 py-3 text-[12px] text-ink-muted">
        {info?.adminReady ? <ShieldCheck size={14} strokeWidth={1.75} className="text-positive-fg" /> : <ShieldOff size={14} strokeWidth={1.75} />}
        <span>{info?.adminReady ? t("nav.admin") : t("nav.standard")}</span>
        {info?.portable && <span className="ml-auto rounded-sm bg-surface-muted px-1.5 font-mono text-[11px]">{t("nav.portable")}</span>}
      </div>
    </aside>
  );
}

function Toasts() {
  const toasts = useApp((s) => s.toasts);
  const dismiss = useApp((s) => s.dismissToast);
  return (
    <div className="z-toast pointer-events-none fixed bottom-5 right-5 flex w-[360px] flex-col gap-2" aria-live="polite">
      {toasts.map((t) => (
        <div
          key={t.id}
          className={cx(
            "anim-rise pointer-events-auto flex items-start gap-3 rounded-md border bg-canvas px-3.5 py-3 text-[14px] shadow-popover",
            t.tone === "positive" ? "border-positive" : t.tone === "danger" ? "border-danger" : t.tone === "warning" ? "border-warning" : "border-line",
          )}
        >
          <span className={cx("mt-1.5 h-2 w-2 shrink-0 rounded-full", t.tone === "positive" ? "bg-positive" : t.tone === "danger" ? "bg-danger" : t.tone === "warning" ? "bg-warning" : "bg-primary")} />
          <span className="selectable min-w-0 flex-1 text-ink">{t.text}</span>
          <button type="button" aria-label="close" onClick={() => dismiss(t.id)} className="text-ink-muted hover:text-ink">
            <X size={15} strokeWidth={1.75} />
          </button>
        </div>
      ))}
    </div>
  );
}

export function Shell({ children }: { children: ReactNode }) {
  return (
    <div className="app-shell flex h-full">
      <Sidebar />
      <main className="relative min-w-0 flex-1 overflow-y-auto" id="main">
        <div className="mx-auto w-full max-w-[1320px] px-6 pb-6 pt-6">{children}</div>
      </main>
      <Toasts />
    </div>
  );
}
