import { ShieldCheck } from "lucide-react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Card, CardHeader, PageHeader, Segmented } from "../components/ui";
import { useApp } from "../store/app";

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-3 px-4 py-3">
      <span className="text-[14px] text-ink">{label}</span>
      {children}
    </div>
  );
}

export function SettingsPage() {
  const { t } = useTranslation();
  const settings = useApp((s) => s.settings);
  const info = useApp((s) => s.info);
  const save = useApp((s) => s.saveSettings);
  if (!settings) return null;
  const rules = ["rule1", "rule2", "rule3", "rule4", "rule5", "rule6"] as const;
  return (
    <div className="space-y-4">
      <PageHeader title={t("settings.title")} />
      <Card>
        <div className="divide-y divide-line">
          <Row label={t("settings.language")}>
            <Segmented
              value={settings.language}
              onChange={(language) => void save({ language })}
              options={[
                { value: "system", label: t("settings.languageSystem") },
                { value: "en", label: "English" },
                { value: "tr", label: "Türkçe" },
              ]}
            />
          </Row>
          <Row label={t("settings.theme")}>
            <Segmented
              value={settings.theme}
              onChange={(theme) => void save({ theme })}
              options={[
                { value: "system", label: t("settings.themeSystem") },
                { value: "light", label: t("settings.themeLight") },
                { value: "dark", label: t("settings.themeDark") },
              ]}
            />
          </Row>
        </div>
      </Card>
      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader eyebrow={t("settings.rulesTitle")} title={t("app.tagline")} />
          <ul className="space-y-2 p-4">
            {rules.map((r) => (
              <li key={r} className="flex items-start gap-2 text-[14px] text-ink-secondary">
                <ShieldCheck size={16} strokeWidth={1.75} className="mt-0.5 shrink-0 text-positive-fg" />
                <span>{t(`settings.${r}`)}</span>
              </li>
            ))}
          </ul>
        </Card>
        <Card>
          <CardHeader eyebrow={t("settings.about")} title={t("settings.version", { version: info?.version ?? "" })} hint={t("settings.license")} />
          <dl className="divide-y divide-line text-[13px]">
            <div className="flex flex-wrap justify-between gap-2 px-4 py-2.5">
              <dt className="text-ink-muted">{t("settings.mode")}</dt>
              <dd className="text-ink">{info?.portable ? t("nav.portable") : t("settings.installed")}</dd>
            </div>
            <div className="flex flex-wrap justify-between gap-2 px-4 py-2.5">
              <dt className="text-ink-muted">{t("nav.admin")}</dt>
              <dd className="text-ink">{info?.adminReady ? t("admin.granted") : t("nav.standard")}</dd>
            </div>
            <div className="px-4 py-2.5">
              <dt className="text-ink-muted">{t("settings.dataDir")}</dt>
              <dd className="selectable mt-0.5 break-all font-mono text-[12px] text-ink">{info?.dataDir}</dd>
            </div>
          </dl>
          <div className="border-t border-line px-4 py-3">
            <div className="label mb-1">{t("settings.privacyTitle")}</div>
            <p className="text-[13px] text-ink-secondary">{t("settings.privacy")}</p>
          </div>
        </Card>
      </div>
    </div>
  );
}
