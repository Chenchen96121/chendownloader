import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  isInstagramAnalysisPreview,
  isInstagramDownloadIntent,
  isInstagramDownloadProgress,
  isInstagramDownloadResult,
  isInstagramPreviewPayload,
  type InstagramAnalysisPreview,
  type InstagramDownloadIntent,
  type InstagramDownloadProgress,
  type InstagramDownloadResult,
  type InstagramPreviewPayload,
} from "../contracts/instagram";
import { isAnalysisId } from "../contracts/liveAnalysis";
import {
  analysisErrorCodes,
  downloadErrorCodes,
  type AnalysisErrorCode,
  type DownloadErrorCode,
} from "../contracts/errors";

const analysisErrors = new Set<unknown>(analysisErrorCodes);
const downloadErrors = new Set<unknown>(downloadErrorCodes);
const shortcode = /^[A-Za-z0-9_-]{1,64}$/u;

export interface StartedInstagramDownload {
  jobId: string;
}

export class InstagramAnalysisIpcError extends Error {
  readonly code: AnalysisErrorCode;

  constructor(code: AnalysisErrorCode) {
    super("Instagram analysis unavailable");
    this.name = "InstagramAnalysisIpcError";
    this.code = code;
  }
}

export class InstagramDownloadIpcError extends Error {
  readonly code: DownloadErrorCode;

  constructor(code: DownloadErrorCode) {
    super("Instagram download unavailable");
    this.name = "InstagramDownloadIpcError";
    this.code = code;
  }
}

function analysisError(value: unknown, fallback: AnalysisErrorCode): InstagramAnalysisIpcError {
  return new InstagramAnalysisIpcError(
    analysisErrors.has(value) ? value as AnalysisErrorCode : fallback,
  );
}

function downloadError(value: unknown): InstagramDownloadIpcError {
  return new InstagramDownloadIpcError(
    downloadErrors.has(value) ? value as DownloadErrorCode : "unknown",
  );
}

function validInstagramUrl(value: unknown): value is string {
  if (typeof value !== "string" || value !== value.trim() || value.includes("\\") || /\p{Cc}/u.test(value)) {
    return false;
  }
  if (new TextEncoder().encode(value).length > 2048) return false;
  try {
    const parsed = new URL(value);
    const segments = parsed.pathname.split("/").filter(Boolean);
    return parsed.protocol === "https:"
      && (parsed.hostname === "instagram.com" || parsed.hostname === "www.instagram.com")
      && segments.length === 2
      && (segments[0] === "p" || segments[0] === "reel" || segments[0] === "reels")
      && shortcode.test(segments[1] ?? "")
      && parsed.username === ""
      && parsed.password === ""
      && parsed.port === "";
  } catch {
    return false;
  }
}

function isStarted(value: unknown): value is StartedInstagramDownload {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const prototype = Object.getPrototypeOf(value);
  const record = value as Record<string, unknown>;
  return (prototype === Object.prototype || prototype === null)
    && Object.keys(record).length === 1
    && Object.hasOwn(record, "jobId")
    && isAnalysisId(record.jobId);
}

async function invokeAnalysis<T>(
  command: string,
  request: Record<string, unknown>,
  validate: (value: unknown) => value is T,
): Promise<T> {
  try {
    const response = await invoke<unknown>(command, { request });
    if (!validate(response)) throw new InstagramAnalysisIpcError("analysis_unavailable");
    return response;
  } catch (error) {
    if (error instanceof InstagramAnalysisIpcError) throw error;
    throw analysisError(error, "analysis_unavailable");
  }
}

async function invokeAnalysisVoid(command: string, request: Record<string, unknown>): Promise<void> {
  try {
    const response = await invoke<unknown>(command, { request });
    if (response !== null) throw new InstagramAnalysisIpcError("analysis_unavailable");
  } catch (error) {
    if (error instanceof InstagramAnalysisIpcError) throw error;
    throw analysisError(error, "analysis_unavailable");
  }
}

async function invokeDownloadVoid(command: string, jobId: string): Promise<void> {
  try {
    if (!isAnalysisId(jobId)) throw new InstagramDownloadIpcError("invalid_request");
    const response = await invoke<unknown>(command, { request: { jobId } });
    if (response !== null) throw new InstagramDownloadIpcError("unknown");
  } catch (error) {
    if (error instanceof InstagramDownloadIpcError) throw error;
    throw downloadError(error);
  }
}

