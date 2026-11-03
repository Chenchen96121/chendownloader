import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { SelectionIntent } from "../contracts/selection";
import { analyzeMedia, LiveAnalysisError, previewMediaSelection, openMediaPage } from "./liveAnalysis";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const nativeInvoke = vi.mocked(invoke);
const intent: SelectionIntent = { mode: "best_available" };
const plan = { compatibilityMode: false, containerSelection: "automatic" as const, videoProcessing: "copy" as const, outputVideoCodec: "h264",  mode: "best_available", sourceFormatIds: ["c1"], videoFormatId: "c1", audioFormatId: "c1", operation: "keep", outputContainer: "mp4", outputAudioCodec: "aac", audioProcessing: "copy", warnings: ["metadata_incomplete", "container_unverified", "output_size_unknown"] };
const response = {
  analysisId: "1",
  media: { platform: "youtube", mediaId: "sample", title: "Synthetic sample", formats: [{ id: "c1", container: "mp4", kind: "combined", videoCodec: "avc1", audioCodec: "aac" }] },
  rankedVideoFormatIds: ["c1"], rankedAudioFormatIds: ["c1"], outcome: { status: "selected", plan },
};

beforeEach(() => nativeInvoke.mockReset());

describe("live analysis IPC", () => {
  it("opens only the native registered analysis, never a caller-provided URL", async () => {
    nativeInvoke.mockResolvedValue(null);
    await openMediaPage("1");
    expect(nativeInvoke).toHaveBeenCalledExactlyOnceWith("open_media_page", { analysisId: "1" });
    nativeInvoke.mockReset();
    await expect(openMediaPage("https://evil.invalid")).rejects.toMatchObject({ code: "analysis_unavailable" });
    expect(nativeInvoke).not.toHaveBeenCalled();
  });
  it("sends only URL and intent to the analysis command", async () => {
    nativeInvoke.mockResolvedValue(response);
    const url = " https://youtube.com/watch?v=demoYT00001 ";
    expect((await analyzeMedia(url, intent)).analysisId).toBe("1");
    expect(nativeInvoke).toHaveBeenCalledExactlyOnceWith("analyze_media", { url, intent });
  });

  it("sends only analysis ID and intent to the preview command", async () => {
    nativeInvoke.mockResolvedValue(response);
    expect((await previewMediaSelection("1", intent)).media.mediaId).toBe("sample");
    expect(nativeInvoke).toHaveBeenCalledExactlyOnceWith("preview_media_selection", { analysisId: "1", intent });
  });

  it.each(["", "   ", "https://youtube.com/watch?v=x\n", "a".repeat(2049)])("rejects URL %j before IPC", async (url) => {
    await expect(analyzeMedia(url, intent)).rejects.toMatchObject({ message: "Media analysis unavailable", code: "invalid_url" });
    expect(nativeInvoke).not.toHaveBeenCalled();
  });

  it.each(["", "0", "01", "-1", "unknown", "1".repeat(21)])("rejects analysis ID %j before IPC", async (analysisId) => {
    await expect(previewMediaSelection(analysisId, intent)).rejects.toMatchObject({ message: "Media analysis unavailable", code: "analysis_unavailable" });
    expect(nativeInvoke).not.toHaveBeenCalled();
  });

  it("rejects invalid intent and malformed native payloads", async () => {
    await expect(analyzeMedia("https://youtube.com/watch?v=demoYT00001", { mode: "raw" } as unknown as SelectionIntent)).rejects.toBeInstanceOf(LiveAnalysisError);
    expect(nativeInvoke).not.toHaveBeenCalled();
    nativeInvoke.mockResolvedValue({ ...response, raw: "PRIVATE" });
    await expect(analyzeMedia("https://youtube.com/watch?v=demoYT00001", intent)).rejects.toMatchObject({ code: "analysis_unavailable" });
  });

  it("preserves allowlisted native codes but hides raw rejections", async () => {
    nativeInvoke.mockRejectedValueOnce("rate_limited");
    await expect(analyzeMedia("https://youtube.com/watch?v=demoYT00001", intent)).rejects.toMatchObject({ message: "Media analysis unavailable", code: "rate_limited" });

    nativeInvoke.mockRejectedValueOnce(new Error("PRIVATE path and signed URL", { cause: "PRIVATE" }));
    const error = await analyzeMedia("https://youtube.com/watch?v=demoYT00001", intent).catch((value: unknown) => value) as LiveAnalysisError;
    expect(error).toBeInstanceOf(LiveAnalysisError);
    expect(error.message).toBe("Media analysis unavailable");
    expect(error.code).toBe("analysis_unavailable");
    expect(error.cause).toBeUndefined();
    expect(String(error)).not.toContain("PRIVATE");
  });
});
