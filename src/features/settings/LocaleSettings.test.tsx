import { Children, isValidElement, type ChangeEvent, type InputHTMLAttributes, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { catalogs } from "../../shared/i18n/catalog";
import { writeLocale, type Locale } from "../../shared/i18n/locale";
import { LocaleSettings } from "./LocaleSettings";

describe("LocaleSettings", () => {
  it.each(["zh-CN", "en-US"] as const)("renders labeled native control in %s with exactly two native-name options", (locale) => {
    const copy = catalogs[locale];
    const html = renderToStaticMarkup(<LocaleSettings locale={locale} copy={copy} onLocaleChange={() => {}} />);
    expect(html).toContain("<label");
    expect(html).toContain(copy.languageLabel);
    expect(html).toContain(`<fieldset class="language-options" aria-label="${copy.languageAriaLabel}"`);
    expect(html.match(/type="radio"/g)).toHaveLength(2);
    expect(html).toContain("简体中文"); expect(html).toContain("English");
    expect(html).toMatch(new RegExp(`<label lang="${locale}"><input[^>]*checked=""`));
  });
  it("forwards only allowed choices and reflects caller state even when persistence is refused", () => {
    let locale: Locale = "zh-CN";
    const changes: Locale[] = [];
    const onLocaleChange = (next: Locale) => {
      locale = next;
      changes.push(next);
      writeLocale(next, { setItem: () => { throw new Error("Denied"); } });
    };
    const control = LocaleSettings({ locale, copy: catalogs[locale], onLocaleChange });
    const fieldset = Children.toArray(control.props.children).find((child) => isValidElement(child) && child.type === "fieldset");
    if (!isValidElement<{children: ReactNode}>(fieldset)) throw new Error("Expected language fieldset");
    const english = Children.toArray(fieldset.props.children).find((child) => isValidElement<{lang: string}>(child) && child.props.lang === "en-US");
    if (!isValidElement<{children: ReactNode}>(english)) throw new Error("Expected English label");
    const radio = Children.toArray(english.props.children).find((child) => isValidElement(child) && child.type === "input");
    if (!isValidElement<InputHTMLAttributes<HTMLInputElement>>(radio)) throw new Error("Expected native radio");
    radio.props.onChange?.({ target: { checked: true } } as ChangeEvent<HTMLInputElement>);
    expect(changes).toEqual(["en-US"]);
    // The original control keeps its prop value until its owner supplies new state.
    expect(radio.props.checked).toBe(false);
    const updated = renderToStaticMarkup(<LocaleSettings locale={locale} copy={catalogs[locale]} onLocaleChange={onLocaleChange} />);
    expect(updated).toMatch(/<label lang="en-US"><input[^>]*checked=""/);
    expect(updated).toContain("Choose interface language");
  });
});
