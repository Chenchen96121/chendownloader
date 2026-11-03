import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isBilibiliAnalysisPreview, isBilibiliCoverPayload, type BilibiliAnalysisPreview, type BilibiliCoverPayload } from "../contracts/bilibili";
import { isAnalysisId } from "../contracts/liveAnalysis";
import { isSelectionIntent, isVideoContainer, type SelectionIntent } from "../contracts/selection";
import type { VideoResolutionPlan } from "../contracts/resolutionOptions";
import { analysisErrorCodes, downloadErrorCodes, type AnalysisErrorCode, type DownloadErrorCode } from "../contracts/errors";
import { isDownloadProgress, type DownloadProgress } from "../contracts/download";
import { isStartedDownload, isDownloadResult, type StartedDownload, type DownloadResult } from "./downloads";
import { parseResolutionOptions } from "./resolutionOptions";

const analysisErrors = new Set<unknown>(analysisErrorCodes);
const downloadErrors = new Set<unknown>(downloadErrorCodes);
export class BilibiliAnalysisIpcError extends Error {
  constructor(readonly code: AnalysisErrorCode) { super("Bilibili analysis unavailable"); this.name = "BilibiliAnalysisIpcError"; }
}
export class BilibiliDownloadIpcError extends Error {
  constructor(readonly code: DownloadErrorCode) { super("Bilibili download unavailable"); this.name = "BilibiliDownloadIpcError"; }
}
function analysisError(value: unknown): BilibiliAnalysisIpcError {
  return value instanceof BilibiliAnalysisIpcError ? value : new BilibiliAnalysisIpcError(analysisErrors.has(value) ? value as AnalysisErrorCode : "analysis_unavailable");
}
function downloadError(value: unknown): BilibiliDownloadIpcError {
  return value instanceof BilibiliDownloadIpcError ? value : new BilibiliDownloadIpcError(downloadErrors.has(value) ? value as DownloadErrorCode : "unknown");
}
async function analysis<T>(command: string, request: Record<string, unknown>, valid: (value: unknown) => value is T): Promise<T> {
  try {
    const response = await invoke<unknown>(command, { request });
    if (!valid(response)) throw new BilibiliAnalysisIpcError("analysis_unavailable");
    return response;
  } catch (error) { throw analysisError(error); }
}
function analysisId(id: string): void {
  if (!isAnalysisId(id)) throw new BilibiliAnalysisIpcError("analysis_unavailable");
}
async function download<T>(command: string, request: Record<string, unknown>, valid: (value: unknown) => value is T): Promise<T> {
  try {
    const response = await invoke<unknown>(command, { request });
    if (!valid(response)) throw new BilibiliDownloadIpcError("unknown");
    return response;
  } catch (error) { throw downloadError(error); }
}
const isNull = (value: unknown): value is null => value === null;
async function downloadVoid(command: string, jobId: string): Promise<void> {
  if (!isAnalysisId(jobId)) throw new BilibiliDownloadIpcError("invalid_request");
  await download(command, { jobId }, isNull);
}
function validPreview(data: BilibiliAnalysisPreview, mode: "manual" | "video_only", compatibilityMode: boolean): boolean {
  try {
    if (data.outcome.status !== "selected") return isBilibiliAnalysisPreview(data, { mode, compatibilityMode });
    const plan = data.outcome.plan;
    const intent: SelectionIntent = { mode: plan.mode, compatibilityMode: plan.compatibilityMode };
    if (plan.clip) intent.clip = plan.clip;
    if (plan.mode === "manual" || plan.mode === "video_only") {
      if (plan.videoFormatId) intent.videoFormatId = plan.videoFormatId;
      if (plan.containerSelection === "explicit" && isVideoContainer(plan.outputContainer)) intent.outputContainer = plan.outputContainer;
    }
    return isBilibiliAnalysisPreview(data, intent);
  } catch { return false; }
}

