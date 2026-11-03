import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import { createInstagramWorkspaceState } from "./instagramState";
import { InstagramWorkspaceView } from "./InstagramWorkspace";

it("removes the Reel iframe synchronously when its workspace is inactive", () => {
  const state = createInstagramWorkspaceState();
  state.analysis = {
    analysisId: "1", sourceKind: "reel", shortcode: "Reel_1", title: "Reel",
    sourcePageUrl: "https://www.instagram.com/reel/Reel_1/",
    capabilities: { postBatch: true, images: true, videos: true, compatibility: true, clip: false, manualQuality: false, audioOnly: false, videoOnly: false },
    items: [{ itemId: "11", ordinal: 1, kind: "video", container: "mp4", previewAvailable: false, availability: "ready" }],
  };
  const html = renderToStaticMarkup(<InstagramWorkspaceView locale="zh-CN" state={state} destination={null}
    eventsReady active={false} pastePending={false} pasteError={null} reelEmbedded
    onUrlChange={() => {}} onPaste={() => {}} onAnalyze={() => {}} onToggleSelection={() => {}}
    onSelectAll={() => {}} onClearSelection={() => {}} onCompatibilityChange={() => {}}
    onRequestPreview={() => {}} onOpenReel={() => {}} onOpenSource={() => {}} onOpenSettings={() => {}}
    onStart={() => {}} onCancel={() => {}} onOpenLocation={() => {}} />);
  expect(html).not.toContain("<iframe");
  expect(html).toContain("Reel");
});

it("renders the Chinese Instagram URL workspace with paste and a fixed media/options split", () => {
  const html = renderToStaticMarkup(<InstagramWorkspaceView
    locale="zh-CN"
    state={createInstagramWorkspaceState()}
    destination={null}
    eventsReady={false}
    active
    pastePending={false}
    pasteError={null}
    reelEmbedded={false}
    onUrlChange={() => {}}
    onPaste={() => {}}
    onAnalyze={() => {}}
    onToggleSelection={() => {}}
    onSelectAll={() => {}}
    onClearSelection={() => {}}
    onCompatibilityChange={() => {}}
    onRequestPreview={() => {}}
    onOpenReel={() => {}}
    onOpenSource={() => {}}
    onOpenSettings={() => {}}
    onStart={() => {}}
    onCancel={() => {}}
    onOpenLocation={() => {}}
  />);
  expect(html).toContain("Instagram 帖子或 Reels 链接");
  expect(html).toContain("粘贴");
  expect(html).toContain("解析 Instagram");
  expect(html).toContain("instagram-layout");
  expect(html).toContain("instagram-gallery-pane");
  expect(html).toContain("instagram-download-panel");
  expect(html).not.toContain("剪切");
  expect(html).not.toContain("仅音频");
});
