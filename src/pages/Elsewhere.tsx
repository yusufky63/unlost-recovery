import { Cloud, ExternalLink, FileClock, HardDriveDownload, Image, Info, Smartphone } from "lucide-react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Banner, Button, Card, PageHeader } from "../components/ui";
import { openUrl } from "../lib/api";

// Help pages checked when this list was written (see docs/research.md, section 3.1).
const LINKS = {
  onedrive: "https://support.microsoft.com/en-us/office/restore-deleted-files-or-folders-in-onedrive-949ada80-0026-4db3-a953-c99083e6a84f",
  gdrive: "https://support.google.com/drive/answer/2375102",
  gphotos: "https://support.google.com/photos/answer/6128858",
};

function Place({ icon, title, body, url }: { icon: ReactNode; title: string; body: string; url?: string }) {
  const { t } = useTranslation();
  return (
    <div className="flex h-full flex-col gap-2 p-4 md:p-5">
      <div className="flex items-center gap-2.5">
        <span className="text-primary">{icon}</span>
        <h2 className="display-medium text-[16px] text-ink">{title}</h2>
      </div>
      <p className="selectable flex-1 text-[14px] text-ink-secondary">{body}</p>
      {url && (
        <div>
          <Button size="sm" variant="ghost" icon={<ExternalLink size={15} strokeWidth={1.75} />} onClick={() => void openUrl(url)}>
            {t("elsewhere.help")}
          </Button>
        </div>
      )}
    </div>
  );
}

export function ElsewherePage() {
  const { t } = useTranslation();
  const s = { size: 18, strokeWidth: 1.75 };
  return (
    <div>
      <PageHeader title={t("elsewhere.title")} subtitle={t("elsewhere.subtitle")} />
      <div className="mb-4">
        <Banner icon={<Info size={16} strokeWidth={1.75} />}>{t("elsewhere.cloudPlaceholder")}</Banner>
      </div>
      <div className="module-grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3">
        <Place icon={<Cloud {...s} />} title={t("elsewhere.onedrive")} body={t("elsewhere.onedriveBody")} url={LINKS.onedrive} />
        <Place icon={<Cloud {...s} />} title={t("elsewhere.gdrive")} body={t("elsewhere.gdriveBody")} url={LINKS.gdrive} />
        <Place icon={<Image {...s} />} title={t("elsewhere.gphotos")} body={t("elsewhere.gphotosBody")} url={LINKS.gphotos} />
        <Place icon={<Cloud {...s} />} title={t("elsewhere.dropbox")} body={t("elsewhere.dropboxBody")} />
        <Place icon={<HardDriveDownload {...s} />} title={t("elsewhere.fileHistory")} body={t("elsewhere.fileHistoryBody")} />
        <Place icon={<FileClock {...s} />} title={t("elsewhere.office")} body={t("elsewhere.officeBody")} />
      </div>
      <Card className="mt-4">
        <Place icon={<Smartphone {...s} />} title={t("elsewhere.phone")} body={t("elsewhere.phoneBody")} />
      </Card>
    </div>
  );
}
