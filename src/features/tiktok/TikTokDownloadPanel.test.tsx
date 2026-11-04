import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import { TikTokDownloadPanel } from "./TikTokDownloadPanel";
import { readyTikTokState, selectedTikTokResult } from "./tiktokTestFixtures";
import type { TikTokWorkspaceState } from "./tiktokState";
function panel(state: TikTokWorkspaceState, locale: "zh-CN"|"en-US" = "zh-CN") {
  return renderToStaticMarkup(<TikTokDownloadPanel locale={locale} state={state} destination={{destinationId:"3",displayName:"Downloads"}} eventsReady
    onSelectAll={() => {}} onClearSelection={() => {}} onCompatibilityChange={() => {}} onOpenSettings={() => {}} onStart={() => {}} onCancel={() => {}} onOpenLocation={() => {}} />);
}
it("offers available items (N/M), not a falsely complete whole post", () => {
  const html=panel(readyTikTokState());
  expect(html).toContain("下载可用项（3/4）");
  expect(html).toContain("1 项不可用");
  expect(html).not.toMatch(/剪切|仅音频|仅视频|手动选择|分辨率选择|<select/);
});
it("hides compatibility controls for photo-only posts and binds video label to TikTok control",()=>{
 const state=readyTikTokState();
 const photo={...state,analysis:{...state.analysis!,items:[state.analysis!.items[0]!]},selectedItemIds:["11"]};
 expect(panel(photo)).not.toContain('id="tiktok-compatibility"');
 expect(panel(state)).toContain('for="tiktok-compatibility"');
});
it("keeps silent animation eligible for opt-in compatibility without promising an audio track", () => {
  const html=panel({...readyTikTokState(), selectedItemIds:["13"],compatibilityMode:true},"en-US");
  expect(html).toContain("1280 × 720 · 30 FPS");
  expect(html).toContain("30 FPS");
  expect(html).toContain("H.264 · MP4");
  expect(html).not.toContain("H.264 + AAC");
  expect(html).not.toContain('disabled=""');
});
it.each(["zh-CN","en-US"] as const)("distinguishes saved selection from whole-post completion and reports content changes in %s", locale => {
  const html=panel({...readyTikTokState(),result:{...selectedTikTokResult,contentChanged:true}},locale);
  expect(html).toContain(locale==="zh-CN"?"已保存所选 1 项":"1 selected item(s) saved");
  expect(html).toContain(locale==="zh-CN"?"原帖 4 项，其中 1 项不可用":"Original post: 4 items; 1 unavailable");
  expect(html).toContain(locale==="zh-CN"?"帖子内容在下载前已发生变化":"The post changed before download");
  expect(html).not.toContain(locale==="zh-CN"?"整帖下载完成":"Whole post completed");
});
