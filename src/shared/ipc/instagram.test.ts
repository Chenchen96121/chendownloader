import { invoke } from "@tauri-apps/api/core";
import { listen, type Event, type UnlistenFn } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  InstagramAnalysisIpcError,
  InstagramDownloadIpcError,
  analyzeInstagram,
  cancelInstagramDownload,
  getInstagramDownloadResult,
  getInstagramPreview,
  listenInstagramDownloadProgress,
  openInstagramCompletedLocation,
  openInstagramSource,
  startInstagramDownload,
} from "./instagram";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const nativeInvoke = vi.mocked(invoke);
const nativeListen = vi.mocked(listen);
const capabilities = {
  postBatch: true, images: true, videos: true, compatibility: true,
  clip: false, manualQuality: false, audioOnly: false, videoOnly: false,
} as const;
const analysis = {
  analysisId: "1", sourceKind: "post", shortcode: "Abc_123-", title: "Post",
  sourcePageUrl: "https://www.instagram.com/p/Abc_123-/", capabilities,
  items: [{
    itemId: "2", ordinal: 1, kind: "image", container: "jpg",
    previewAvailable: true, availability: "ready",
  }],
} as const;
const progress = {
  jobId: "7", state: "downloading", itemIndex: 1, itemCount: 1,
  downloadedBytes: 5, totalBytes: 10, percent: 50,
} as const;
const result = {
  jobId: "7", state: "completed", completedCount: 1, failedCount: 0,
  items: [{ itemId: "2", ordinal: 1, kind: "image", state: "completed", displayName: "01.jpg" }],
} as const;

beforeEach(() => {
  nativeInvoke.mockReset();
  nativeListen.mockReset();
});

