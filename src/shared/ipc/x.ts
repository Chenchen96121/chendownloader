import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  isXAnalysisPreview,
  isXDownloadIntent,
  isXDownloadProgress,
  isXDownloadResult,
  isXPreviewPayload,
  type XAnalysisPreview,
  type XDownloadIntent,
  type XDownloadProgress,
  type XDownloadResult,
  type XPreviewPayload,
} from "../contracts/x";
import { isAnalysisId } from "../contracts/liveAnalysis";
import {
  analysisErrorCodes,
  downloadErrorCodes,
  type AnalysisErrorCode,
  type DownloadErrorCode,
} from "../contracts/errors";

const analysisErrors = new Set<unknown>(analysisErrorCodes);
const downloadErrors = new Set<unknown>(downloadErrorCodes);

export interface StartedXDownload {
  jobId: string;
}

export class XAnalysisIpcError extends Error {
  readonly code: AnalysisErrorCode;

  constructor(code: AnalysisErrorCode) {
    super("X analysis unavailable");
    this.name = "XAnalysisIpcError";
    this.code = code;
  }
}

export class XDownloadIpcError extends Error {
  readonly code: DownloadErrorCode;

  constructor(code: DownloadErrorCode) {
    super("X download unavailable");
    this.name = "XDownloadIpcError";
    this.code = code;
  }
}

function analysisError(value: unknown, fallback: AnalysisErrorCode): XAnalysisIpcError {
  return new XAnalysisIpcError(
    analysisErrors.has(value) ? value as AnalysisErrorCode : fallback,
  );
}

function downloadError(value: unknown): XDownloadIpcError {
  return new XDownloadIpcError(
    downloadErrors.has(value) ? value as DownloadErrorCode : "unknown",
  );
}

