import { describe, expect, it } from "vitest";
import type { SelectionIntent } from "./selection";
import { isLiveAnalysisPreview } from "./liveAnalysis";

const intent: SelectionIntent = { mode: "best_available" };
const plan = { compatibilityMode: false, containerSelection: "automatic" as const, videoProcessing: "copy" as const, outputVideoCodec: "h264",
  mode: "best_available",
  sourceFormatIds: ["c1"],
  videoFormatId: "c1",
  audioFormatId: "c1",
  operation: "keep",
  outputContainer: "mp4", outputAudioCodec: "aac", audioProcessing: "copy",
  warnings: ["metadata_incomplete", "container_unverified", "output_size_unknown"],
};
const response = {
  analysisId: "1",
  media: {
    platform: "youtube",
    mediaId: "sample",
    title: "Synthetic sample",
    formats: [{ id: "c1", container: "mp4", kind: "combined", videoCodec: "avc1", audioCodec: "aac" }],
  },
  rankedVideoFormatIds: ["c1"],
  rankedAudioFormatIds: ["c1"],
  outcome: { status: "selected", plan },
};

describe("isLiveAnalysisPreview", () => {
  it("accepts the exact correlated live preview", () => {
    expect(isLiveAnalysisPreview(response, intent)).toBe(true);
  });

  it.each(["", "0", "01", "-1", "1a", "1".repeat(21)])("rejects analysis ID %j", (analysisId) => {
    expect(isLiveAnalysisPreview({ ...response, analysisId }, intent)).toBe(false);
  });

  it.each([
    null,
    { ...response, raw: "PRIVATE" },
    Object.fromEntries(Object.entries(response).filter(([key]) => key !== "outcome")),
    { ...response, rankedVideoFormatIds: ["missing"] },
    { ...response, media: { ...response.media, formats: [] } },
    { ...response, outcome: { status: "selected", plan: { ...plan, mode: "manual" } } },
  ])("rejects malformed or uncorrelated payload %j", (payload) => {
    expect(isLiveAnalysisPreview(payload, intent)).toBe(false);
  });

  it("correlates the response with a manual reselection intent", () => {
    const manual: SelectionIntent = { mode: "manual", videoFormatId: "c1" };
    const manualResponse = {
      ...response,
      outcome: { status: "selected", plan: { ...plan, mode: "manual" } },
    };
    expect(isLiveAnalysisPreview(manualResponse, manual)).toBe(true);
    expect(isLiveAnalysisPreview(response, manual)).toBe(false);
  });
});
