import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import { TikTokWorkspaceView, pendingTikTokPreviewIds } from "./TikTokWorkspace";
import { createTikTokWorkspaceState } from "./tiktokState";
import { readyTikTokState } from "./tiktokTestFixtures";
it("requests only the first preview, and only one at a time",()=>{
 const state=readyTikTokState();
 expect(pendingTikTokPreviewIds(state,false)).toEqual([]);
 expect(pendingTikTokPreviewIds(state,true)).toEqual(["11"]);
 state.previews["11"]={status:"loading",requestId:1};
 expect(pendingTikTokPreviewIds(state,true)).toEqual([]);
 state.previews["11"]={status:"error",requestId:1,errorCode:"network_failed"};
 expect(pendingTikTokPreviewIds(state,true)).toEqual([]);
});
it("keeps Chinese TikTok input/paste and center-preview/right-options layout without Reel embed", () => {
  const html=renderToStaticMarkup(<TikTokWorkspaceView locale="zh-CN" state={createTikTokWorkspaceState()} destination={null} eventsReady={false} active
    pastePending={false} pasteError={null} onUrlChange={() => {}} onPaste={() => {}} onAnalyze={() => {}}
    onToggleSelection={() => {}} onSelectAll={() => {}} onClearSelection={() => {}} onCompatibilityChange={() => {}}
    onRequestPreview={() => {}} onOpenSource={() => {}} onOpenSettings={() => {}} onStart={() => {}} onCancel={() => {}} onOpenLocation={() => {}} />);
  for (const text of ["TikTok 帖子链接","解析 TikTok","粘贴","x-layout","x-gallery-pane","x-download-panel"]) expect(html).toContain(text);
  expect(html).toContain('for="tiktok-url"');
  expect(html).toContain('type="text"');
  expect(html).not.toContain('>X<');
  expect(html).not.toMatch(/Reels|<iframe|剪切|仅音频|分辨率选择/);
});
