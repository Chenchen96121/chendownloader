import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { InstagramAnalysisPreview } from "../../shared/contracts/instagram";
import { InstagramMediaGallery } from "./InstagramMediaGallery";

const capabilities = {
  postBatch: true,
  images: true,
  videos: true,
  compatibility: true,
  clip: false,
  manualQuality: false,
  audioOnly: false,
  videoOnly: false,
} as const;

const mixedPost: InstagramAnalysisPreview = {
  analysisId: "1",
  sourceKind: "post",
  shortcode: "Post_1",
  title: "A mixed post",
  creator: "creator",
  sourcePageUrl: "https://www.instagram.com/p/Post_1/",
  capabilities,
  items: [
    {
      itemId: "11",
      ordinal: 1,
      kind: "image",
      width: 1080,
      height: 1350,
      container: "jpg",
      fileSize: { bytes: 2_000_000, isApproximate: false },
      previewAvailable: true,
      availability: "ready",
    },
    {
      itemId: "12",
      ordinal: 2,
      kind: "video",
      width: 1080,
      height: 1920,
      container: "mp4",
      fps: 30,
      videoCodec: "h264",
      audioCodec: "aac",
      dynamicRange: "HDR",
      fileSize: { bytes: 20_000_000, isApproximate: true },
      previewAvailable: false,
      availability: "ready",
    },
    {
      itemId: "13",
      ordinal: 3,
      kind: "video",
      container: "mp4",
      previewAvailable: false,
      availability: "unavailable",
      unavailableReason: "content_changed",
    },
  ],
};

describe("Instagram media gallery", () => {
  it("shows ordered common media details, selection, and unavailable items in Chinese", () => {
    const html = renderToStaticMarkup(<InstagramMediaGallery
      locale="zh-CN"
      analysis={mixedPost}
      selectedItemIds={["11", "12"]}
      previews={{
        "11": {
          status: "ready",
          requestId: 1,
          preview: { mimeType: "image/jpeg", dataBase64: "AQID" },
        },
      }}
      locked={false}
      reelEmbedded={false}
      onToggle={() => {}}
      onRequestPreview={() => {}}
      onOpenReel={() => {}}
      onOpenSource={() => {}}
    />);
    expect(html.indexOf("第 1 项")).toBeLessThan(html.indexOf("第 2 项"));
    expect(html).toContain("1080 × 1350");
    expect(html).toContain("JPG");
    expect(html).toContain("1.91 MB");
    expect(html).toContain("H.264");
    expect(html).toContain("HDR");
    const primaryFacts = [...html.matchAll(/<ul class="instagram-media-facts"[^>]*>(.*?)<\/ul>/g)].map(match => match[1]);
    expect(primaryFacts).toHaveLength(3);
    expect(primaryFacts[1]).toContain("1080 × 1920");
    expect(primaryFacts[1]).toContain("MP4");
    expect(primaryFacts[1]).not.toContain("H.264");
    expect(html).toMatch(/<details class="media-extra-details"><summary>来源信息/);
    expect(html).toMatch(/<ul class="media-extra-facts">.*?H\.264.*?HDR/);
    expect(html).toContain("帖子内容已发生变化");
    expect(html).toContain("data:image/jpeg;base64,AQID");
    expect(html).toContain('disabled=""');
  });

  it("uses only a canonical sandboxed Reel embed without navigation or download permission", () => {
    const reel: InstagramAnalysisPreview = {
      ...mixedPost,
      sourceKind: "reel",
      shortcode: "Reel_2",
      sourcePageUrl: "https://www.instagram.com/reel/Reel_2/",
      items: [{
        itemId: "21",
        ordinal: 1,
        kind: "video",
        width: 1080,
        height: 1920,
        container: "mp4",
        fps: 30,
        videoCodec: "h264",
        audioCodec: "aac",
        dynamicRange: "HDR",
        previewAvailable: false,
        availability: "ready",
      }],
    };
    const html = renderToStaticMarkup(<InstagramMediaGallery
      locale="en-US"
      analysis={reel}
      selectedItemIds={["21"]}
      previews={{}}
      locked={false}
      reelEmbedded
      onToggle={() => {}}
      onRequestPreview={() => {}}
      onOpenReel={() => {}}
      onOpenSource={() => {}}
    />);
    expect(html).toContain('src="https://www.instagram.com/reel/Reel_2/embed/"');
    expect(html).toContain('sandbox="allow-scripts allow-same-origin allow-popups"');
    expect(html).not.toContain("allow-top-navigation");
    expect(html).not.toContain("allow-downloads");
    expect(html).toContain("Open original post");
  });
});
