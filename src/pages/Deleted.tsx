import { FileSearch, FolderX, HardDrive, Image as ImageIcon, Info, Search } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { AdminGate, ProgressPanel, RecoverDialog, SelectionBar, StateBadge } from "../components/common";
import { Badge, Banner, Button, Card, Checkbox, EmptyState, PageHeader, Segmented, Select, Stat } from "../components/ui";
import { api, pickImage } from "../lib/api";
import { cx } from "../lib/cx";
import { baseName, driveName, formatCount, formatDateTime } from "../lib/format";
import { useBytes, useLocale, useReasonText } from "../lib/hooks";
import { isNtfs } from "../lib/outlook";
import { USEFUL_STATES, recoverable, stateTone } from "../lib/states";
import type { Category, EntryView, Page, Partition, SortKey, SourceSpec, State } from "../lib/types";
import { errorText, useApp } from "../store/app";

const PAGE_SIZE = 200;
const CATEGORIES: (Category | "all")[] = ["all", "photo", "video", "document", "archive", "audio", "other"];
const SORTS: SortKey[] = ["state", "modified", "size", "name", "folder"];

function SourcePicker({ onScan, busy }: { onScan: (s: SourceSpec) => void; busy: boolean }) {
  const { t } = useTranslation();
  const fmt = useBytes();
  const drives = useApp((s) => s.drives);
  const sourceRoot = useApp((s) => s.sourceRoot);
  const adminReady = useApp((s) => s.info?.adminReady ?? false);
  const toast = useApp((s) => s.toast);
  const [mode, setMode] = useState<"drive" | "image">("drive");
  const ntfs = drives.filter((d) => isNtfs(d.fileSystem) && d.readable);
  const [root, setRoot] = useState<string>(sourceRoot && ntfs.some((d) => d.root === sourceRoot) ? sourceRoot : (ntfs[0]?.root ?? ""));
  const [image, setImage] = useState<string | null>(null);
  const [parts, setParts] = useState<Partition[] | null>(null);
  const [part, setPart] = useState<number>(0);
  const picked = drives.find((d) => d.root === sourceRoot);

  const chooseImage = async () => {
    const p = await pickImage();
    if (!p) return;
    setImage(p);
    setParts(null);
    try {
      const list = await api.imagePartitions(p);
      setParts(list);
      setPart(list.find((x) => x.fs === "ntfs")?.index ?? list[0]?.index ?? 0);
    } catch (e) {
      toast("danger", errorText(e));
    }
  };

  const scanButton = (spec: SourceSpec | null) => (
    <Button variant="primary" icon={<FileSearch size={16} strokeWidth={1.75} />} disabled={!spec || busy} loading={busy} onClick={() => spec && onScan(spec)}>
      {t("deleted.scan")}
    </Button>
  );

  return (
    <Card>
      <div className="space-y-4 p-4">
        <Segmented
          value={mode}
          onChange={setMode}
          options={[
            { value: "drive", label: <><HardDrive size={14} strokeWidth={1.75} />{t("deleted.sourceDrive")}</> },
            { value: "image", label: <><ImageIcon size={14} strokeWidth={1.75} />{t("deleted.sourceImage")}</> },
          ]}
        />
        {mode === "drive" ? (
          <>
            {picked && !isNtfs(picked.fileSystem) && <Banner tone="warning">{t("deleted.notNtfs", { drive: driveName(picked.root), fs: picked.fileSystem || "?" })}</Banner>}
            {adminReady ? (
              <div className="flex flex-wrap items-center gap-2">
                <Select
                  label={t("common.drive")}
                  value={root}
                  onChange={setRoot}
                  className="min-w-[260px]"
                  options={ntfs.map((d) => ({ value: d.root, label: `${driveName(d.root)} ${d.label} · ${fmt(d.total)}` }))}
                />
                {scanButton(root ? { kind: "volume", root } : null)}
              </div>
            ) : (
              <AdminGate why={t("deleted.whyAdmin")}>{null}</AdminGate>
            )}
            <p className="text-[12px] text-ink-muted">{t("deleted.sourceWarning")}</p>
          </>
        ) : (
          <>
            <p className="text-[13px] text-ink-muted">{t("deleted.imageHint")}</p>
            <div className="flex flex-wrap items-center gap-2">
              <Button icon={<ImageIcon size={16} strokeWidth={1.75} />} onClick={() => void chooseImage()}>
                {t("deleted.chooseImage")}
              </Button>
              {image && (
                <span className="selectable truncate font-mono text-[12px] text-ink-secondary" title={image}>
                  {baseName(image)}
                </span>
              )}
              {parts && parts.length > 1 && (
                <Select
                  label={t("deleted.partition")}
                  value={part}
                  onChange={setPart}
                  options={parts.map((p) => ({ value: p.index, label: t("deleted.partitionLabel", { index: p.index, fs: p.fs.toUpperCase(), size: fmt(p.length) }) }))}
                />
              )}
              {scanButton(image && parts ? { kind: "image", path: image, partition: part } : null)}
            </div>
          </>
        )}
      </div>
    </Card>
  );
}

