import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  isTikTokAnalysisPreview,
  isTikTokDownloadIntent,
  isTikTokDownloadProgress,
  isTikTokDownloadResult,
  isTikTokPreviewPayload,
  type TikTokAnalysisPreview,
  type TikTokDownloadIntent,
  type TikTokDownloadProgress,
  type TikTokDownloadResult,
  type TikTokPreviewPayload,
} from "../contracts/tiktok";
import { isAnalysisId } from "../contracts/liveAnalysis";
import {
  analysisErrorCodes,
  downloadErrorCodes,
  type AnalysisErrorCode,
  type DownloadErrorCode,
} from "../contracts/errors";

const analysisErrors = new Set<unknown>(analysisErrorCodes);
const downloadErrors = new Set<unknown>(downloadErrorCodes);

export interface StartedTikTokDownload {
  jobId: string;
}

export class TikTokAnalysisIpcError extends Error {
  readonly code: AnalysisErrorCode;

  constructor(code: AnalysisErrorCode) {
    super("TikTok analysis unavailable");
    this.name = "TikTokAnalysisIpcError";
    this.code = code;
  }
}

export class TikTokDownloadIpcError extends Error {
  readonly code: DownloadErrorCode;

  constructor(code: DownloadErrorCode) {
    super("TikTok download unavailable");
    this.name = "TikTokDownloadIpcError";
    this.code = code;
  }
}

function analysisError(value: unknown, fallback: AnalysisErrorCode): TikTokAnalysisIpcError {
  return new TikTokAnalysisIpcError(
    analysisErrors.has(value) ? value as AnalysisErrorCode : fallback,
  );
}

function downloadError(value: unknown): TikTokDownloadIpcError {
  return new TikTokDownloadIpcError(
    downloadErrors.has(value) ? value as DownloadErrorCode : "unknown",
  );
}

