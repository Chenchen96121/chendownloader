import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { LiveAnalysisPreview } from "../contracts/liveAnalysis";
import { getResolutionOptions } from "./resolutionOptions";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const native = vi.mocked(invoke);
beforeEach(() => { native.mockReset(); });
const data: LiveAnalysisPreview = {
  analysisId: "1", media: { platform: "youtube", mediaId: "demoYT00001", title: "Sample", formats: [{ id: "c1", kind: "combined", container: "mp4", videoCodec: "avc1", audioCodec: "aac" }] },
  rankedVideoFormatIds: ["c1"], rankedAudioFormatIds: ["c1"],
  outcome: { status: "unavailable", code: "format_unavailable" },
};
const response = { analysisId: "1", mode: "video_only", compatibilityMode: false, options: [{ videoFormatId: "c1", outputContainer: "mp4", outcome: { status: "selected", plan: { compatibilityMode: false, containerSelection: "explicit", videoProcessing: "copy" as const, outputVideoCodec: "h264", mode: "video_only", videoFormatId: "c1", sourceFormatIds: ["c1"], operation: "extract_video", outputContainer: "mp4", warnings: ["metadata_incomplete", "container_unverified", "output_size_unknown"] } } }, { videoFormatId: "c1", outputContainer: "webm", outcome: { status: "unavailable", code: "invalid_selection" } }] };
it("obtains correlated native plans without URLs or paths", async () => {
  native.mockResolvedValue(response);
  expect(await getResolutionOptions(data, "video_only")).toEqual(response.options);
  expect(native).toHaveBeenCalledExactlyOnceWith("get_resolution_options", { analysisId: "1", mode: "video_only", compatibilityMode: false });
});
it("rejects a response from a different compatibility generation", async () => {
  native.mockResolvedValue({ ...response, compatibilityMode: true });
  await expect(getResolutionOptions(data, "video_only")).rejects.toMatchObject({ code: "analysis_unavailable" });
});
it.each([{ ...response, analysisId: "2" }, { ...response, mode: "manual" }, { ...response, options: [] }, { ...response, raw: "private" }, { ...response, options: [{ ...response.options[0], videoFormatId: "other" }] }])("rejects malformed or stale response", async (value) => {
  native.mockResolvedValue(value);
  await expect(getResolutionOptions(data, "video_only")).rejects.toMatchObject({ code: "analysis_unavailable" });
});
