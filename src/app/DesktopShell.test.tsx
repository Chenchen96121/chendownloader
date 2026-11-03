import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import { DesktopShell } from "./DesktopShell";

it("places download history after Bilibili and keeps its single panel mounted off route", () => {
  const render = (route: "history" | "youtube") => renderToStaticMarkup(<DesktopShell locale="zh-CN" route={route} onNavigate={() => {}}
    youtubeWorkspace={<p>YouTube</p>} instagramWorkspace={null} xWorkspace={null} bilibiliWorkspace={null} settings={null}
    downloads={<p data-history="retained">Task progress</p>} />);
  const history = render("history");
  expect(history).toContain("下载历史");
  expect(history.indexOf("Bilibili")).toBeLessThan(history.indexOf("下载历史"));
  expect(history).toContain('class="history-route"');
  expect(history).not.toContain('class="history-route" hidden=""');
  expect(render("youtube")).toContain('class="history-route" hidden=""');
  expect(render("youtube")).toContain('data-history="retained"');
});

it("provides five routes and keeps the workspace mounted even on Settings", () => {
  const html = renderToStaticMarkup(<DesktopShell locale="zh-CN" route="settings" onNavigate={() => {}} settings={<p>Language</p>}
    youtubeWorkspace={<p data-controller="youtube-persisted">Active download</p>}
    instagramWorkspace={<p data-controller="instagram-persisted">Instagram selection</p>}
    xWorkspace={<p data-controller="x-persisted">X selection</p>}
    bilibiliWorkspace={<p data-controller="bilibili-persisted">Bilibili selection</p>} />);
  expect(html).toContain("YouTube 视频解析");
  for (const name of ["Instagram", "X", "Bilibili", "设置"]) expect(html).toContain(name);
  expect(html).not.toContain("即将支持");
  expect(html).toContain('data-controller="youtube-persisted"');
  expect(html).toContain('data-controller="instagram-persisted"');
  expect(html).toContain('data-controller="x-persisted"');
  expect(html).toContain('class="workspace-route bilibili-route" hidden=""');
  expect(html).toContain('data-controller="bilibili-persisted"');
  expect(html).toContain('hidden=""');
  expect(html).toContain('aria-current="page"');
});
it("keeps all four workspaces mounted on any active platform", () => {
  const html = renderToStaticMarkup(<DesktopShell locale="en-US" route="instagram" onNavigate={() => {}}
    youtubeWorkspace={<p>YouTube retained</p>}
    instagramWorkspace={<button>Analyze Instagram</button>}
    xWorkspace={<button>Analyze X</button>}
    bilibiliWorkspace={<button>Analyze Bilibili</button>}
    settings={<p>Settings contents</p>} />);
  expect(html).toContain("Analyze Instagram");
  expect(html).toContain("YouTube retained");
  expect(html).toContain("Analyze X");
  expect(html).toContain("Analyze Bilibili");
  expect(html).not.toContain("Coming soon");
  expect(html).not.toContain("future-platform");
});
