import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import { en } from "./en";
import { tr } from "./tr";

export type Lang = "en" | "tr";

export function resolveLang(pref: string | undefined): Lang {
  if (pref === "en" || pref === "tr") return pref;
  const nav = typeof navigator !== "undefined" ? navigator.language : "en";
  return nav.toLowerCase().startsWith("tr") ? "tr" : "en";
}

function cachedLang(): string | undefined {
  try {
    return localStorage.getItem("unlost:lang") ?? undefined;
  } catch {
    return undefined;
  }
}

void i18n.use(initReactI18next).init({
  resources: { en: { translation: en }, tr: { translation: tr } },
  lng: resolveLang(cachedLang()),
  fallbackLng: "en",
  interpolation: { escapeValue: false },
  returnNull: false,
});

export function setLanguage(pref: string): Lang {
  const lang = resolveLang(pref);
  try {
    localStorage.setItem("unlost:lang", pref);
  } catch {
    // Storage can be unavailable; the language still applies for this session.
  }
  document.documentElement.lang = lang;
  void i18n.changeLanguage(lang);
  return lang;
}

export default i18n;