function Results() {
  const { t } = useTranslation();
  const fmt = useBytes();
  const locale = useLocale();
  const reason = useReasonText();
  const scan = useApp((s) => s.scan)!;
  const toast = useApp((s) => s.toast);
  const [text, setText] = useState("");
  const [query, setQuery] = useState("");
  const [states, setStates] = useState<State[]>(USEFUL_STATES);
  const [category, setCategory] = useState<Category | "all">("all");
  const [sort, setSort] = useState<SortKey>("state");
  const [offset, setOffset] = useState(0);
  const [page, setPage] = useState<Page | null>(null);
  const [picked, setPicked] = useState<Map<number, EntryView>>(new Map());
  const [dialog, setDialog] = useState(false);

  useEffect(() => {
    const id = window.setTimeout(() => {
      setQuery(text);
      setOffset(0);
    }, 250);
    return () => window.clearTimeout(id);
  }, [text]);

  useEffect(() => {
    let live = true;
    api
      .scanQuery(scan.scan, { text: query, states, category: category === "all" ? null : category, sort, desc: sort === "modified" || sort === "size", offset, limit: PAGE_SIZE })
      .then((p) => live && setPage(p))
      .catch((e) => live && toast("danger", errorText(e)));
    return () => {
      live = false;
    };
  }, [scan.scan, query, states, category, sort, offset, toast]);

  const rows = page?.rows ?? [];
  const pageRows = rows.filter((r) => recoverable(r.assessment.state));
  const allOn = pageRows.length > 0 && pageRows.every((r) => picked.has(r.record));
  const chosen = [...picked.values()];
  const bytes = chosen.reduce((s, r) => s + r.size, 0);
  const pages = Math.max(1, Math.ceil((page?.total ?? 0) / PAGE_SIZE));
  const count = (s: State) => scan.counts[s] ?? 0;
  const useful = USEFUL_STATES.join() === states.join();

  const toggle = (r: EntryView, on: boolean) => {
    const next = new Map(picked);
    if (on) next.set(r.record, r);
    else next.delete(r.record);
    setPicked(next);
  };
  const setFilter = (next: State[]) => {
    setStates(next);
    setOffset(0);
  };

  const stateChip = (s: State) => {
    const on = states.includes(s);
    const n = page?.counts[s] ?? 0;
    return (
      <button
        key={s}
        type="button"
        aria-pressed={on}
        onClick={() => setFilter(on ? states.filter((x) => x !== s) : [...states, s])}
        className={cx("inline-flex h-7 items-center gap-1.5 rounded-md border px-2.5 text-[12px] transition-colors transition-fast", on ? "border-primary bg-primary-soft text-ink" : "border-line text-ink-secondary hover:border-line-strong")}
      >
        <span className={cx("h-1.5 w-1.5 rounded-full", { positive: "bg-positive", warning: "bg-warning", danger: "bg-danger", neutral: "bg-line-strong", primary: "bg-primary" }[stateTone(s)])} aria-hidden />
        {t(`state.${s}`)}
        <span className="num font-mono text-ink-muted">{formatCount(n, locale)}</span>
      </button>
    );
  };

  return (
    <div className="space-y-4">
      <div className="module-grid grid-cols-2 lg:grid-cols-4">
        <Stat label={t("deleted.found")} value={formatCount(scan.files, locale)} sub={scan.label || driveName(scan.source.kind === "volume" ? scan.source.root : "")} />
        <Stat label={t("deleted.recoverable")} value={formatCount(count("likelyIntact"), locale)} dot={<span className="h-2 w-2 rounded-full bg-positive" />} />
        <Stat label={t("deleted.partial")} value={formatCount(count("partial"), locale)} dot={<span className="h-2 w-2 rounded-full bg-warning" />} />
        <Stat label={t("deleted.gone")} value={formatCount(count("zeroed") + count("overwritten"), locale)} sub={`${t("deleted.nameOnly")}: ${formatCount(count("metadataOnly") + count("notRecoverableHere"), locale)}`} dot={<span className="h-2 w-2 rounded-full bg-danger" />} />
      </div>
      {scan.stats.truncated && <Banner tone="warning">{t("deleted.truncated", { count: formatCount(scan.files, locale) })}</Banner>}
      <Card>
        <div className="space-y-3 border-b border-line p-4">
          <div className="flex flex-wrap items-center gap-2">
            <label className="field flex min-w-[240px] flex-1 items-center gap-2">
              <Search size={15} strokeWidth={1.75} className="shrink-0 text-ink-muted" aria-hidden />
              <input value={text} onChange={(e) => setText(e.target.value)} placeholder={t("common.search")} aria-label={t("common.search")} className="h-full w-full bg-transparent outline-none" />
            </label>
            <Select label={t("deleted.typeFilter")} value={category} onChange={(c) => { setCategory(c); setOffset(0); }} options={CATEGORIES.map((c) => ({ value: c, label: t(`deleted.category.${c}`) }))} />
            <Select label={t("deleted.sortBy")} value={sort} onChange={(s) => { setSort(s); setOffset(0); }} options={SORTS.map((s) => ({ value: s, label: t(`deleted.sort.${s}`) }))} />
          </div>
          <div className="flex flex-wrap items-center gap-1.5">
            <Button size="sm" variant={useful ? "primary" : "secondary"} onClick={() => setFilter(USEFUL_STATES)}>
              {t("deleted.filterUseful")}
            </Button>
            <Button size="sm" variant={states.length === 0 ? "primary" : "secondary"} onClick={() => setFilter([])}>
              {t("deleted.filterAll")}
            </Button>
            <span className="mx-1 h-5 w-px bg-line" aria-hidden />
            {(["likelyIntact", "partial", "zeroed", "overwritten", "metadataOnly", "notRecoverableHere"] as State[]).map(stateChip)}
          </div>
        </div>
        {page && rows.length === 0 ? (
          <EmptyState icon={<FolderX size={20} strokeWidth={1.75} />} title={t("deleted.empty")} body={t("deleted.emptyBody")} />
        ) : (
          <table className="w-full table-fixed text-left">
            <thead>
              <tr className="border-b border-line">
                <th className="w-10 px-4 py-2">
                  <Checkbox
                    checked={allOn}
                    indeterminate={!allOn && pageRows.some((r) => picked.has(r.record))}
                    onChange={(on) => {
                      const next = new Map(picked);
                      for (const r of pageRows) {
                        if (on) next.set(r.record, r);
                        else next.delete(r.record);
                      }
                      setPicked(next);
                    }}
                    label={t("common.selectAll")}
                  />
                </th>
                <th className="label px-2 py-2 font-normal">{t("common.name")}</th>
                <th className="label w-[100px] px-2 py-2 text-right font-normal">{t("common.size")}</th>
                <th className="label w-[170px] px-2 py-2 font-normal">{t("common.modified")}</th>
                <th className="label w-[34%] px-4 py-2 font-normal">{t("common.state")}</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-line">
              {rows.map((r) => {
                const ok = recoverable(r.assessment.state);
                return (
                  <tr key={r.record} className="align-top hover:bg-surface">
                    <td className="px-4 py-2.5">
                      <Checkbox checked={picked.has(r.record)} disabled={!ok} onChange={(on) => toggle(r, on)} label={r.name} />
                    </td>
                    <td className="min-w-0 px-2 py-2.5">
                      <div className="flex items-center gap-2">
                        <span className="selectable truncate text-[14px] text-ink" title={r.name}>
                          {r.name}
                        </span>
                        {r.links > 1 && <Badge>{t("deleted.links", { count: r.links })}</Badge>}
                      </div>
                      <div className="flex items-center gap-1.5">
                        {r.pathStatus === "lost" && (
                          <Badge tone="warning" title={t("deleted.lostHint")}>
                            {t("deleted.lost")}
                          </Badge>
                        )}
                        <span className="selectable truncate font-mono text-[12px] text-ink-muted" title={r.pathStatus === "deletedParent" ? `${r.folder}\n${t("deleted.deletedParent")}` : r.folder}>
                          {r.folder}
                        </span>
                      </div>
                    </td>
                    <td className="num px-2 py-2.5 text-right font-mono text-[13px] text-ink">{fmt(r.size)}</td>
                    <td className="px-2 py-2.5 font-mono text-[12px] text-ink-secondary">{formatDateTime(r.modified, locale)}</td>
                    <td className="px-4 py-2.5">
                      <StateBadge assessment={r.assessment} ext={r.ext} />
                      <div className="mt-1 text-[12px] leading-snug text-ink-muted">{reason(r.assessment, r.ext)}</div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
        {page && page.total > PAGE_SIZE && (
          <div className="flex items-center justify-between gap-3 border-t border-line px-4 py-2.5">
            <span className="num font-mono text-[12px] text-ink-muted">{t("common.page", { page: Math.floor(offset / PAGE_SIZE) + 1, pages })}</span>
            <div className="flex gap-2">
              <Button size="sm" disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}>
                {t("common.previous")}
              </Button>
              <Button size="sm" disabled={offset + PAGE_SIZE >= page.total} onClick={() => setOffset(offset + PAGE_SIZE)}>
                {t("common.next")}
              </Button>
            </div>
          </div>
        )}
      </Card>
      <SelectionBar count={chosen.length} bytes={bytes}>
        {chosen.length > 0 && (
          <Button variant="ghost" onClick={() => setPicked(new Map())}>
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
        items={{ kind: "ntfs", scan: scan.scan, source: scan.source, records: chosen.map((r) => r.record) }}
        source={{ kind: "spec", spec: scan.source }}
        count={chosen.length}
        bytes={bytes}
      />
    </div>
  );
}

export function DeletedPage() {
  const { t } = useTranslation();
  const locale = useLocale();
  const scan = useApp((s) => s.scan);
  const setScan = useApp((s) => s.setScan);
  const toast = useApp((s) => s.toast);
  const [busy, setBusy] = useState(false);
  const stageLabel = (stage: string) => (["mft", "paths", "sample"].includes(stage) ? t(`deleted.stage.${stage}`) : t("common.scanning"));
  const progress = useApp((s) => s.progress.scan);

  const start = async (source: SourceSpec) => {
    setBusy(true);
    try {
      if (scan) void api.scanForget(scan.scan).catch(() => {});
      setScan(null);
      setScan(await api.scanStart(source));
    } catch (e) {
      toast("danger", errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4">
      <PageHeader title={t("deleted.title")} subtitle={t("deleted.subtitle")} />
      <SourcePicker onScan={(s) => void start(s)} busy={busy} />
      {busy && (
        <ProgressPanel
          kind="scan"
          label={stageLabel(progress?.stage ?? "")}
          detail={(p) => t("deleted.progress", { done: formatCount(p.done, locale), total: formatCount(p.total, locale), found: formatCount(p.found, locale) })}
        />
      )}
      {scan ? (
        <Results key={scan.scan} />
      ) : (
        !busy && (
          <Card>
            <EmptyState icon={<Info size={20} strokeWidth={1.75} />} title={t("deleted.nothingYet")} />
          </Card>
        )
      )}
    </div>
  );
}