function validTikTokUrl(value:unknown):value is string {
 if(typeof value!=="string"||value.length>8192||value.includes("\\")||/[\x00-\x08\x0e-\x1f\x7f]/u.test(value))return false;
 const links=value.trim().split(/\s+/u).filter(part=>/^https?:/u.test(part));
 if(links.length!==1)return false;
 const raw=links[0]!.replace(/[。，,;；]+$/u,"");
 try{
  const url=new URL(raw);const authority=raw.split("://")[1]?.split(/[/?#]/u)[0];
  if(!authority||/[:@]/u.test(authority)||url.protocol!=="https:")return false;
  if(raw.split("://")[1]?.slice(authority.length).split(/[?#]/u)[0]!==url.pathname)return false;
  if(["vm.tiktok.com","vt.tiktok.com"].includes(url.hostname))return /^\/[A-Za-z0-9_-]{1,128}\/?$/u.test(url.pathname);
  return ["tiktok.com","www.tiktok.com"].includes(url.hostname)&&(/^\/@[A-Za-z0-9_.-]{1,64}\/(?:video|photo)\/[0-9]{1,20}\/?$/u.test(url.pathname)||/^\/t\/[A-Za-z0-9_-]{1,128}\/?$/u.test(url.pathname));
 }catch{return false;}
}

function isStarted(value: unknown): value is StartedTikTokDownload {
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
    if (!validate(response)) throw new TikTokAnalysisIpcError("analysis_unavailable");
    return response;
  } catch (error) {
    if (error instanceof TikTokAnalysisIpcError) throw error;
    throw analysisError(error, "analysis_unavailable");
  }
}

async function invokeAnalysisVoid(command: string, request: Record<string, unknown>): Promise<void> {
  try {
    const response = await invoke<unknown>(command, { request });
    if (response !== null) throw new TikTokAnalysisIpcError("analysis_unavailable");
  } catch (error) {
    if (error instanceof TikTokAnalysisIpcError) throw error;
    throw analysisError(error, "analysis_unavailable");
  }
}

async function invokeDownloadVoid(command: string, jobId: string): Promise<void> {
  try {
    if (!isAnalysisId(jobId)) throw new TikTokDownloadIpcError("invalid_request");
    const response = await invoke<unknown>(command, { request: { jobId } });
    if (response !== null) throw new TikTokDownloadIpcError("unknown");
  } catch (error) {
    if (error instanceof TikTokDownloadIpcError) throw error;
    throw downloadError(error);
  }
}

export function analyzeTikTok(url: string): Promise<TikTokAnalysisPreview> {
  if (!validTikTokUrl(url)) return Promise.reject(new TikTokAnalysisIpcError("invalid_url"));
  return invokeAnalysis("analyze_tiktok", { url }, isTikTokAnalysisPreview);
}

export function getTikTokPreview(
  analysisId: string,
  itemId: string,
): Promise<TikTokPreviewPayload> {
  if (!isAnalysisId(analysisId) || !isAnalysisId(itemId)) {
    return Promise.reject(new TikTokAnalysisIpcError("analysis_unavailable"));
  }
  return invokeAnalysis(
    "get_tiktok_preview",
    { analysisId, itemId },
    isTikTokPreviewPayload,
  );
}

export function openTikTokSource(analysisId: string): Promise<void> {
  if (!isAnalysisId(analysisId)) {
    return Promise.reject(new TikTokAnalysisIpcError("analysis_unavailable"));
  }
  return invokeAnalysisVoid("open_tiktok_source", { analysisId });
}

export async function startTikTokDownload(
  analysisId: string,
  destinationId: string,
  intent: TikTokDownloadIntent,
): Promise<StartedTikTokDownload> {
  try {
    if (!isAnalysisId(analysisId)
      || !isAnalysisId(destinationId)
      || !isTikTokDownloadIntent(intent)) {
      throw new TikTokDownloadIpcError("invalid_request");
    }
    const response = await invoke<unknown>("start_tiktok_download", {
      request: { analysisId, destinationId, intent },
    });
    if (!isStarted(response)) throw new TikTokDownloadIpcError("unknown");
    return response;
  } catch (error) {
    if (error instanceof TikTokDownloadIpcError) throw error;
    throw downloadError(error);
  }
}

export function cancelTikTokDownload(jobId: string): Promise<void> {
  return invokeDownloadVoid("cancel_tiktok_download", jobId);
}

export async function getTikTokDownloadResult(jobId: string): Promise<TikTokDownloadResult> {
  try {
    if (!isAnalysisId(jobId)) throw new TikTokDownloadIpcError("invalid_request");
    const response = await invoke<unknown>("get_tiktok_download_result", {
      request: { jobId },
    });
    if (!isTikTokDownloadResult(response)) throw new TikTokDownloadIpcError("unknown");
    return response;
  } catch (error) {
    if (error instanceof TikTokDownloadIpcError) throw error;
    throw downloadError(error);
  }
}

export function openTikTokCompletedLocation(jobId: string): Promise<void> {
  return invokeDownloadVoid("open_tiktok_completed_location", jobId);
}

export async function listenTikTokDownloadProgress(
  receiver: (progress: TikTokDownloadProgress) => void,
): Promise<() => void> {
  let unlisten: UnlistenFn;
  try {
    unlisten = await listen<unknown>("tiktok-download-progress", (event) => {
      if (event.event !== "tiktok-download-progress" || !isTikTokDownloadProgress(event.payload)) {
        throw new TikTokDownloadIpcError("unknown");
      }
      receiver(event.payload);
    });
  } catch (error) {
    if (error instanceof TikTokDownloadIpcError) throw error;
    throw downloadError(error);
  }
  return () => { unlisten(); };
}

export interface TikTokIpc {
  analyze(url: string): Promise<TikTokAnalysisPreview>;
  getPreview(analysisId: string, itemId: string): Promise<TikTokPreviewPayload>;
  openSource(analysisId: string): Promise<void>;
  startDownload(
    analysisId: string,
    destinationId: string,
    intent: TikTokDownloadIntent,
  ): Promise<StartedTikTokDownload>;
  cancelDownload(jobId: string): Promise<void>;
  getDownloadResult(jobId: string): Promise<TikTokDownloadResult>;
  openCompletedLocation(jobId: string): Promise<void>;
  listenProgress(receiver: (progress: TikTokDownloadProgress) => void): Promise<() => void>;
}

export const tiktokIpc: TikTokIpc = {
  analyze: analyzeTikTok,
  getPreview: getTikTokPreview,
  openSource: openTikTokSource,
  startDownload: startTikTokDownload,
  cancelDownload: cancelTikTokDownload,
  getDownloadResult: getTikTokDownloadResult,
  openCompletedLocation: openTikTokCompletedLocation,
  listenProgress: listenTikTokDownloadProgress,
};

export interface TikTokRuntimeStatus{state:"ready"|"missing"|"integrity_failed";version?:string}
export async function getTikTokRuntimeStatus():Promise<TikTokRuntimeStatus>{
 const value:unknown=await invoke("get_tiktok_runtime_status");
 if(typeof value!=="object"||value===null||Array.isArray(value))throw new TikTokAnalysisIpcError("tool_integrity_failed");
 const record=value as Record<string,unknown>;
 if(Object.keys(record).some(key=>!["state","version"].includes(key))||!["ready","missing","integrity_failed"].includes(record.state as string)||(record.state==="ready"?record.version!=="1.1.0":Object.hasOwn(record,"version")))throw new TikTokAnalysisIpcError("tool_integrity_failed");
 return value as TikTokRuntimeStatus;
}
