import { ChevronRight, File, Folder, History, RefreshCw } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { AdminGate, RecoverDialog, SelectionBar } from "../components/common";
import { Badge, Button, Card, CardHeader, Checkbox, EmptyState, PageHeader, Skeleton } from "../components/ui";
import { api } from "../lib/api";
import { cx } from "../lib/cx";
import { driveName, formatDateTime } from "../lib/format";
import { useBytes, useLocale } from "../lib/hooks";
import type { ShadowCopy, ShadowEntry, ShadowPick } from "../lib/types";
import { errorText, useApp } from "../store/app";

function Browser({ copy }: { copy: ShadowCopy }) {
  const { t } = useTranslation();
  const fmt = useBytes();
  const locale = useLocale();
  const toast = useApp((s) => s.toast);
  const [path, setPath] = useState("");
  const [listing, setListing] = useState<{ path: string | null; entries: ShadowEntry[] }>({ path: null, entries: [] });
  const entries = listing.path === path ? listing.entries : null;
  const [picked, setPicked] = useState<Map<string, ShadowPick & { size: number }>>(new Map());
  const [dialog, setDialog] = useState(false);

  useEffect(() => {
    let live = true;
    api
      .shadowDir(copy.device, path)
      .then((e) => live && setListing({ path, entries: e }))
      .catch((e) => {
        if (!live) return;
        setListing({ path, entries: [] });
        toast("danger", errorText(e));
      });
    return () => {
      live = false;
    };
  }, [copy.device, path, toast]);

  const parts = path ? path.split("\\") : [];
  const child = (name: string) => (path ? `${path}\\${name}` : name);
  const toggle = (e: ShadowEntry, on: boolean) => {
    const next = new Map(picked);
    const p = child(e.name);
    if (on) next.set(p, { path: p, isDir: e.isDir, size: e.size });
    else next.delete(p);
    setPicked(next);
  };
  const chosen = [...picked.values()];
  const bytes = chosen.reduce((s, p) => s + p.size, 0);
  const root = driveName(copy.roots[0] ?? "");

  return (
    <>
      <Card>
        <div className="flex flex-wrap items-center gap-1 border-b border-line px-4 py-2.5 font-mono text-[13px]">
          <button type="button" className="rounded px-1 text-primary hover:bg-primary-soft" onClick={() => setPath("")}>
            {root || t("versions.root")}
          </button>
          {parts.map((p, i) => (
            <span key={i} className="flex items-center gap-1">
              <ChevronRight size={14} strokeWidth={1.75} className="text-ink-muted" aria-hidden />
              <button type="button" className="rounded px-1 text-primary hover:bg-primary-soft" onClick={() => setPath(parts.slice(0, i + 1).join("\\"))}>
                {p}
              </button>
            </span>
          ))}
        </div>
        {entries === null ? (
          <div className="space-y-2 p-4">
            {[0, 1, 2, 3].map((i) => (
              <Skeleton key={i} className="h-8" />
            ))}
          </div>
        ) : entries.length === 0 ? (
          <p className="px-4 py-8 text-center text-[14px] text-ink-muted">{t("versions.emptyFolder")}</p>
        ) : (
          <ul className="max-h-[520px] divide-y divide-line overflow-y-auto">
            {entries.map((e) => (
              <li key={e.name} className="rail flex items-center gap-3 px-4 py-2 hover:bg-surface">
                <Checkbox checked={picked.has(child(e.name))} onChange={(on) => toggle(e, on)} label={e.name} />
                {e.isDir ? <Folder size={16} strokeWidth={1.75} className="shrink-0 text-ink-muted" /> : <File size={16} strokeWidth={1.75} className="shrink-0 text-ink-muted" />}
                {e.isDir ? (
                  <button type="button" className="selectable min-w-0 flex-1 truncate text-left text-[14px] text-ink hover:text-primary" onClick={() => setPath(child(e.name))}>
                    {e.name}
                  </button>
                ) : (
                  <span className="selectable min-w-0 flex-1 truncate text-[14px] text-ink">{e.name}</span>
                )}
                <span className="num w-[90px] text-right font-mono text-[12px] text-ink-secondary">{e.isDir ? "" : fmt(e.size)}</span>
                <span className="w-[170px] font-mono text-[12px] text-ink-muted">{formatDateTime(e.modified, locale)}</span>
              </li>
            ))}
          </ul>
        )}
      </Card>
      <SelectionBar count={chosen.length} bytes={bytes}>
        <Button variant="primary" disabled={chosen.length === 0} onClick={() => setDialog(true)}>
          {t("common.recoverSelected")}
        </Button>
      </SelectionBar>
      <RecoverDialog
        open={dialog}
        onClose={() => setDialog(false)}
        items={{ kind: "shadow", device: copy.device, volume: copy.volume, created: copy.created, picks: chosen.map(({ path, isDir }) => ({ path, isDir })) }}
        source={{ kind: "spec", spec: { kind: "shadow", device: copy.device, volume: copy.volume } }}
        count={chosen.length}
        bytes={bytes}
      />
    </>
  );
}

