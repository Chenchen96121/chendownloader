export type Locale = "zh-CN" | "en-US";
export const LOCALE_STORAGE_KEY = "local-media-downloader.locale";

export function readLocale(storage?: Pick<Storage, "getItem">): Locale {
  try {
    const value = (storage ?? globalThis.localStorage).getItem(LOCALE_STORAGE_KEY);
    return value === "en-US" ? "en-US" : "zh-CN";
  } catch {
    return "zh-CN";
  }
}

export function writeLocale(locale: Locale, storage?: Pick<Storage, "setItem">): void {
  if (locale !== "zh-CN" && locale !== "en-US") return;
  try {
    (storage ?? globalThis.localStorage).setItem(LOCALE_STORAGE_KEY, locale);
  } catch {
    // Persistence is optional; the caller's in-memory locale remains authoritative.
  }
}