function validXUrl(value: unknown): value is string {
  if (typeof value !== "string" || value !== value.trim() || value.includes("\\") || /\p{Cc}/u.test(value)) {
    return false;
  }
  if (new TextEncoder().encode(value).length > 2048) return false;
  try {
    const parsed = new URL(value);
    const authority = value.split("://")[1]?.split(/[/?#]/u)[0];
    if (!authority || /[:@]/u.test(authority)) return false;
    const originalPath = value.split("://")[1]?.slice(authority.length).split(/[?#]/u)[0];
    if (originalPath !== parsed.pathname) return false;
    const path = parsed.pathname.replace(/\/$/u, "");
    const postRoute = /^\/(?:[A-Za-z0-9_]{1,15}|i\/web)\/status\/[0-9]{1,20}(?:\/(?:photo|video)\/([1-9]|[12][0-9]|3[0-2]))?$/u;
    return parsed.protocol === "https:"
      && ["x.com", "www.x.com", "twitter.com", "www.twitter.com"].includes(parsed.hostname)
      && postRoute.test(path)
      && parsed.username === ""
      && parsed.password === ""
      && parsed.port === "";
  } catch {
    return false;
  }
}

function isStarted(value: unknown): value is StartedXDownload {
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
    if (!validate(response)) throw new XAnalysisIpcError("analysis_unavailable");
    return response;
  } catch (error) {
    if (error instanceof XAnalysisIpcError) throw error;
    throw analysisError(error, "analysis_unavailable");
  }
}

async function invokeAnalysisVoid(command: string, request: Record<string, unknown>): Promise<void> {
  try {
    const response = await invoke<unknown>(command, { request });
    if (response !== null) throw new XAnalysisIpcError("analysis_unavailable");
  } catch (error) {
    if (error instanceof XAnalysisIpcError) throw error;
    throw analysisError(error, "analysis_unavailable");
  }
}

async function invokeDownloadVoid(command: string, jobId: string): Promise<void> {
  try {
    if (!isAnalysisId(jobId)) throw new XDownloadIpcError("invalid_request");
    const response = await invoke<unknown>(command, { request: { jobId } });
    if (response !== null) throw new XDownloadIpcError("unknown");
  } catch (error) {
    if (error instanceof XDownloadIpcError) throw error;
    throw downloadError(error);
  }
}

export function analyzeX(url: string): Promise<XAnalysisPreview> {
  if (!validXUrl(url)) return Promise.reject(new XAnalysisIpcError("invalid_url"));
  return invokeAnalysis("analyze_x", { url }, isXAnalysisPreview);
}

export function getXPreview(
  analysisId: string,
  itemId: string,
): Promise<XPreviewPayload> {
  if (!isAnalysisId(analysisId) || !isAnalysisId(itemId)) {
    return Promise.reject(new XAnalysisIpcError("analysis_unavailable"));
  }
  return invokeAnalysis(
    "get_x_preview",
    { analysisId, itemId },
    isXPreviewPayload,
  );
}

export function openXSource(analysisId: string): Promise<void> {
  if (!isAnalysisId(analysisId)) {
    return Promise.reject(new XAnalysisIpcError("analysis_unavailable"));
  }
  return invokeAnalysisVoid("open_x_source", { analysisId });
}

export async function startXDownload(
  analysisId: string,
  destinationId: string,
  intent: XDownloadIntent,
): Promise<StartedXDownload> {
  try {
    if (!isAnalysisId(analysisId)
      || !isAnalysisId(destinationId)
      || !isXDownloadIntent(intent)) {
      throw new XDownloadIpcError("invalid_request");
    }
    const response = await invoke<unknown>("start_x_download", {
      request: { analysisId, destinationId, intent },
    });
    if (!isStarted(response)) throw new XDownloadIpcError("unknown");
    return response;
  } catch (error) {
    if (error instanceof XDownloadIpcError) throw error;
    throw downloadError(error);
  }
}

export function cancelXDownload(jobId: string): Promise<void> {
  return invokeDownloadVoid("cancel_x_download", jobId);
}

export async function getXDownloadResult(jobId: string): Promise<XDownloadResult> {
  try {
    if (!isAnalysisId(jobId)) throw new XDownloadIpcError("invalid_request");
    const response = await invoke<unknown>("get_x_download_result", {
      request: { jobId },
    });
    if (!isXDownloadResult(response)) throw new XDownloadIpcError("unknown");
    return response;
  } catch (error) {
    if (error instanceof XDownloadIpcError) throw error;
    throw downloadError(error);
  }
}

export function openXCompletedLocation(jobId: string): Promise<void> {
  return invokeDownloadVoid("open_x_completed_location", jobId);
}

export async function listenXDownloadProgress(
  receiver: (progress: XDownloadProgress) => void,
): Promise<() => void> {
  let unlisten: UnlistenFn;
  try {
    unlisten = await listen<unknown>("x-download-progress", (event) => {
      if (event.event !== "x-download-progress" || !isXDownloadProgress(event.payload)) {
        throw new XDownloadIpcError("unknown");
      }
      receiver(event.payload);
    });
  } catch (error) {
    if (error instanceof XDownloadIpcError) throw error;
    throw downloadError(error);
  }
  return () => { unlisten(); };
}

export interface XIpc {
  analyze(url: string): Promise<XAnalysisPreview>;
  getPreview(analysisId: string, itemId: string): Promise<XPreviewPayload>;
  openSource(analysisId: string): Promise<void>;
  startDownload(
    analysisId: string,
    destinationId: string,
    intent: XDownloadIntent,
  ): Promise<StartedXDownload>;
  cancelDownload(jobId: string): Promise<void>;
  getDownloadResult(jobId: string): Promise<XDownloadResult>;
  openCompletedLocation(jobId: string): Promise<void>;
  listenProgress(receiver: (progress: XDownloadProgress) => void): Promise<() => void>;
}

export const xIpc: XIpc = {
  analyze: analyzeX,
  getPreview: getXPreview,
  openSource: openXSource,
  startDownload: startXDownload,
  cancelDownload: cancelXDownload,
  getDownloadResult: getXDownloadResult,
  openCompletedLocation: openXCompletedLocation,
  listenProgress: listenXDownloadProgress,
};
