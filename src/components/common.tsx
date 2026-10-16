import { AlertTriangle, CheckCircle2, FolderOpen, FolderSearch, HardDrive, ShieldCheck, ShieldOff } from "lucide-react";
import { useCallback, useEffect, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { api, pickFolder } from "../lib/api";
import { cx } from "../lib/cx";
import { formatCount } from "../lib/format";
import { useBytes, useLocale, useReasonText } from "../lib/hooks";
import { stateTone } from "../lib/states";
import type { Assessment, GuardSource, GuardView, RecoverItems, RunSummary } from "../lib/types";
import { errorText, useApp } from "../store/app";
import { Badge, Banner, Button, Card, Checkbox, Dialog, ProgressBar } from "./ui";

export function StateBadge({ assessment, ext }: { assessment: Assessment; ext?: string }) {
  const { t } = useTranslation();
  const reason = useReasonText();
  return (
    <Badge tone={stateTone(assessment.state)} title={`${t(`state.hint.${assessment.state}`)}\n${reason(assessment, ext)}`}>
      {t(`state.${assessment.state}`)}
    </Badge>
  );
}

export function UsageBar({ used, total, className }: { used: number; total: number; className?: string }) {
  const pct = total > 0 ? Math.min(100, (used / total) * 100) : 0;
  return (
    <div className={cx("h-1.5 w-full overflow-hidden rounded-full bg-surface-muted", className)}>
      <div className="h-full rounded-full bg-primary" style={{ width: `${pct}%` }} />
    </div>
  );
}

/** Live progress of one job kind, with a cancel button. */
export function ProgressPanel({ kind, label, detail, className }: { kind: string; label: string; detail?: (p: { done: number; total: number; found: number; bytes: number }) => string; className?: string }) {
  const { t } = useTranslation();
  const p = useApp((s) => s.progress[kind]);
  const ratio = p && p.total > 0 ? p.done / p.total : null;
  return (
    <div className={cx("rounded-lg border border-line bg-canvas px-4 py-3", className)} aria-live="polite">
      <div className="mb-2 flex items-center justify-between gap-3">
        <div className="min-w-0">
          <div className="text-[14px] font-medium text-ink">{label}</div>
          <div className="num truncate font-mono text-[12px] text-ink-muted">{p && detail ? detail(p) : "…"}</div>
        </div>
        <Button size="sm" variant="ghost" onClick={() => void api.cancel(kind)}>
          {t("common.cancel")}
        </Button>
      </div>
      <ProgressBar value={ratio} />
      {p?.current && (
        <div className="selectable mt-2 truncate font-mono text-[12px] text-ink-muted" title={p.current}>
          {p.current}
        </div>
      )}
    </div>
  );
}

/** Explains why administrator access is needed and asks for it; renders children once it is on. */
export function AdminGate({ why, children }: { why: string; children: ReactNode }) {
  const { t } = useTranslation();
  const ready = useApp((s) => s.info?.adminReady ?? false);
  const busy = useApp((s) => s.adminBusy);
  const startAdmin = useApp((s) => s.startAdmin);
  if (ready) return <>{children}</>;
  return (
    <Card ticks className="anim-rise">
      <div className="flex flex-col gap-4 p-5 md:flex-row md:items-start">
        <div className="flex h-11 w-11 shrink-0 items-center justify-center rounded-lg border border-line bg-surface text-primary">
          <ShieldOff size={20} strokeWidth={1.75} />
        </div>
        <div className="min-w-0 flex-1">
          <h2 className="display-medium text-[18px] text-ink">{t("admin.title")}</h2>
          <p className="mt-1 max-w-[72ch] text-[14px] text-ink-secondary">{why}</p>
          <p className="mt-2 max-w-[72ch] text-[13px] text-ink-muted">{t("admin.body")}</p>
          <Button variant="primary" className="mt-4" loading={busy} icon={<ShieldCheck size={16} strokeWidth={1.75} />} onClick={() => void startAdmin()}>
            {t("admin.grant")}
          </Button>
        </div>
      </div>
    </Card>
  );
}

/** Folder picker that shows the destination guard's verdict for `source`. The texts default to
 *  the recovery destination's; the scan-data folder passes its own. */
export function DestinationPicker({ source, onReady, title, hint, footnote }: { source: GuardSource; onReady: (dest: string | null, confirmUnknown: boolean) => void; title?: string; hint?: string; footnote?: string }) {
  const { t } = useTranslation();
  const dest = useApp((s) => s.destination);
  const setDestination = useApp((s) => s.setDestination);
  const sourceKey = JSON.stringify(source);
  const key = dest ? `${dest}|${sourceKey}` : "";
  // Results are stored with the request they answer, so a stale answer is simply ignored.
  const [result, setResult] = useState<{ key: string; view: GuardView | null; error: string | null }>({ key: "", view: null, error: null });
  const [confirm, setConfirm] = useState<{ key: string; on: boolean }>({ key: "", on: false });
  const current = result.key === key;
  const view = current ? result.view : null;
  const error = current ? result.error : null;
  const checking = !!dest && !current;
  const confirmed = confirm.key === key && confirm.on;
  const setConfirmed = (on: boolean) => setConfirm({ key, on });

  useEffect(() => {
    if (!dest) return;
    let live = true;
    const k = `${dest}|${sourceKey}`;
    api
      .guardCheck(JSON.parse(sourceKey) as GuardSource, dest)
      .then((v) => live && setResult({ key: k, view: v, error: null }))
      .catch((e) => live && setResult({ key: k, view: null, error: errorText(e) }));
    return () => {
      live = false;
    };
  }, [dest, sourceKey]);

  const verdict = view?.verdict;
  const ok = verdict?.verdict === "ok" || (verdict?.verdict === "unknown" && confirmed);
  useEffect(() => {
    onReady(ok && dest ? dest : null, verdict?.verdict === "unknown" && confirmed);
  }, [ok, dest, confirmed, verdict?.verdict, onReady]);

  const choose = async () => {
    const picked = await pickFolder();
    if (picked) setDestination(picked);
  };

  return (
    <div className="space-y-3">
      <div>
        <div className="text-[14px] font-medium text-ink">{title ?? t("dest.title")}</div>
        <p className="mt-0.5 text-[13px] text-ink-muted">{hint ?? t("dest.hint")}</p>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <div className="selectable flex h-9 min-w-0 flex-1 items-center gap-2 rounded-md border border-line-strong bg-canvas px-3 font-mono text-[13px] text-ink">
          <FolderOpen size={15} strokeWidth={1.75} className="shrink-0 text-ink-muted" aria-hidden />
          <span className="truncate" title={dest ?? ""}>
            {dest ?? t("dest.choose")}
          </span>
        </div>
        <Button icon={<FolderSearch size={16} strokeWidth={1.75} />} onClick={() => void choose()}>
          {t("common.choose")}
        </Button>
      </div>
      {checking && <Banner>{t("dest.checking")}</Banner>}
      {error && (
        <Banner tone="danger" icon={<AlertTriangle size={16} strokeWidth={1.75} />}>
          {error}
        </Banner>
      )}
      {verdict?.verdict === "ok" && (
        <Banner tone="positive" icon={<HardDrive size={16} strokeWidth={1.75} />}>
          {t("dest.ok", { dest: verdict.dest.join(", "), source: verdict.source.join(", ") })}
        </Banner>
      )}
      {verdict?.verdict === "sameDisk" && (
        <Banner tone="danger" icon={<AlertTriangle size={16} strokeWidth={1.75} />}>
          {t("dest.sameDisk", { disks: verdict.disks.join(", ") })}
        </Banner>
      )}
      {verdict?.verdict === "unknown" && (
        <Banner tone="warning" icon={<AlertTriangle size={16} strokeWidth={1.75} />}>
          <div>{t("dest.unknown", { reason: verdict.reason })}</div>
          {verdict.network && <div className="mt-1">{t("dest.network")}</div>}
          <label className="mt-2 flex items-center gap-2">
            <Checkbox checked={confirmed} onChange={setConfirmed} label={t("dest.confirmUnknown")} />
            <span>{t("dest.confirmUnknown")}</span>
          </label>
        </Banner>
      )}
      <p className="text-[12px] text-ink-muted">{footnote ?? t("dest.folderName")}</p>
    </div>
  );
}

/** Choose a destination, run the recovery, show the result. */
export function RecoverDialog({ open, onClose, items, source, count, bytes }: { open: boolean; onClose: () => void; items: RecoverItems; source: GuardSource; count: number; bytes: number }) {
  const { t } = useTranslation();
  const fmt = useBytes();
  const locale = useLocale();
  const toast = useApp((s) => s.toast);
  const [target, setTarget] = useState<{ dest: string | null; confirm: boolean }>({ dest: null, confirm: false });
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<RunSummary | null>(null);

  const onReady = useCallback((dest: string | null, confirm: boolean) => setTarget({ dest, confirm }), []);

  const start = async () => {
    if (!target.dest) return;
    setRunning(true);
    try {
      setResult(await api.recover(target.dest, target.confirm, items));
    } catch (e) {
      toast("danger", errorText(e));
    } finally {
      setRunning(false);
    }
  };

  const close = () => {
    setResult(null);
    onClose();
  };
  const failed = result?.lines.filter((l) => l.error) ?? [];
  const verified = result?.lines.filter((l) => l.verifiedWrite).length ?? 0;

  return (
    <Dialog
      open={open}
      onClose={running ? () => {} : close}
      wide
      title={result ? t("run.doneTitle") : t("run.title")}
      footer={
        result ? (
          <>
            <Button icon={<FolderOpen size={16} strokeWidth={1.75} />} onClick={() => void api.reveal(result.folder)}>
              {t("common.showInFolder")}
            </Button>
            <Button variant="primary" onClick={close}>
              {t("common.close")}
            </Button>
          </>
        ) : (
          <>
            <Button variant="ghost" onClick={close} disabled={running}>
              {t("common.cancel")}
            </Button>
            <Button variant="primary" disabled={!target.dest} loading={running} onClick={() => void start()}>
              {t("run.start")}
            </Button>
          </>
        )
      }
    >
      {result ? (
        <div className="space-y-4">
          <div className="module-grid grid-cols-3">
            <div className="px-4 py-3">
              <div className="label">{t("run.written")}</div>
              <div className="num mt-1 font-mono text-[20px] text-ink">{formatCount(result.written, locale)}</div>
              <div className="font-mono text-[12px] text-ink-muted">{fmt(result.bytes)}</div>
            </div>
            <div className="px-4 py-3">
              <div className="label">{t("run.verified")}</div>
              <div className="num mt-1 font-mono text-[20px] text-ink">{formatCount(verified, locale)}</div>
            </div>
            <div className="px-4 py-3">
              <div className="label">{t("run.failed")}</div>
              <div className={cx("num mt-1 font-mono text-[20px]", result.failed > 0 ? "text-danger-fg" : "text-ink")}>{formatCount(result.failed, locale)}</div>
            </div>
          </div>
          <div className="selectable truncate font-mono text-[12px] text-ink-muted" title={result.folder}>
            {result.folder}
          </div>
          <Banner tone="positive" icon={<CheckCircle2 size={16} strokeWidth={1.75} />}>
            {t("run.report")}
          </Banner>
          <Banner tone="warning" icon={<AlertTriangle size={16} strokeWidth={1.75} />}>
            {t("run.neverOpened")}
          </Banner>
          {failed.length > 0 && (
            <div>
              <div className="label mb-1.5">{t("run.failedList")}</div>
              <ul className="max-h-[160px] divide-y divide-line overflow-y-auto rounded-md border border-line">
                {failed.slice(0, 200).map((l, i) => (
                  <li key={i} className="selectable px-3 py-1.5 font-mono text-[12px]">
                    <div className="truncate text-ink" title={l.original}>
                      {l.original}
                    </div>
                    <div className="truncate text-ink-muted">{l.error}</div>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      ) : (
        <div className="space-y-4">
          <p className="num font-mono text-[13px] text-ink-secondary">{t("run.summary", { count, size: fmt(bytes) })}</p>
          <DestinationPicker source={source} onReady={onReady} />
          {running && <ProgressPanel kind="recover" label={t("run.running")} detail={(p) => t("run.progress", { done: formatCount(p.done, locale), total: formatCount(p.total, locale), bytes: fmt(p.bytes) })} />}
        </div>
      )}
    </Dialog>
  );
}

/** Sticky footer that summarises a selection and holds the main action. */
export function SelectionBar({ count, bytes, children }: { count: number; bytes: number; children: ReactNode }) {
  const { t } = useTranslation();
  const fmt = useBytes();
  return (
    <div className="z-sticky sticky bottom-0 -mx-6 mt-4 flex flex-wrap items-center justify-between gap-3 border-t border-line bg-canvas px-6 py-3">
      <div className="num font-mono text-[14px] text-ink">{count > 0 ? t("common.selected", { count, size: fmt(bytes) }) : t("common.nothingSelected")}</div>
      <div className="flex flex-wrap items-center gap-2">{children}</div>
    </div>
  );
}
