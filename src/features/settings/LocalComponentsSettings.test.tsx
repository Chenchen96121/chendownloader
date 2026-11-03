import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { catalogs } from "../../shared/i18n/catalog";
import type { ToolInventory } from "../../shared/contracts/managedTools";
import { LocalComponentsSettings } from "./LocalComponentsSettings";

const inventory: ToolInventory = {
  target: "PRIVATE TARGET",
  tools: [
    { name: "yt-dlp", state: "ready", version: "2026.08.19" },
    { name: "deno", state: "ready", version: "2.9.7" },
    { name: "ffmpeg", state: "ready", version: "9.0.2" },
    { name: "ffprobe", state: "ready", version: "9.0.2" },
  ],
};

describe("LocalComponentsSettings", () => {
  it.each(["zh-CN", "en-US"] as const)("does not show an endless inventory read after startup failure in %s", (locale) => {
    const html = renderToStaticMarkup(
      <LocalComponentsSettings
        copy={catalogs[locale]}
        status={{ state: "failed", errorCode: "package_missing" }}
        inventory={null}
        repairPending={false}
        onRepair={() => {}}
      />,
    );
    expect(html).toContain(catalogs[locale].componentsUnavailable);
    expect(html).not.toContain(catalogs[locale].componentsLoading);
  });
  it.each(["zh-CN", "en-US"] as const)("shows concise versions and one repair action in %s", (locale) => {
    const html = renderToStaticMarkup(
      <LocalComponentsSettings
        copy={catalogs[locale]}
        status={{ state: "ready" }}
        inventory={inventory}
        repairPending={false}
        onRepair={() => {}}
      />,
    );
    for (const tool of inventory.tools) {
      expect(html).toContain(tool.name);
      expect(html).toContain(tool.version);
    }
    expect(html).toContain(catalogs[locale].verifyAndRepair);
    expect((html.match(/<button/g) ?? [])).toHaveLength(1);
    expect(html).not.toContain("PRIVATE TARGET");
    expect(html).not.toMatch(/sha|hash|path|manifest|update/i);
  });

  it("disables duplicate repair while repairing", () => {
    const onRepair = vi.fn();
    const html = renderToStaticMarkup(
      <LocalComponentsSettings
        copy={catalogs["zh-CN"]}
        status={{ state: "repairing" }}
        inventory={inventory}
        repairPending={true}
        onRepair={onRepair}
      />,
    );
    expect(html).toMatch(/<button[^>]*disabled=""/);
    expect(html).toContain(catalogs["zh-CN"].repairingComponents);
    expect(onRepair).not.toHaveBeenCalled();
  });
});
