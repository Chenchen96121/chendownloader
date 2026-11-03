import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import { DefaultDestinationSettingsView } from "./DefaultDestinationSettings";
it("places the folder picker in Settings with only a safe display label", () => {
  const html = renderToStaticMarkup(<DefaultDestinationSettingsView locale="zh-CN" status={{state: "ready", displayName: "Downloads"}} busy={false} failed={false} onChoose={() => {}} />);
  expect(html).toContain("默认下载目录"); expect(html).toContain("Downloads"); expect(html).toContain("选择文件夹"); expect(html).not.toContain("C:");
});
it("keeps the picker available for missing or unavailable saved settings", () => {
  const html = renderToStaticMarkup(<DefaultDestinationSettingsView locale="en-US" status={{state: "unavailable"}} busy={false} failed={false} onChoose={() => {}} />);
  expect(html).toContain("Choose folder"); expect(html).toContain("unavailable"); expect(html).not.toContain('disabled=""');
});
