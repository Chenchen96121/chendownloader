import { invoke } from "@tauri-apps/api/core";
import {
  isAnalysisId,
  isLiveAnalysisPreview,
  type LiveAnalysisPreview,
} from "../contracts/liveAnalysis";
import { isSelectionIntent, type SelectionIntent } from "../contracts/selection";
import {
  analysisErrorCodes,
  type AnalysisErrorCode,
} from "../contracts/errors";

export { analysisErrorCodes, type AnalysisErrorCode } from "../contracts/errors";

const errorCodeSet = new Set<unknown>(analysisErrorCodes);

export class LiveAnalysisError extends Error {
  readonly code: AnalysisErrorCode;

  constructor(code: AnalysisErrorCode) {
    super("Media analysis unavailable");
    this.name = "LiveAnalysisError";
    this.code = code;
  }
}

function validUrlInput(value: unknown): value is string {
  if (typeof value !== "string" || value.includes("\\") || /\p{Cc}/u.test(value)) return false;
  const trimmed = value.trim();
  return trimmed.length > 0 && new TextEncoder().encode(trimmed).length <= 2048;
}

function publicError(value: unknown, fallback: AnalysisErrorCode): LiveAnalysisError {
  return new LiveAnalysisError(errorCodeSet.has(value) ? value as AnalysisErrorCode : fallback);
}

export async function openMediaPage(analysisId: string): Promise<void> {
  try {
    if (!isAnalysisId(analysisId)) throw new LiveAnalysisError("analysis_unavailable");
    await invoke("open_media_page", { analysisId });
  } catch (error) {
    if (error instanceof LiveAnalysisError) throw error;
    throw publicError(error, "analysis_unavailable");
  }
}

export async function analyzeMedia(url: string, intent: SelectionIntent): Promise<LiveAnalysisPreview> {
  try {
    if (!validUrlInput(url)) throw new LiveAnalysisError("invalid_url");
    if (!isSelectionIntent(intent)) throw new LiveAnalysisError("analysis_unavailable");
    const response = await invoke<unknown>("analyze_media", { url, intent });
    if (!isLiveAnalysisPreview(response, intent)) throw new LiveAnalysisError("analysis_unavailable");
    return response;
  } catch (error) {
    if (error instanceof LiveAnalysisError) throw error;
    throw publicError(error, "analysis_unavailable");
  }
}

export async function previewMediaSelection(
  analysisId: string,
  intent: SelectionIntent,
): Promise<LiveAnalysisPreview> {
  try {
    if (!isAnalysisId(analysisId) || !isSelectionIntent(intent)) {
      throw new LiveAnalysisError("analysis_unavailable");
    }
    const response = await invoke<unknown>("preview_media_selection", { analysisId, intent });
    if (!isLiveAnalysisPreview(response, intent) || response.analysisId !== analysisId) {
      throw new LiveAnalysisError("analysis_unavailable");
    }
    return response;
  } catch (error) {
    if (error instanceof LiveAnalysisError) throw error;
    throw publicError(error, "analysis_unavailable");
  }
}