export function VersionsPage() {
  const { t } = useTranslation();
  const locale = useLocale();
  const shadows = useApp((s) => s.shadows);
  const loadShadows = useApp((s) => s.loadShadows);
  const source = useApp((s) => s.sourceRoot);
  const adminReady = useApp((s) => s.info?.adminReady ?? false);
  const [current, setCurrent] = useState<string | null>(null);

  useEffect(() => {
    if (adminReady) void loadShadows();
  }, [adminReady, loadShadows]);

  const list = useMemo(() => {
    const all = shadows ?? [];
    const mine = source ? all.filter((c) => c.roots.some((r) => r.toUpperCase() === source.toUpperCase())) : [];
    return mine.length > 0 ? [...mine, ...all.filter((c) => !mine.includes(c))] : all;
  }, [shadows, source]);
  const copy = list.find((c) => c.id === current) ?? null;

  return (
    <div>
      <PageHeader
        title={t("versions.title")}
        subtitle={t("versions.subtitle")}
        actions={
          adminReady && (
            <Button variant="ghost" icon={<RefreshCw size={15} strokeWidth={1.75} />} onClick={() => void loadShadows()}>
              {t("common.refresh")}
            </Button>
          )
        }
      />
      <AdminGate why={t("versions.whyAdmin")}>
        {shadows === null ? (
          <Skeleton className="h-24" />
        ) : list.length === 0 ? (
          <Card>
            <EmptyState icon={<History size={20} strokeWidth={1.75} />} title={t("versions.none")} body={t("versions.noneBody")} />
          </Card>
        ) : (
          <div className="grid gap-4 lg:grid-cols-[300px_1fr]">
            <Card className="self-start">
              <CardHeader title={t("versions.copies")} />
              <ul className="divide-y divide-line">
                {list.map((c) => (
                  <li key={c.id}>
                    <button type="button" onClick={() => setCurrent(c.id)} className={cx("rail w-full px-4 py-2.5 text-left hover:bg-surface", c.id === current && "rail-active bg-primary-soft")}>
                      <div className="flex items-center gap-2">
                        <span className="display-medium text-[15px] text-ink">{c.roots.map(driveName).join(" ") || "?"}</span>
                        {source && c.roots.some((r) => r.toUpperCase() === source.toUpperCase()) && <Badge tone="primary">{t("common.drive")}</Badge>}
                      </div>
                      <div className="font-mono text-[12px] text-ink-muted">{t("versions.created", { date: formatDateTime(c.created, locale) })}</div>
                    </button>
                  </li>
                ))}
              </ul>
            </Card>
            <div className="min-w-0">{copy ? <Browser key={copy.id} copy={copy} /> : <Card><p className="px-4 py-10 text-center text-[14px] text-ink-muted">{t("versions.pick")}</p></Card>}</div>
          </div>
        )}
      </AdminGate>
    </div>
  );
}
