import { invoke } from "@tauri-apps/api/core";
import { listen, type Event, type UnlistenFn } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  XAnalysisIpcError,
  XDownloadIpcError,
  analyzeX,
  cancelXDownload,
  getXDownloadResult,
  getXPreview,
  listenXDownloadProgress,
  openXCompletedLocation,
  openXSource,
  startXDownload,
} from "./x";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const nativeInvoke = vi.mocked(invoke);
const nativeListen = vi.mocked(listen);
const capabilities = {
  postBatch: true, images: true, videos: true, compatibility: true,
  clip: false, manualQuality: false, audioOnly: false, videoOnly: false,
} as const;
const analysis = {
  analysisId: "1", postId: "12345678901234567890", title: "Post",
  sourcePageUrl: "https://x.com/i/web/status/12345678901234567890", capabilities,
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
  jobId: "7", state: "completed", postItemCount: 1, unavailableCount: 0, contentChanged: false,
  completedCount: 1, failedCount: 0, folderDisplayName: "X post",
  items: [{ itemId: "2", ordinal: 1, kind: "image", state: "completed", displayName: "01.jpg" }],
} as const;

beforeEach(() => {
  nativeInvoke.mockReset();
  nativeListen.mockReset();
});

describe("X IPC", () => {
  it("uses strict request wrappers and validates analysis, preview, and ID-only open", async () => {
    nativeInvoke
      .mockResolvedValueOnce(analysis)
      .mockResolvedValueOnce({ mimeType: "image/jpeg", dataBase64: "AQID" })
      .mockResolvedValueOnce(null);

    await expect(analyzeX("https://x.com/i/web/status/12345678901234567890")).resolves.toEqual(analysis);
    await expect(getXPreview("1", "2")).resolves.toEqual({ mimeType: "image/jpeg", dataBase64: "AQID" });
    await openXSource("1");
    expect(nativeInvoke.mock.calls).toEqual([
      ["analyze_x", { request: { url: "https://x.com/i/web/status/12345678901234567890" } }],
      ["get_x_preview", { request: { analysisId: "1", itemId: "2" } }],
      ["open_x_source", { request: { analysisId: "1" } }],
    ]);
  });

  it("starts, cancels, reads results, and opens locations using only opaque IDs", async () => {
    nativeInvoke
      .mockResolvedValueOnce({ jobId: "7" })
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce(result)
      .mockResolvedValueOnce(null);
    const intent = { itemIds: ["2"], compatibilityMode: false };

    await expect(startXDownload("1", "3", intent)).resolves.toEqual({ jobId: "7" });
    await cancelXDownload("7");
    await expect(getXDownloadResult("7")).resolves.toEqual(result);
    await openXCompletedLocation("7");
    expect(nativeInvoke.mock.calls).toEqual([
      ["start_x_download", { request: { analysisId: "1", destinationId: "3", intent } }],
      ["cancel_x_download", { request: { jobId: "7" } }],
      ["get_x_download_result", { request: { jobId: "7" } }],
      ["open_x_completed_location", { request: { jobId: "7" } }],
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
      ? analyzeX("https://x.com/i/web/status/12345678901234567890")
      : kind === "preview"
        ? getXPreview("1", "2")
        : kind === "start"
          ? startXDownload("1", "3", { itemIds: ["2"], compatibilityMode: false })
          : getXDownloadResult("7");
    await expect(operation).rejects.toMatchObject({ code: kind === "analysis" || kind === "preview" ? "analysis_unavailable" : "unknown" });
  });

  it("rejects invalid inputs before native IPC", async () => {
    await expect(analyzeX("https://example.com/")).rejects.toBeInstanceOf(XAnalysisIpcError);
    await expect(startXDownload("0", "3", { itemIds: ["2"], compatibilityMode: false }))
      .rejects.toBeInstanceOf(XDownloadIpcError);
    await expect(cancelXDownload("../7")).rejects.toBeInstanceOf(XDownloadIpcError);
    expect(nativeInvoke).not.toHaveBeenCalled();
  });

  it.each([
    "https://x.com:443/a/status/123", "https://@x.com/a/status/123",
    "https://x.com/a/../a/status/123", "https://x.com/a//status/123",
    "https://x.com/a/status/123/photo/0", "https://x.com/a/status/123/video/33",
    "https://x.com/home", "https://x.com/a/status/123/analytics",
    "https://x.com.evil.invalid/a/status/123", "https:x.com/a/status/123",
    "https://www.instagram.com/p/Abc_123-/",
  ])("rejects non-post or ambiguous URL syntax: %s", async (url) => {
    await expect(analyzeX(url)).rejects.toMatchObject({ code: "invalid_url" });
    expect(nativeInvoke).not.toHaveBeenCalled();
  });

  it.each([
    "https://twitter.com/example/status/123/video/2?s=20",
    "https://www.x.com/i/web/status/123/photo/1",
    "https://www.twitter.com/a/status/123/",
  ])("accepts canonicalizable public post aliases: %s", async (url) => {
    nativeInvoke.mockResolvedValueOnce(analysis);
    await expect(analyzeX(url)).resolves.toEqual(analysis);
    expect(nativeInvoke).toHaveBeenCalledExactlyOnceWith("analyze_x", { request: { url } });
  });

  it("does not mistake Instagram analysis or results for X data", async () => {
    nativeInvoke.mockResolvedValueOnce({ ...analysis, postId: undefined, shortcode: "abc", sourceKind: "post" });
    await expect(analyzeX("https://x.com/a/status/123")).rejects.toMatchObject({ code: "analysis_unavailable" });
    const { postItemCount: _p, unavailableCount: _u, contentChanged: _c, ...instagramResult } = result;
    nativeInvoke.mockResolvedValueOnce(instagramResult);
    await expect(getXDownloadResult("7")).rejects.toMatchObject({ code: "unknown" });
  });

  it("listens to the fixed event, rejects extra fields, and cleans up", async () => {
    let nativeHandler: ((event: Event<unknown>) => void) | undefined;
    const unlisten = vi.fn<UnlistenFn>();
    nativeListen.mockImplementationOnce(async (eventName, handler) => {
      expect(eventName).toBe("x-download-progress");
      nativeHandler = handler as (event: Event<unknown>) => void;
      return unlisten;
    });
    const receiver = vi.fn();
    const cleanup = await listenXDownloadProgress(receiver);
    nativeHandler?.({ event: "x-download-progress", id: 1, payload: progress } as Event<unknown>);
    expect(receiver).toHaveBeenCalledExactlyOnceWith(progress);
    expect(() => nativeHandler?.({
      event: "instagram-download-progress", id: 1, payload: progress,
    } as Event<unknown>)).toThrow(XDownloadIpcError);
    expect(() => nativeHandler?.({
      event: "x-download-progress", id: 2, payload: { ...progress, url: "private" },
    } as Event<unknown>)).toThrow(XDownloadIpcError);
    cleanup();
    expect(unlisten).toHaveBeenCalledOnce();
  });

  it("maps only closed native errors and discards raw diagnostics", async () => {
    nativeInvoke.mockRejectedValueOnce("authentication_required");
    await expect(analyzeX("https://x.com/i/web/status/12345678901234567890"))
      .rejects.toEqual(new XAnalysisIpcError("authentication_required"));
    nativeInvoke.mockRejectedValueOnce(new Error("C:\\private signed URL"));
    const error = await cancelXDownload("7").catch((value: unknown) => value);
    expect(error).toEqual(new XDownloadIpcError("unknown"));
    expect(JSON.stringify(error)).not.toContain("private");
  });
});
