import type { SelectionPreview } from "../../shared/contracts/selection";
import type { VideoResolutionPlan } from "../../shared/contracts/resolutionOptions";
import type { Locale } from "../../shared/i18n/locale";
import { formatFileSize, formatResolution } from "./formatPresentation";
import type {LiveAnalysisPreview} from "../../shared/contracts/liveAnalysis";
import {getResolutionOptions} from "../../shared/ipc/resolutionOptions";

export interface ResolutionPlansResult {analysisId:string;mode:"manual"|"video_only";compatibilityMode:boolean;plans:VideoResolutionPlan[];failed:boolean}
export async function requestResolutionPlans(data:LiveAnalysisPreview,mode:"manual"|"video_only",compatibilityMode:boolean,generation:{current:number},publish:(result:ResolutionPlansResult)=>void):Promise<void>{
  const request=++generation.current;
  try {
    const plans=await getResolutionOptions(data,mode,compatibilityMode);
    if(generation.current===request)publish({analysisId:data.analysisId,mode,compatibilityMode,plans,failed:false});
  } catch {
    if(generation.current===request)publish({analysisId:data.analysisId,mode,compatibilityMode,plans:[],failed:true});
  }
}

export function videoChoiceKey(videoFormatId: string, outputContainer: "mp4" | "webm"): string {
  return JSON.stringify([videoFormatId, outputContainer]);
}
export function resolutionOptions(data: SelectionPreview, mode: "manual" | "video_only", locale: Locale, plans: readonly VideoResolutionPlan[], clipped = false, selectedId = "", selectedContainer = "") {
  const formats = new Map(data.media.formats.map((format) => [format.id, format]));
  const groups = new Map<string, { videoFormatId: string; outputContainer: "mp4" | "webm"; value: string; label: string }>();
  for (const row of plans) {
    const id = row.videoFormatId; const format = formats.get(id);
    if (!format || row.outcome.status !== "selected" || row.outcome.plan.mode !== mode) continue;
    const plan = row.outcome.plan;
    if (plan.outputContainer !== row.outputContainer) continue;
    const dimensions = formatResolution(format, locale);
    const container = row.outputContainer.toUpperCase();
    const group = format.width !== undefined && format.height !== undefined
      ? `${format.width}x${format.height}:${container}`
      : `source:${id}:${container}`;
    if (groups.has(group) && (id !== selectedId || row.outputContainer !== selectedContainer)) continue;
    const size = clipped || plan.videoProcessing === "transcode" || plan.audioProcessing === "transcode" || plan.operation === "extract_video" || plan.estimatedSizeBytes === undefined ? undefined
      : { bytes: plan.estimatedSizeBytes, isApproximate: true };
    groups.set(group, { videoFormatId: id, outputContainer: row.outputContainer, value: videoChoiceKey(id, row.outputContainer),
      label: `${dimensions} · ${formatFileSize(size, locale)} · ${container}` });
  }
  return [...groups.values()];
}
