import { Folder, Info, RefreshCw, Trash2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { RecoverDialog, SelectionBar } from "../components/common";
import { Badge, Banner, Button, Card, Checkbox, EmptyState, PageHeader, Select, Skeleton } from "../components/ui";
import { driveName, formatDateTime, parentPath } from "../lib/format";
import { useBytes, useLocale } from "../lib/hooks";
import { useApp } from "../store/app";

export function RecycleBinPage() {
  const { t } = useTranslation();
  const fmt = useBytes();
  const locale = useLocale();
  const bin = useApp((s) => s.bin);
  const loadBin = useApp((s) => s.loadBin);
  const source = useApp((s) => s.sourceRoot);
  const [drive, setDrive] = useState<string>(source ?? "all");
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [dialog, setDialog] = useState(false);

  useEffect(() => {
    void loadBin();
  }, [loadBin]);

  const drives = useMemo(() => [...new Set((bin ?? []).map((b) => b.drive))].sort(), [bin]);
  // Items whose data still exists first; orphaned records (data gone) last.
  const items = useMemo(() => (bin ?? []).filter((b) => drive === "all" || b.drive === drive).sort((a, b) => Number(b.present) - Number(a.present) || b.deleted - a.deleted), [bin, drive]);
  const chosen = (bin ?? []).filter((b) => picked.has(b.id));
  const bytes = chosen.reduce((s, b) => s + b.size, 0);
  const selectable = items.filter((b) => b.present);
  const allOn = selectable.length > 0 && selectable.every((b) => picked.has(b.id));

  const toggle = (id: string, on: boolean) => {
    const next = new Set(picked);
    if (on) next.add(id);
    else next.delete(id);
    setPicked(next);
  };

  return (
    <div>
      <PageHeader
        title={t("recycle.title")}
        subtitle={t("recycle.subtitle")}
        actions={
          <>
            {drives.length > 1 && (
              <Select
                label={t("common.drive")}
                value={drive}
                onChange={setDrive}
                options={[{ value: "all", label: t("recycle.allDrives") }, ...drives.map((d) => ({ value: d, label: driveName(d) }))]}
              />
            )}
            <Button variant="ghost" icon={<RefreshCw size={15} strokeWidth={1.75} />} onClick={() => void loadBin()}>
              {t("common.refresh")}
            </Button>
          </>
        }
      />
      <div className="mb-4 space-y-2">
        <Banner icon={<Info size={16} strokeWidth={1.75} />}>{t("recycle.note")}</Banner>
      </div>
      <Card>
        {bin === null ? (
          <div className="space-y-2 p-4">
            {[0, 1, 2].map((i) => (
              <Skeleton key={i} className="h-10" />
            ))}
          </div>
        ) : items.length === 0 ? (
          <EmptyState icon={<Trash2 size={20} strokeWidth={1.75} />} title={t("recycle.empty")} body={t("recycle.emptyBody")} />
        ) : (
          <table className="w-full table-fixed text-left">
            <thead>
              <tr className="border-b border-line">
                <th className="w-10 px-4 py-2">
                  <Checkbox
                    checked={allOn}
                    indeterminate={!allOn && selectable.some((b) => picked.has(b.id))}
                    onChange={(on) => setPicked(on ? new Set([...picked, ...selectable.map((b) => b.id)]) : new Set([...picked].filter((id) => !selectable.some((b) => b.id === id))))}
                    label={t("common.selectAll")}
                  />
                </th>
                <th className="label px-2 py-2 font-normal">{t("common.name")}</th>
                <th className="label w-[110px] px-2 py-2 text-right font-normal">{t("common.size")}</th>
                <th className="label w-[180px] px-2 py-2 font-normal">{t("common.deleted")}</th>
                <th className="label w-[150px] px-4 py-2 font-normal">{t("common.state")}</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-line">
              {items.map((b) => (
                <tr key={b.id} className="hover:bg-surface">
                  <td className="px-4 py-2.5 align-top">
                    <Checkbox checked={picked.has(b.id)} disabled={!b.present} onChange={(on) => toggle(b.id, on)} label={b.name} />
                  </td>
                  <td className="min-w-0 px-2 py-2.5">
                    <div className="flex items-center gap-2">
                      {b.isDir && <Folder size={15} strokeWidth={1.75} className="shrink-0 text-ink-muted" aria-hidden />}
                      <span className="selectable truncate text-[14px] text-ink" title={b.name}>
                        {b.name}
                      </span>
                      {b.isDir && <Badge>{t("recycle.folderBadge")}</Badge>}
                    </div>
                    <div className="selectable truncate font-mono text-[12px] text-ink-muted" title={b.originalPath}>
                      {parentPath(b.originalPath)}
                    </div>
                  </td>
                  <td className="num px-2 py-2.5 text-right font-mono text-[13px] text-ink">{fmt(b.size)}</td>
                  <td className="px-2 py-2.5 font-mono text-[12px] text-ink-secondary">{formatDateTime(b.deleted, locale)}</td>
                  <td className="px-4 py-2.5">
                    {b.present ? (
                      <Badge tone="positive" title={t("state.hint.intactCopy")}>
                        {t("state.intactCopy")}
                      </Badge>
                    ) : (
                      <Badge tone="neutral" title={t("recycle.missingHint")}>
                        {t("recycle.missing")}
                      </Badge>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Card>
      <p className="mt-3 text-[12px] text-ink-muted">{t("recycle.privacy")}</p>
      <SelectionBar count={chosen.length} bytes={bytes}>
        {chosen.length > 0 && (
          <Button variant="ghost" onClick={() => setPicked(new Set())}>
            {t("common.clearSelection")}
          </Button>
        )}
        <Button variant="primary" disabled={chosen.length === 0} onClick={() => setDialog(true)}>
          {t("common.recoverSelected")}
        </Button>
      </SelectionBar>
      <RecoverDialog
        open={dialog}
        onClose={() => setDialog(false)}
        items={{ kind: "recycleBin", ids: chosen.map((b) => b.id) }}
        source={{ kind: "bin", ids: chosen.map((b) => b.id) }}
        count={chosen.length}
        bytes={bytes}
      />
    </div>
  );
}
