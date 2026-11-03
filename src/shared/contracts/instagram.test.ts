import { describe, expect, it } from "vitest";
import { downloadErrorCodes } from "./download";
import {
  instagramJobStates,
  isInstagramAnalysisPreview,
  isInstagramDownloadIntent,
  isInstagramDownloadProgress,
  isInstagramDownloadResult,
  isInstagramPreviewPayload,
} from "./instagram";
import { analysisErrorCodes } from "../ipc/liveAnalysis";

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

function imageItem(ordinal: number) {
  return {
    itemId: String(ordinal), ordinal, kind: "image", width: 1080, height: 1350,
    container: "jpg", fileSize: { bytes: 1234, isApproximate: false },
    previewAvailable: true, availability: "ready",
  };
}

function unavailableVideoItem(ordinal: number) {
  return {
    itemId: String(ordinal), ordinal, kind: "video", width: 1080, height: 1920,
    container: "mp4", fps: 30, videoBitrateKbps: 12_000, audioBitrateKbps: 160,
    videoCodec: "h264", audioCodec: "aac", dynamicRange: "HDR",
    previewAvailable: false, availability: "unavailable", unavailableReason: "content_changed",
  };
}

function postPreview(items: unknown[]) {
  return {
    analysisId: "1", sourceKind: "post", shortcode: "Abc_123-", title: "公开帖子",
    creator: "creator_name", sourcePageUrl: "https://www.instagram.com/p/Abc_123-/",
    capabilities, items,
  };
}

