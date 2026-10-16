import { useTranslation } from "react-i18next";
import { formatBytes, formatDateTime } from "./format";
import { reasonParams } from "./states";
import type { Assessment } from "./types";

/** BCP 47 locale for Intl formatting, following the UI language. */
export function useLocale(): string {
  const { i18n } = useTranslation();
  return i18n.language === "tr" ? "tr-TR" : "en-US";
}

export function useBytes() {
  const locale = useLocale();
  return (n: number) => formatBytes(n, locale);
}

/** Translated reason sentence for an assessment. */
export function useReasonText() {
  const { t } = useTranslation();
  const locale = useLocale();
  const bytes = useBytes();
  return (a: Assessment, ext = "") => t(`reason.${a.reason.code}`, reasonParams(a.reason, { ext, date: (n) => formatDateTime(n, locale), bytes }));
}