describe("Instagram IPC", () => {
  it.each([
    "https://www.instagram.com/reels/Det5ABjSXVU/",
    "https://instagram.com/reels/Det5ABjSXVU?utm_source=share#clip",
    "https://www.instagram.com/reel/Det5ABjSXVU/?utm_source=ig_web_copy_link&dlrf=SYNTHETIC",
  ])("accepts the individual Reel alias and keeps native canonicalization: %s", async (url) => {
    const reel = {
      analysisId: "1", sourceKind: "reel", shortcode: "Det5ABjSXVU", title: "Reel",
      sourcePageUrl: "https://www.instagram.com/reel/Det5ABjSXVU/", capabilities,
      items: [{ itemId: "2", ordinal: 1, kind: "video", container: "mp4", previewAvailable: true, availability: "ready" }],
    };
    nativeInvoke.mockResolvedValueOnce(reel);
    await expect(analyzeInstagram(url)).resolves.toEqual(reel);
    expect(nativeInvoke.mock.calls).toEqual([["analyze_instagram", { request: { url } }]]);
  });

  it.each([
    "https://www.instagram.com/reels/",
    "https://www.instagram.com/reels/Det5ABjSXVU/embed/",
    "https://www.instagram.com/%72eels/Det5ABjSXVU/",
    "https://www.instagram.com/reels/%44et5ABjSXVU/",
    "https://instagram.com.evil.invalid/reels/Det5ABjSXVU/",
    "https://user@www.instagram.com/reels/Det5ABjSXVU/",
    "https://www.instagram.com:444/reels/Det5ABjSXVU/",
    "http://www.instagram.com/reels/Det5ABjSXVU/",
  ])("rejects feed, malformed or foreign Reel URLs before IPC: %s", async (url) => {
    await expect(analyzeInstagram(url)).rejects.toMatchObject({ code: "invalid_url" });
    expect(nativeInvoke).not.toHaveBeenCalled();
  });

  it("uses strict request wrappers and validates analysis, preview, and ID-only open", async () => {
    nativeInvoke
      .mockResolvedValueOnce(analysis)
      .mockResolvedValueOnce({ mimeType: "image/jpeg", dataBase64: "AQID" })
      .mockResolvedValueOnce(null);

    await expect(analyzeInstagram("https://www.instagram.com/p/Abc_123-/")).resolves.toEqual(analysis);
    await expect(getInstagramPreview("1", "2")).resolves.toEqual({ mimeType: "image/jpeg", dataBase64: "AQID" });
    await openInstagramSource("1");
    expect(nativeInvoke.mock.calls).toEqual([
      ["analyze_instagram", { request: { url: "https://www.instagram.com/p/Abc_123-/" } }],
      ["get_instagram_preview", { request: { analysisId: "1", itemId: "2" } }],
      ["open_instagram_source", { request: { analysisId: "1" } }],
    ]);
  });

  it("starts, cancels, reads results, and opens locations using only opaque IDs", async () => {
    nativeInvoke
      .mockResolvedValueOnce({ jobId: "7" })
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce(result)
      .mockResolvedValueOnce(null);
    const intent = { itemIds: ["2"], compatibilityMode: false };

    await expect(startInstagramDownload("1", "3", intent)).resolves.toEqual({ jobId: "7" });
    await cancelInstagramDownload("7");
    await expect(getInstagramDownloadResult("7")).resolves.toEqual(result);
    await openInstagramCompletedLocation("7");
    expect(nativeInvoke.mock.calls).toEqual([
      ["start_instagram_download", { request: { analysisId: "1", destinationId: "3", intent } }],
      ["cancel_instagram_download", { request: { jobId: "7" } }],
      ["get_instagram_download_result", { request: { jobId: "7" } }],
      ["open_instagram_completed_location", { request: { jobId: "7" } }],
    ]);
  });

  it.each([
    ["analysis", { ...analysis, signedUrl: "https://private.invalid" }],
    ["preview", { mimeType: "image/jpeg", dataBase64: "AQID", path: "C:\\private" }],
    ["start", { jobId: "7", path: "C:\\private" }],
    ["result", { ...result, absolutePath: "C:\\private" }],
  ] as const)("rejects malformed %s responses", async (kind, response) => {
    nativeInvoke.mockResolvedValueOnce(response);
    const operation = kind === "analysis"
      ? analyzeInstagram("https://www.instagram.com/p/Abc_123-/")
      : kind === "preview"
        ? getInstagramPreview("1", "2")
        : kind === "start"
          ? startInstagramDownload("1", "3", { itemIds: ["2"], compatibilityMode: false })
          : getInstagramDownloadResult("7");
    await expect(operation).rejects.toMatchObject({ code: kind === "analysis" || kind === "preview" ? "analysis_unavailable" : "unknown" });
  });

  it("rejects invalid inputs before native IPC", async () => {
    await expect(analyzeInstagram("https://example.com/")).rejects.toBeInstanceOf(InstagramAnalysisIpcError);
    await expect(startInstagramDownload("0", "3", { itemIds: ["2"], compatibilityMode: false }))
      .rejects.toBeInstanceOf(InstagramDownloadIpcError);
    await expect(cancelInstagramDownload("../7")).rejects.toBeInstanceOf(InstagramDownloadIpcError);
    expect(nativeInvoke).not.toHaveBeenCalled();
  });

  it("listens to the fixed event, rejects extra fields, and cleans up", async () => {
    let nativeHandler: ((event: Event<unknown>) => void) | undefined;
    const unlisten = vi.fn<UnlistenFn>();
    nativeListen.mockImplementationOnce(async (eventName, handler) => {
      expect(eventName).toBe("instagram-download-progress");
      nativeHandler = handler as (event: Event<unknown>) => void;
      return unlisten;
    });
    const receiver = vi.fn();
    const cleanup = await listenInstagramDownloadProgress(receiver);
    nativeHandler?.({ event: "instagram-download-progress", id: 1, payload: progress } as Event<unknown>);
    expect(receiver).toHaveBeenCalledExactlyOnceWith(progress);
    expect(() => nativeHandler?.({
      event: "instagram-download-progress", id: 2, payload: { ...progress, url: "private" },
    } as Event<unknown>)).toThrow(InstagramDownloadIpcError);
    cleanup();
    expect(unlisten).toHaveBeenCalledOnce();
  });

  it("maps only closed native errors and discards raw diagnostics", async () => {
    nativeInvoke.mockRejectedValueOnce("authentication_required");
    await expect(analyzeInstagram("https://www.instagram.com/p/Abc_123-/"))
      .rejects.toEqual(new InstagramAnalysisIpcError("authentication_required"));
    nativeInvoke.mockRejectedValueOnce(new Error("C:\\private signed URL"));
    const error = await cancelInstagramDownload("7").catch((value: unknown) => value);
    expect(error).toEqual(new InstagramDownloadIpcError("unknown"));
    expect(JSON.stringify(error)).not.toContain("private");
  });
});