describe("Instagram public contracts", () => {
  it("accepts one to thirty-two ordered items and preserves unknown size as absent", () => {
    expect(isInstagramAnalysisPreview(postPreview(Array.from({ length: 32 }, (_, index) => imageItem(index + 1))))).toBe(true);
    const unknownSize = postPreview([{
      itemId: "1", ordinal: 1, kind: "image", container: "webp",
      previewAvailable: false, availability: "ready",
    }]);
    expect(isInstagramAnalysisPreview(unknownSize)).toBe(true);
    expect(Object.hasOwn((unknownSize.items[0] as object), "fileSize")).toBe(false);
    expect(isInstagramAnalysisPreview(postPreview([unavailableVideoItem(1)]))).toBe(true);
  });

  it("rejects oversized, unordered, duplicate and inconsistent item sets", () => {
    expect(isInstagramAnalysisPreview(postPreview([]))).toBe(false);
    expect(isInstagramAnalysisPreview(postPreview(Array.from({ length: 33 }, (_, index) => imageItem(index + 1))))).toBe(false);
    expect(isInstagramAnalysisPreview(postPreview([{ ...imageItem(1), ordinal: 2 }]))).toBe(false);
    expect(isInstagramAnalysisPreview(postPreview([imageItem(1), { ...imageItem(2), itemId: "1" }]))).toBe(false);
    expect(isInstagramAnalysisPreview(postPreview([{ ...imageItem(1), unavailableReason: "content_changed" }]))).toBe(false);
    const { unavailableReason: _reason, ...missingReason } = unavailableVideoItem(1);
    expect(isInstagramAnalysisPreview(postPreview([missingReason]))).toBe(false);
    expect(isInstagramAnalysisPreview(postPreview([{ ...imageItem(1), dynamicRange: "HDR" }]))).toBe(false);
    for (const invalid of [
      { videoBitrateKbps: 0 }, { audioBitrateKbps: 100_000.1 }, { dynamicRange: "HDR99" },
    ]) expect(isInstagramAnalysisPreview(postPreview([{ ...unavailableVideoItem(1), ...invalid }]))).toBe(false);
  });

  it("requires exact capabilities, canonical source pages and a closed field set", () => {
    for (const key of Object.keys(capabilities) as (keyof typeof capabilities)[]) {
      expect(isInstagramAnalysisPreview({
        ...postPreview([imageItem(1)]),
        capabilities: { ...capabilities, [key]: !capabilities[key] },
      })).toBe(false);
    }
    for (const sourcePageUrl of [
      "https://www.instagram.com/p/Abc_123-/?token=secret",
      "https://cdninstagram.invalid/signed.jpg",
      "C:\\private\\preview.jpg",
    ]) {
      expect(isInstagramAnalysisPreview({ ...postPreview([imageItem(1)]), sourcePageUrl })).toBe(false);
    }
    for (const injected of [
      { rawMediaUrl: "https://cdn.invalid/signed" },
      { cookie: "session=private" },
      { absolutePath: "C:\\private\\file.jpg" },
    ]) {
      expect(isInstagramAnalysisPreview({ ...postPreview([imageItem(1)]), ...injected })).toBe(false);
    }
  });

  it("accepts a canonical single-video Reel and rejects post-shaped Reel data", () => {
    expect(isInstagramAnalysisPreview({
      analysisId: "2", sourceKind: "reel", shortcode: "Reel_1", title: "Reel",
      sourcePageUrl: "https://www.instagram.com/reel/Reel_1/", capabilities,
      items: [{ itemId: "1", ordinal: 1, kind: "video", container: "mp4", previewAvailable: true, availability: "ready" }],
    })).toBe(true);
    expect(isInstagramAnalysisPreview({
      ...postPreview([imageItem(1)]), sourceKind: "reel",
      sourcePageUrl: "https://www.instagram.com/reel/Abc_123-/",
    })).toBe(false);
  });

  it("accepts only a closed nonempty unique download intent of at most 32 items", () => {
    expect(isInstagramDownloadIntent({
      itemIds: Array.from({ length: 32 }, (_, index) => String(index + 1)), compatibilityMode: true,
    })).toBe(true);
    for (const value of [
      { itemIds: [], compatibilityMode: false },
      { itemIds: ["1", "1"], compatibilityMode: false },
      { itemIds: Array.from({ length: 33 }, (_, index) => String(index + 1)), compatibilityMode: false },
      { itemIds: ["https://cdn.invalid/signed"], compatibilityMode: false },
      { itemIds: ["1"], compatibilityMode: false, sourceUrl: "https://cdn.invalid" },
      { itemIds: ["1"], compatibilityMode: false, formatId: "best" },
      { itemIds: ["1"], compatibilityMode: false, path: "C:\\private" },
    ]) expect(isInstagramDownloadIntent(value)).toBe(false);
  });

  it("accepts only bounded inline JPEG, PNG, WebP or AVIF preview data", () => {
    for (const mimeType of ["image/jpeg", "image/png", "image/webp", "image/avif"]) {
      expect(isInstagramPreviewPayload({ mimeType, dataBase64: "AQID" })).toBe(true);
    }
    expect(isInstagramPreviewPayload({ mimeType: "image/gif", dataBase64: "AQID" })).toBe(false);
    expect(isInstagramPreviewPayload({ mimeType: "image/jpeg", dataBase64: "https://cdn.invalid/a.jpg" })).toBe(false);
    expect(isInstagramPreviewPayload({ mimeType: "image/jpeg", dataBase64: "AQID", path: "C:\\private" })).toBe(false);
  });

  it("keeps job, progress and result states closed and internally consistent", () => {
    expect(instagramJobStates).toEqual([
      "queued", "downloading", "processing", "verifying", "publishing",
      "completed", "partial", "failed", "canceled",
    ]);
    expect(isInstagramDownloadProgress({
      jobId: "1", state: "downloading", itemIndex: 1, itemCount: 2,
      downloadedBytes: 5, totalBytes: 10, percent: 50,
    })).toBe(true);
    expect(isInstagramDownloadProgress({
      jobId: "1", state: "downloading", itemIndex: 3, itemCount: 2,
      downloadedBytes: null, totalBytes: null, percent: 50,
    })).toBe(false);

    const result = {
      jobId: "1", state: "partial", completedCount: 1, failedCount: 1,
      folderDisplayName: "creator - Abc_123-",
      items: [
        { itemId: "1", ordinal: 1, kind: "image", state: "completed", displayName: "01.jpg" },
        { itemId: "2", ordinal: 2, kind: "video", state: "failed", errorCode: "content_changed" },
      ],
    };
    expect(isInstagramDownloadResult(result)).toBe(true);
    expect(isInstagramDownloadResult({ ...result, completedCount: 2 })).toBe(false);
    expect(isInstagramDownloadResult({
      ...result, items: [{ ...result.items[0], displayName: "C:\\private\\01.jpg" }, result.items[1]],
    })).toBe(false);

    const selectedSubset = {
      ...result,
      items: [result.items[0], { ...result.items[1], itemId: "3", ordinal: 3 }],
    };
    expect(isInstagramDownloadResult(selectedSubset)).toBe(true);
    expect(isInstagramDownloadResult({
      ...selectedSubset,
      items: [selectedSubset.items[1], selectedSubset.items[0]],
    })).toBe(false);
  });

  it("publishes content_changed in both stable error sets", () => {
    expect(analysisErrorCodes).toContain("content_changed");
    expect(downloadErrorCodes).toContain("content_changed");
  });
});