export function analyzeInstagram(url: string): Promise<InstagramAnalysisPreview> {
  if (!validInstagramUrl(url)) return Promise.reject(new InstagramAnalysisIpcError("invalid_url"));
  return invokeAnalysis("analyze_instagram", { url }, isInstagramAnalysisPreview);
}

export function getInstagramPreview(
  analysisId: string,
  itemId: string,
): Promise<InstagramPreviewPayload> {
  if (!isAnalysisId(analysisId) || !isAnalysisId(itemId)) {
    return Promise.reject(new InstagramAnalysisIpcError("analysis_unavailable"));
  }
  return invokeAnalysis(
    "get_instagram_preview",
    { analysisId, itemId },
    isInstagramPreviewPayload,
  );
}

export function openInstagramSource(analysisId: string): Promise<void> {
  if (!isAnalysisId(analysisId)) {
    return Promise.reject(new InstagramAnalysisIpcError("analysis_unavailable"));
  }
  return invokeAnalysisVoid("open_instagram_source", { analysisId });
}

export async function startInstagramDownload(
  analysisId: string,
  destinationId: string,
  intent: InstagramDownloadIntent,
): Promise<StartedInstagramDownload> {
  try {
    if (!isAnalysisId(analysisId)
      || !isAnalysisId(destinationId)
      || !isInstagramDownloadIntent(intent)) {
      throw new InstagramDownloadIpcError("invalid_request");
    }
    const response = await invoke<unknown>("start_instagram_download", {
      request: { analysisId, destinationId, intent },
    });
    if (!isStarted(response)) throw new InstagramDownloadIpcError("unknown");
    return response;
  } catch (error) {
    if (error instanceof InstagramDownloadIpcError) throw error;
    throw downloadError(error);
  }
}

export function cancelInstagramDownload(jobId: string): Promise<void> {
  return invokeDownloadVoid("cancel_instagram_download", jobId);
}

export async function getInstagramDownloadResult(jobId: string): Promise<InstagramDownloadResult> {
  try {
    if (!isAnalysisId(jobId)) throw new InstagramDownloadIpcError("invalid_request");
    const response = await invoke<unknown>("get_instagram_download_result", {
      request: { jobId },
    });
    if (!isInstagramDownloadResult(response)) throw new InstagramDownloadIpcError("unknown");
    return response;
  } catch (error) {
    if (error instanceof InstagramDownloadIpcError) throw error;
    throw downloadError(error);
  }
}

export function openInstagramCompletedLocation(jobId: string): Promise<void> {
  return invokeDownloadVoid("open_instagram_completed_location", jobId);
}

export async function listenInstagramDownloadProgress(
  receiver: (progress: InstagramDownloadProgress) => void,
): Promise<() => void> {
  let unlisten: UnlistenFn;
  try {
    unlisten = await listen<unknown>("instagram-download-progress", (event) => {
      if (!isInstagramDownloadProgress(event.payload)) {
        throw new InstagramDownloadIpcError("unknown");
      }
      receiver(event.payload);
    });
  } catch (error) {
    if (error instanceof InstagramDownloadIpcError) throw error;
    throw downloadError(error);
  }
  return () => { unlisten(); };
}

export interface InstagramIpc {
  analyze(url: string): Promise<InstagramAnalysisPreview>;
  getPreview(analysisId: string, itemId: string): Promise<InstagramPreviewPayload>;
  openSource(analysisId: string): Promise<void>;
  startDownload(
    analysisId: string,
    destinationId: string,
    intent: InstagramDownloadIntent,
  ): Promise<StartedInstagramDownload>;
  cancelDownload(jobId: string): Promise<void>;
  getDownloadResult(jobId: string): Promise<InstagramDownloadResult>;
  openCompletedLocation(jobId: string): Promise<void>;
  listenProgress(receiver: (progress: InstagramDownloadProgress) => void): Promise<() => void>;
}

export const instagramIpc: InstagramIpc = {
  analyze: analyzeInstagram,
  getPreview: getInstagramPreview,
  openSource: openInstagramSource,
  startDownload: startInstagramDownload,
  cancelDownload: cancelInstagramDownload,
  getDownloadResult: getInstagramDownloadResult,
  openCompletedLocation: openInstagramCompletedLocation,
  listenProgress: listenInstagramDownloadProgress,
};
