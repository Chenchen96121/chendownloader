import type { Catalog } from "../../shared/i18n/catalog";
import type { Locale } from "../../shared/i18n/locale";

interface LocaleSettingsProps {
  locale: Locale;
  copy: Pick<Catalog, "languageLabel" | "languageAriaLabel" | "languageChinese" | "languageEnglish">;
  onLocaleChange: (locale: Locale) => void;
}

export function LocaleSettings({ locale, copy, onLocaleChange }: LocaleSettingsProps) {
  return (
    <section className="locale-setting">
      <h3>{copy.languageLabel}</h3>
      <fieldset className="language-options" aria-label={copy.languageAriaLabel}>
        <label lang="zh-CN"><input type="radio" name="locale" checked={locale === "zh-CN"} onChange={() => onLocaleChange("zh-CN")} />{copy.languageChinese}</label>
        <label lang="en-US"><input type="radio" name="locale" checked={locale === "en-US"} onChange={() => onLocaleChange("en-US")} />{copy.languageEnglish}</label>
      </fieldset>
    </section>
  );
}
