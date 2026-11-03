import { describe, expect, it } from "vitest";
import { engineStatusLabel, toolStateLabel } from "./toolStatus";

describe("toolStateLabel", () => {
  it("uses the approved labels for managed-tool states", () => {
    expect(toolStateLabel("ready", "zh-CN")).toBe("已就绪");
    expect(toolStateLabel("missing", "zh-CN")).toBe("缺失");
    expect(toolStateLabel("integrity_failed", "zh-CN")).toBe("需要修复");
    expect(toolStateLabel("ready", "en-US")).toBe("Ready");
  });
});

describe("concise engine status", () => {
  it.each(["zh-CN", "en-US"] as const)("localizes readiness without runtime details in %s", (locale) => {
    const ready = { state: "ready" as const };
    expect(engineStatusLabel(ready, locale)).toBe(locale === "zh-CN" ? "本地引擎已准备" : "Local engine ready");
    expect(engineStatusLabel({ state: "failed", errorCode: "package_invalid" }, locale)).toBe(locale === "zh-CN" ? "本地引擎未准备" : "Local engine not ready");
    expect(engineStatusLabel({ state: "repairing" }, locale)).toBe(locale === "zh-CN" ? "正在修复本地组件…" : "Repairing local components…");
    expect(engineStatusLabel(undefined, locale)).toBe(locale === "zh-CN" ? "暂时无法检查本地引擎" : "Local engine status unavailable");
    expect(engineStatusLabel(null, locale)).toBe(locale === "zh-CN" ? "正在检查本地引擎…" : "Checking local engine…");
  });
});
