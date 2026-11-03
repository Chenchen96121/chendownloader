import { isMediaInfo } from "./media";
import {
  isSelectionIntent,
  isSelectionPreview,
  type SelectionIntent,
  type SelectionPreview,
} from "./selection";

export interface LiveAnalysisPreview extends SelectionPreview {
  analysisId: string;
}

const fields = [
  "analysisId",
  "media",
  "rankedVideoFormatIds",
  "rankedAudioFormatIds",
  "outcome",
] as const;

export function isAnalysisId(value: unknown): value is string {
  return typeof value === "string"
    && /^[1-9][0-9]{0,19}$/.test(value)
    && BigInt(value) <= 18_446_744_073_709_551_615n;
}

export function isLiveAnalysisPreview(
  value: unknown,
  intent: SelectionIntent,
): value is LiveAnalysisPreview {
  if (!isSelectionIntent(intent) || typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  const prototype = Object.getPrototypeOf(value);
  const candidate = value as Record<string, unknown>;
  if ((prototype !== Object.prototype && prototype !== null)
    || Object.keys(candidate).length !== fields.length
    || !fields.every((field) => Object.hasOwn(candidate, field))
    || !isAnalysisId(candidate.analysisId)
    || !isMediaInfo(candidate.media)) {
    return false;
  }
  return isSelectionPreview(
    {
      media: candidate.media,
      rankedVideoFormatIds: candidate.rankedVideoFormatIds,
      rankedAudioFormatIds: candidate.rankedAudioFormatIds,
      outcome: candidate.outcome,
    },
    candidate.media.platform,
    intent,
  );
}
