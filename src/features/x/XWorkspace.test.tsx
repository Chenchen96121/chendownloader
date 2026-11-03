import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import { XWorkspaceView, pendingXPreviewIds } from "./XWorkspace";
import { createXWorkspaceState } from "./xState";
import { readyXState } from "./xTestFixtures";
it("does not request previews from hidden X and bounds active cover requests to two", () => {
  const state=readyXState();
  expect(pendingXPreviewIds(state,false)).toEqual([]);
  expect(pendingXPreviewIds(state,true)).toEqual(["11","13"]);
  state.previews["11"]={status:"loading",requestId:1};
  expect(pendingXPreviewIds(state,true)).toEqual(["13"]);
  state.previews["13"]={status:"loading",requestId:2};
  expect(pendingXPreviewIds(state,true)).toEqual([]);
  state.previews["11"]={status:"error",requestId:1,errorCode:"network_failed"};
  expect(pendingXPreviewIds(state,true)).toEqual(["14"]);
});
it("keeps Chinese X input/paste and center-preview/right-options layout without Reel embed", () => {
  const html=renderToStaticMarkup(<XWorkspaceView locale="zh-CN" state={createXWorkspaceState()} destination={null} eventsReady={false} active
    pastePending={false} pasteError={null} onUrlChange={() => {}} onPaste={() => {}} onAnalyze={() => {}}
    onToggleSelection={() => {}} onSelectAll={() => {}} onClearSelection={() => {}} onCompatibilityChange={() => {}}
    onRequestPreview={() => {}} onOpenSource={() => {}} onOpenSettings={() => {}} onStart={() => {}} onCancel={() => {}} onOpenLocation={() => {}} />);
  for (const text of ["X 帖子链接","解析 X","粘贴","x-layout","x-gallery-pane","x-download-panel"]) expect(html).toContain(text);
  expect(html).not.toMatch(/Reels|<iframe|剪切|仅音频|分辨率选择/);
});
