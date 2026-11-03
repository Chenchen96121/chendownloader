import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { InstagramAnalysisPreview } from "../../shared/contracts/instagram";
import {
  createInstagramWorkspaceState,
  instagramWorkspaceReducer,
  type InstagramWorkspaceState,
} from "./instagramState";
import { InstagramDownloadPanel } from "./InstagramDownloadPanel";

const capabilities = {
  postBatch: true, images: true, videos: true, compatibility: true,
  clip: false, manualQuality: false, audioOnly: false, videoOnly: false,
} as const;
const analysis: InstagramAnalysisPreview = {
  analysisId: "1", sourceKind: "post", shortcode: "Post_1", title: "Post",
  sourcePageUrl: "https://www.instagram.com/p/Post_1/", capabilities,
  items: [
    { itemId: "11", ordinal: 1, kind: "image", container: "jpg", previewAvailable: true, availability: "ready" },
    { itemId: "12", ordinal: 2, kind: "video", width: 1080, height: 1920, container: "mp4", fps: 30, videoCodec: "h264", audioCodec: "aac", dynamicRange: "HDR", videoBitrateKbps: 8200, audioBitrateKbps: 128, fileSize: { bytes: 20_000_000, isApproximate: true }, previewAvailable: false, availability: "ready" },
  ],
};

function readyState(): InstagramWorkspaceState {
  let state = createInstagramWorkspaceState();
  state = instagramWorkspaceReducer(state, { type: "urlChanged", url: analysis.sourcePageUrl });
  state = instagramWorkspaceReducer(state, { type: "analysisStarted", generation: 2 });
  return instagramWorkspaceReducer(state, { type: "analysisSucceeded", generation: 2, analysis });
}

function panel(state: InstagramWorkspaceState, locale: "zh-CN" | "en-US" = "zh-CN") {
  return renderToStaticMarkup(<InstagramDownloadPanel
    locale={locale}
    state={state}
    destination={{ destinationId: "3", displayName: "Downloads" }}
    eventsReady
    onSelectAll={() => {}}
    onClearSelection={() => {}}
    onCompatibilityChange={() => {}}
    onOpenSettings={() => {}}
    onStart={() => {}}
    onCancel={() => {}}
    onOpenLocation={() => {}}
  />);
}

describe("Instagram download panel", () => {
  it("retains but disables compatibility for an image-only selection", () => {
    let state = instagramWorkspaceReducer(readyState(), { type: "compatibilityChanged", value: true });
    state = instagramWorkspaceReducer(state, { type: "selectionToggled", itemId: "12" });
    const html = panel(state);
    expect(html).toContain("已选择 1 项");
    expect(html).toContain('checked=""');
    expect(html).toContain('disabled=""');
    expect(html).toContain("仅在所选内容包含视频时生效");
    expect(html).not.toContain("剪切");
    expect(html).not.toContain("分辨率选择");
  });

  it("shows partial counts and preserves the failed item ordinal", () => {
    let state = readyState();
    state = {
      ...state,
      job: {
        jobId: "8", generation: 3, state: "partial", itemIndex: 2, itemCount: 2,
        downloadedBytes: null, totalBytes: null, percent: null, cancelRequested: false,
      },
      result: {
        jobId: "8", state: "partial", completedCount: 1, failedCount: 1,
        folderDisplayName: "creator [Post_1]",
        items: [
          { itemId: "11", ordinal: 1, kind: "image", state: "completed", displayName: "01.jpg" },
          { itemId: "12", ordinal: 2, kind: "video", state: "failed", errorCode: "content_changed" },
        ],
      },
    };
    const html = panel(state, "en-US");
    expect(html).toContain("1 completed, 1 failed");
    expect(html).toContain("Item 2");
    expect(html).toContain("The post changed");
    expect(html).toContain("Open containing folder");
  });

  it("reports how many completed items remain after cancellation", () => {
    const state: InstagramWorkspaceState = {
      ...readyState(),
      job: {
        jobId: "9", generation: 3, state: "canceled", itemIndex: 2, itemCount: 2,
        downloadedBytes: null, totalBytes: null, percent: null, cancelRequested: true,
      },
      result: {
        jobId: "9", state: "canceled", completedCount: 1, failedCount: 0,
        items: [
          { itemId: "11", ordinal: 1, kind: "image", state: "completed", displayName: "01.jpg" },
          { itemId: "12", ordinal: 2, kind: "video", state: "canceled" },
        ],
      },
    };

    expect(panel(state)).toContain("已取消，已保存 1 项");
    expect(panel(state, "en-US")).toContain("Canceled; 1 item(s) saved");
  });

  it("shows the selected video's common source details without quality or audio choices", () => {
    const html = panel(readyState(), "en-US");
    expect(html).toContain("1080 × 1920 · MP4");
    expect(html).toContain("30 FPS");
    expect(html).toContain("Video codec: H264");
    expect(html).toContain("Audio codec: AAC");
    expect(html).toContain("HDR");
    expect(html).toContain("Video bitrate: 8200 kbps");
    expect(html).toContain("Audio bitrate: 128 kbps");
    expect(html).not.toContain("Choose formats");
    expect(html).not.toContain("Audio only");
  });
});
