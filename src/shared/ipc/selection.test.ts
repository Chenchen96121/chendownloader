import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { Platform } from "../contracts/media";
import type { SelectionIntent } from "../contracts/selection";
import { previewMediaSelectionFixture } from "./selection";

// Native transport is the only external boundary. All validation stays real.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const nativeInvoke = vi.mocked(invoke);
const intent: SelectionIntent = { mode: "best_available" };
const plan = { compatibilityMode: false, containerSelection: "automatic" as const, videoProcessing: "copy" as const, outputVideoCodec: "h264",  mode: "best_available", sourceFormatIds: ["c1"], videoFormatId: "c1", audioFormatId: "c1", operation: "keep", outputContainer: "mp4", outputAudioCodec: "aac", audioProcessing: "copy", warnings: ["metadata_incomplete", "container_unverified", "output_size_unknown"] };
const response = {
  media: { platform: "youtube", mediaId: "sample", title: "Synthetic sample", formats: [{ id: "c1", container: "mp4", kind: "combined", videoCodec: "avc1", audioCodec: "aac" }] },
  rankedVideoFormatIds: ["c1"], rankedAudioFormatIds: ["c1"], outcome: { status: "selected", plan },
};
beforeEach(() => { nativeInvoke.mockReset(); });

describe("previewMediaSelectionFixture", () => {
  it("sends only platform and intent and returns a validated selection", async () => {
    nativeInvoke.mockResolvedValue(response);
    const result = await previewMediaSelectionFixture("youtube", intent);
    expect(nativeInvoke).toHaveBeenCalledExactlyOnceWith("preview_media_selection_fixture", { platform: "youtube", intent });
    expect(result.media.title).toBe("Synthetic sample");
    expect(result.outcome).toEqual({ status: "selected", plan });
  });
  it("returns unavailable outcomes with their media intact", async () => {
    nativeInvoke.mockResolvedValue({ ...response, outcome: { status: "unavailable", code: "format_unavailable" } });
    const result = await previewMediaSelectionFixture("youtube", intent);
    expect(result.outcome).toEqual({ status: "unavailable", code: "format_unavailable" });
    expect(result.media.mediaId).toBe("sample");
  });
  it.each(["bilibili", "x", "instagram"] as const)("accepts platform %s", async (platform) => {
    nativeInvoke.mockResolvedValue({ ...response, media: { ...response.media, platform } });
    expect((await previewMediaSelectionFixture(platform, intent)).media.platform).toBe(platform);
  });
  it("rejects an invalid platform before IPC", async () => {
    await expect(previewMediaSelectionFixture("unknown" as Platform, intent)).rejects.toThrow("Selection preview unavailable");
    expect(nativeInvoke).not.toHaveBeenCalled();
  });
  it.each([{}, { mode: "raw" }, { mode: "manual", videoFormatId: "../x" }, { mode: "audio_only", clip: { startMs: 0, endMs: 1 } }, { mode: "best_available", raw: "PRIVATE" }])("rejects invalid intent %j before IPC", async (invalid) => {
    await expect(previewMediaSelectionFixture("youtube", invalid as SelectionIntent)).rejects.toThrow("Selection preview unavailable");
    expect(nativeInvoke).not.toHaveBeenCalled();
  });
  it.each([
    null, { ...response, raw: "PRIVATE" },
    { ...response, media: { ...response.media, platform: "x" } },
    { ...response, outcome: { status: "selected", plan: { ...plan, mode: "manual" } } },
    { ...response, outcome: { status: "selected", plan: { ...plan, operation: "merge" } } },
    { ...response, rankedAudioFormatIds: ["missing"] },
  ])("redacts malformed or mismatched response %j", async (payload) => {
    nativeInvoke.mockResolvedValue(payload);
    await expect(previewMediaSelectionFixture("youtube", intent)).rejects.toMatchObject({ message: "Selection preview unavailable" });
  });
  it("replaces native rejection with a fixed error without raw text or cause", async () => {
    nativeInvoke.mockRejectedValue(new Error("PRIVATE path credentials", { cause: "PRIVATE" }));
    const error = await previewMediaSelectionFixture("youtube", intent).catch((value: unknown) => value);
    expect(error).toBeInstanceOf(Error);
    expect((error as Error).message).toBe("Selection preview unavailable");
    expect((error as Error).cause).toBeUndefined();
    expect(String(error)).not.toContain("PRIVATE");
  });
});
