import { expect, it } from "vitest";
import { catalogs } from "./catalog";
it("offers the same complete nonempty message set in Chinese and English", () => {
  expect(Object.keys(catalogs["en-US"]).sort()).toEqual(Object.keys(catalogs["zh-CN"]).sort());
  for (const catalog of Object.values(catalogs)) {
    for (const message of Object.values(catalog)) expect(message.trim().length).toBeGreaterThan(0);
    expect(catalog.bilibiliPartLabel.replace("{part}", "2")).not.toContain("{part}");
    expect(catalog.bilibiliScopeHint).not.toContain("YouTube");
  }
});
