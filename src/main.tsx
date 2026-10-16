import "@fontsource-variable/dm-sans";
import "@fontsource-variable/space-grotesk";
import "@fontsource-variable/jetbrains-mono";
import "./styles/globals.css";
import "./i18n";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { setLanguage } from "./i18n";
import { api } from "./lib/api";
import { useApp } from "./store/app";

// The WebView's own context menu (Reload, Inspect) is noise in a desktop app; text fields keep
// theirs so copy and paste still work.
if (import.meta.env.PROD) {
  document.addEventListener("contextmenu", (e) => {
    const el = e.target as HTMLElement | null;
    if (!el?.closest("input, textarea, .selectable")) e.preventDefault();
  });
}

// Dev builds expose the store so scripts/screenshots.mjs can drive the app.
if (import.meta.env.DEV) {
  const w = window as unknown as Record<string, unknown>;
  w.__unlost = useApp;
  w.__unlostSetLang = setLanguage;
  w.__unlostApi = api;
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
