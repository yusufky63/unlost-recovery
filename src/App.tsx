import { AlertTriangle, MonitorX } from "lucide-react";
import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { Shell } from "./components/Shell";
import { Banner, EmptyState } from "./components/ui";
import { isDesktop, onProgress } from "./lib/api";
import { DeletedPage } from "./pages/Deleted";
import { ElsewherePage } from "./pages/Elsewhere";
import { RecycleBinPage } from "./pages/RecycleBin";
import { SettingsPage } from "./pages/Settings";
import { StartPage } from "./pages/Start";
import { VersionsPage } from "./pages/Versions";
import { useApp } from "./store/app";

function Page() {
  const page = useApp((s) => s.page);
  switch (page) {
    case "start":
      return <StartPage />;
    case "recycle":
      return <RecycleBinPage />;
    case "versions":
      return <VersionsPage />;
    case "deleted":
      return <DeletedPage />;
    case "elsewhere":
      return <ElsewherePage />;
    case "settings":
      return <SettingsPage />;
  }
}

export function App() {
  const { t } = useTranslation();
  const boot = useApp((s) => s.boot);
  const ready = useApp((s) => s.ready);
  const bootError = useApp((s) => s.bootError);

  useEffect(() => {
    void boot();
    const unlisten = onProgress((e) => useApp.getState().setProgress(e));
    return () => {
      void unlisten.then((f) => f());
    };
  }, [boot]);

  if (!isDesktop()) {
    return (
      <div className="flex h-full items-center justify-center">
        <EmptyState icon={<MonitorX size={20} strokeWidth={1.75} />} title={t("app.fullName")} body={t("common.desktopOnly")} />
      </div>
    );
  }
  if (!ready) return <div className="h-full bg-canvas" />;
  return (
    <Shell>
      {bootError && (
        <div className="mb-4">
          <Banner tone="danger" icon={<AlertTriangle size={16} strokeWidth={1.75} />}>
            {bootError}
          </Banner>
        </div>
      )}
      <Page />
    </Shell>
  );
}
