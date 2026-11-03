import { invoke } from "@tauri-apps/api/core";
import { isAnalysisId, type LiveAnalysisPreview } from "../contracts/liveAnalysis";
import { isSelectionPreview, isVideoContainer } from "../contracts/selection";
import type { VideoResolutionPlan } from "../contracts/resolutionOptions";
import { LiveAnalysisError } from "./liveAnalysis";

function exactRecord(value: unknown, fields: string[]): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    && [Object.prototype, null].includes(Object.getPrototypeOf(value))
    && Object.keys(value).length === fields.length && fields.every((key) => Object.hasOwn(value, key));
}
export async function getResolutionOptions(data: LiveAnalysisPreview, mode: "manual" | "video_only", compatibilityMode = false): Promise<VideoResolutionPlan[]> {
  try {
    if (!isAnalysisId(data.analysisId) || (mode !== "manual" && mode !== "video_only") || typeof compatibilityMode !== "boolean") throw new Error();
    const value = await invoke<unknown>("get_resolution_options", { analysisId: data.analysisId, mode, compatibilityMode });
    return parseResolutionOptions(value, data, mode, compatibilityMode);
  } catch { throw new LiveAnalysisError("analysis_unavailable"); }
}

export function parseResolutionOptions(value: unknown, data: LiveAnalysisPreview, mode: "manual" | "video_only", compatibilityMode: boolean): VideoResolutionPlan[] {
    if (!exactRecord(value, ["analysisId", "mode", "compatibilityMode", "options"]) || value.analysisId !== data.analysisId || value.mode !== mode || value.compatibilityMode !== compatibilityMode
      || !Array.isArray(value.options) || value.options.length > data.rankedVideoFormatIds.length * 2) throw new Error();
    const expected = data.rankedVideoFormatIds.flatMap((id) => (compatibilityMode ? ["mp4"] : ["mp4", "webm"]).map((outputContainer) => ({id, outputContainer})));
    if (value.options.length !== expected.length) throw new Error();
    for (const [index, row] of value.options.entries()) {
      if (!exactRecord(row, ["videoFormatId", "outputContainer", "outcome"]) || row.videoFormatId !== expected[index]?.id || row.outputContainer !== expected[index]?.outputContainer || !isVideoContainer(row.outputContainer)
        || !isSelectionPreview({ media: data.media, rankedVideoFormatIds: data.rankedVideoFormatIds, rankedAudioFormatIds: data.rankedAudioFormatIds, outcome: row.outcome }, data.media.platform, { mode, videoFormatId: row.videoFormatId as string, outputContainer: row.outputContainer, compatibilityMode })) throw new Error();
    }
    return value.options as VideoResolutionPlan[];
}