export interface BilibiliIpc {
  analyze(url: string, intent: SelectionIntent): Promise<BilibiliAnalysisPreview>;
  preview(analysisId: string, intent: SelectionIntent): Promise<BilibiliAnalysisPreview>;
  resolutionOptions(data: BilibiliAnalysisPreview, mode: "manual" | "video_only", compatibilityMode: boolean): Promise<VideoResolutionPlan[]>;
  cover(analysisId: string): Promise<BilibiliCoverPayload>;
  openSource(analysisId: string): Promise<void>;
  startDownload(analysisId: string, destinationId: string, intent: SelectionIntent): Promise<StartedDownload>;
  cancelDownload(jobId: string): Promise<void>;
  getDownloadResult(jobId: string): Promise<DownloadResult>;
  openCompletedLocation(jobId: string): Promise<void>;
  listenProgress(receiver: (progress: DownloadProgress) => void): Promise<() => void>;
}
export const bilibiliIpc: BilibiliIpc = {
  async analyze(url, intent) {
    if (typeof url !== "string" || url !== url.trim() || url.length === 0 || url.includes("\\") || /\p{Cc}/u.test(url) || new TextEncoder().encode(url).length > 2048) throw new BilibiliAnalysisIpcError("invalid_url");
    if (!isSelectionIntent(intent)) throw new BilibiliAnalysisIpcError("analysis_unavailable");
    return analysis("analyze_bilibili", { url, intent }, (value): value is BilibiliAnalysisPreview => isBilibiliAnalysisPreview(value, intent));
  },
  async preview(id, intent) {
    analysisId(id);
    if (!isSelectionIntent(intent)) throw new BilibiliAnalysisIpcError("analysis_unavailable");
    return analysis("preview_bilibili_selection", { analysisId: id, intent }, (value): value is BilibiliAnalysisPreview => isBilibiliAnalysisPreview(value, intent) && value.analysisId === id);
  },
  async resolutionOptions(data, mode, compatibilityMode) {
    try {
      if ((mode !== "manual" && mode !== "video_only") || typeof compatibilityMode !== "boolean" || !validPreview(data, mode, compatibilityMode)) throw new BilibiliAnalysisIpcError("analysis_unavailable");
      const value = await invoke<unknown>("get_bilibili_resolution_options", { request: { analysisId: data.analysisId, mode, compatibilityMode } });
      return parseResolutionOptions(value, data, mode, compatibilityMode);
    } catch (error) { throw analysisError(error); }
  },
  async cover(id) {
    analysisId(id);
    return analysis("get_bilibili_cover", { analysisId: id }, isBilibiliCoverPayload);
  },
  async openSource(id) { analysisId(id); await analysis("open_bilibili_source", { analysisId: id }, isNull); },
  async startDownload(analysisId, destinationId, intent) {
    if (!isAnalysisId(analysisId) || !isAnalysisId(destinationId) || !isSelectionIntent(intent)) throw new BilibiliDownloadIpcError("invalid_request");
    return download("start_bilibili_download", { analysisId, destinationId, intent }, isStartedDownload);
  },
  cancelDownload: (id) => downloadVoid("cancel_bilibili_download", id),
  async getDownloadResult(jobId) {
    if (!isAnalysisId(jobId)) throw new BilibiliDownloadIpcError("invalid_request");
    return download("get_bilibili_download_result", { jobId }, (value): value is DownloadResult => isDownloadResult(value) && value.jobId === jobId);
  },
  openCompletedLocation: (id) => downloadVoid("open_bilibili_completed_location", id),
  async listenProgress(receiver) {
    try {
      return await listen<unknown>("bilibili-download-progress", (event) => {
        if (event.event !== "bilibili-download-progress" || !isDownloadProgress(event.payload)) throw new BilibiliDownloadIpcError("unknown");
        receiver(event.payload);
      });
    } catch (error) { throw downloadError(error); }
  },
};
