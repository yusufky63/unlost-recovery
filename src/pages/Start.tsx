import { AlertOctagon, ArrowRight, Ban, Cloud, Download, FileSearch, HardDrive, History, RefreshCw, ShieldCheck, Smartphone, Trash2, Usb, XCircle } from "lucide-react";
import { useEffect, useMemo, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { UsageBar } from "../components/common";
import { Badge, Banner, Button, Card, EmptyState } from "../components/ui";
import { cx } from "../lib/cx";
import { driveName, formatCount } from "../lib/format";
import { useBytes, useLocale } from "../lib/hooks";
import { chanceTone, outlook, shadowCounts } from "../lib/outlook";
import type { Drive } from "../lib/types";
import { useApp, type PageId } from "../store/app";

function DriveCard({ d, selected, onSelect, shadows }: { d: Drive; selected: boolean; onSelect: () => void; shadows: number | null }) {
  const { t } = useTranslation();
  const fmt = useBytes();
  const appDisks = useApp((s) => s.info?.appDisks ?? []);
  const o = outlook(d, appDisks);
  const Icon = d.removable || d.media === "usbStick" || d.media === "sdCard" ? Usb : HardDrive;
  return (
    <button
      type="button"
      onClick={onSelect}
      aria-pressed={selected}
      className={cx("rail flex h-full flex-col gap-3 p-4 text-left transition-colors transition-fast hover:bg-surface", selected && "rail-active bg-primary-soft")}
    >
      <div className="flex items-start gap-3">
        <div className={cx("flex h-10 w-10 shrink-0 items-center justify-center rounded-md border", selected ? "border-primary text-primary" : "border-line text-ink-secondary")}>
          <Icon size={18} strokeWidth={1.75} />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline gap-2">
            <span className="display text-[20px] text-ink">{driveName(d.root)}</span>
            <span className="truncate text-[14px] text-ink-secondary">{d.label}</span>
          </div>
          <div className="truncate font-mono text-[12px] text-ink-muted" title={d.model}>
            {[d.fileSystem || t("drive.locked"), t(`drive.media.${d.media}`), d.disks.map((n) => t("drive.disk", { n })).join(", ")].filter(Boolean).join(" · ")}
          </div>
        </div>
        <Badge tone={chanceTone(o.chance)}>{t(`drive.chance.${o.chance}`)}</Badge>
      </div>
      {d.readable && (
        <div>
          <UsageBar used={d.total - d.free} total={d.total} />
          <div className="num mt-1 font-mono text-[12px] text-ink-muted">{t("drive.free", { free: fmt(d.free), total: fmt(d.total) })}</div>
        </div>
      )}
      <div className="flex flex-wrap gap-1.5">
        {d.media !== "hdd" && d.media !== "usbStick" && d.media !== "sdCard" && <Badge tone={d.trim === "on" ? "danger" : d.trim === "off" ? "positive" : "warning"}>{t(`drive.trim.${d.trim}`)}</Badge>}
        {d.system && <Badge tone="warning">{t("drive.system")}</Badge>}
        {d.removable && <Badge>{t("drive.removable")}</Badge>}
        {/* Removable drives do not use the Recycle Bin; a zero there would mislead. */}
        {(!d.removable || d.binItems > 0) && <Badge tone={d.binItems > 0 ? "positive" : "neutral"}>{t("drive.bin", { count: d.binItems })}</Badge>}
        <Badge tone={shadows ? "positive" : "neutral"}>{shadows === null ? t("drive.shadowsUnknown") : t("drive.shadows", { count: shadows })}</Badge>
      </div>
      <ul className="space-y-1">
        {o.notes.map((n) => (
          <li key={n.key} className="flex items-start gap-2 text-[13px] text-ink-secondary">
            <span className={cx("mt-[7px] h-1.5 w-1.5 shrink-0 rounded-full", n.tone === "positive" ? "bg-positive" : n.tone === "danger" ? "bg-danger" : n.tone === "warning" ? "bg-warning" : "bg-line-strong")} aria-hidden />
            <span>{t(`drive.note.${n.key}`, n.params)}</span>
          </li>
        ))}
      </ul>
    </button>
  );
}

function Step({ n, icon, title, hint, page, disabled, badge }: { n: number; icon: ReactNode; title: string; hint: string; page: PageId; disabled?: boolean; badge?: ReactNode }) {
  const { t } = useTranslation();
  const go = useApp((s) => s.go);
  return (
    <li className="rail flex items-center gap-4 px-4 py-3">
      <span className="num w-5 shrink-0 font-mono text-[13px] text-ink-muted">{String(n).padStart(2, "0")}</span>
      <span className="shrink-0 text-primary">{icon}</span>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-[14px] font-medium text-ink">{title}</span>
          {badge}
        </div>
        <div className="text-[13px] text-ink-muted">{hint}</div>
      </div>
      <Button size="sm" disabled={disabled} onClick={() => go(page)} icon={<ArrowRight size={15} strokeWidth={1.75} />}>
        {t("start.open")}
      </Button>
    </li>
  );
}

export function StartPage() {
  const { t } = useTranslation();
  const locale = useLocale();
  const drives = useApp((s) => s.drives);
  const source = useApp((s) => s.sourceRoot);
  const setSource = useApp((s) => s.setSource);
  const shadows = useApp((s) => s.shadows);
  const refreshDrives = useApp((s) => s.refreshDrives);
  const phones = useApp((s) => s.phones);
  const go = useApp((s) => s.go);
  const adminReady = useApp((s) => s.info?.adminReady ?? false);
  const counts = useMemo(() => shadowCounts(shadows), [shadows]);
  const selected = drives.find((d) => d.root === source) ?? null;

  useEffect(() => {
    void refreshDrives();
  }, [refreshDrives]);

  const sel = selected ? outlook(selected, []) : null;
  const name = selected ? driveName(selected.root) : "";

  return (
    <div className="space-y-6">
      <Card ticks className="anim-rise">
        <div className="grid gap-6 p-5 md:p-6 lg:grid-cols-[1.4fr_1fr]">
          <div>
            <div className="eyebrow mb-3">{t("start.eyebrow")}</div>
            <h1 className="display text-[24px] leading-[1.05] text-ink lg:text-[40px]">{t("start.title")}</h1>
            <p className="mt-3 max-w-[60ch] text-[15px] text-ink-secondary">{t("start.lead")}</p>
            <div className="mt-4 flex items-start gap-2 text-[13px] text-ink-secondary">
              <ShieldCheck size={16} strokeWidth={1.75} className="mt-0.5 shrink-0 text-positive-fg" />
              <span>{t("start.promise")}</span>
            </div>
          </div>
          <ul className="space-y-2.5 self-center">
            {[
              { icon: <Download size={16} strokeWidth={1.75} />, text: t("start.dont1") },
              { icon: <Ban size={16} strokeWidth={1.75} />, text: t("start.dont2") },
              { icon: <XCircle size={16} strokeWidth={1.75} />, text: t("start.dont3") },
            ].map((x, i) => (
              <li key={i} className="flex items-start gap-2.5 rounded-md border border-line bg-surface px-3 py-2 text-[14px] text-ink">
                <span className="mt-0.5 shrink-0 text-danger-fg">{x.icon}</span>
                <span>{x.text}</span>
              </li>
            ))}
          </ul>
        </div>
      </Card>

      <section>
        <div className="mb-3 flex flex-wrap items-end justify-between gap-3">
          <div>
            <div className="eyebrow">{t("start.pick")}</div>
            <p className="mt-1 text-[13px] text-ink-muted">{t("start.pickHint")}</p>
          </div>
          <Button size="sm" variant="ghost" icon={<RefreshCw size={15} strokeWidth={1.75} />} onClick={() => void refreshDrives()}>
            {t("common.refresh")}
          </Button>
        </div>
        {drives.length === 0 ? (
          <Card>
            <EmptyState icon={<HardDrive size={20} strokeWidth={1.75} />} title={t("start.noDrives")} />
          </Card>
        ) : (
          <div className={cx("module-grid grid-cols-1", drives.length > 1 && "md:grid-cols-2")}>
            {drives.map((d) => (
              <DriveCard key={d.root} d={d} selected={d.root === source} onSelect={() => setSource(d.root === source ? null : d.root)} shadows={counts ? (counts[d.root.toUpperCase()] ?? 0) : null} />
            ))}
            {/* Keeps the last row whole, so no empty grid area shows through. */}
            {drives.length > 1 && drives.length % 2 === 1 && <div className="hidden md:block" aria-hidden />}
          </div>
        )}
        {phones.length > 0 && (
          <div className="mt-3">
            <Banner
              tone="warning"
              icon={<Smartphone size={16} strokeWidth={1.75} />}
              action={
                <Button size="sm" variant="ghost" onClick={() => go("elsewhere")}>
                  {t("start.phoneOpen")}
                </Button>
              }
            >
              {t("start.phone", { names: phones.join(", ") })}
            </Banner>
          </div>
        )}
      </section>

      <section>
        <div className="eyebrow">{t("start.steps")}</div>
        <p className="mb-3 mt-1 text-[13px] text-ink-muted">{t("start.stepsHint")}</p>
        {selected && sel ? (
          <Card className="anim-fade">
            {selected.system && (
              <div className="border-b border-line p-3">
                <Banner tone="warning" icon={<AlertOctagon size={16} strokeWidth={1.75} />}>
                  {t("start.system")}
                </Banner>
              </div>
            )}
            <ol className="divide-y divide-line">
              <Step
                n={1}
                icon={<Trash2 size={18} strokeWidth={1.75} />}
                title={t("start.stepBin")}
                hint={selected.removable ? t("start.stepBinRemovable") : selected.binItems > 0 ? t("start.stepBinHint", { count: formatCount(selected.binItems, locale), drive: name }) : t("start.stepBinNone", { drive: name })}
                page="recycle"
                badge={selected.binItems > 0 ? <Badge tone="positive">{t("state.intactCopy")}</Badge> : undefined}
              />
              <Step
                n={2}
                icon={<History size={18} strokeWidth={1.75} />}
                title={t("start.stepVersions")}
                hint={t("start.stepVersionsHint", { drive: name })}
                page="versions"
                badge={!adminReady ? <Badge tone="primary">{t("common.adminNeeded")}</Badge> : counts ? <Badge tone={(counts[selected.root.toUpperCase()] ?? 0) > 0 ? "positive" : "neutral"}>{t("drive.shadows", { count: counts[selected.root.toUpperCase()] ?? 0 })}</Badge> : undefined}
              />
              <Step n={3} icon={<Cloud size={18} strokeWidth={1.75} />} title={t("start.stepElsewhere")} hint={t("start.stepElsewhereHint")} page="elsewhere" />
              <Step
                n={4}
                icon={<FileSearch size={18} strokeWidth={1.75} />}
                title={t("start.stepDeleted")}
                hint={sel.scannable ? t("start.stepDeletedHint", { drive: name }) : t("deleted.notNtfs", { drive: name, fs: selected.fileSystem || "?" })}
                page="deleted"
                disabled={!sel.scannable}
                badge={!adminReady && sel.scannable ? <Badge tone="primary">{t("common.adminNeeded")}</Badge> : undefined}
              />
            </ol>
          </Card>
        ) : (
          <Card>
            <p className="px-4 py-6 text-center text-[14px] text-ink-muted">{t("start.selectDrive")}</p>
          </Card>
        )}
      </section>
    </div>
  );
}
