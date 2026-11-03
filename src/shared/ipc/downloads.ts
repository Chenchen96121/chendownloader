import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  downloadErrorCodes,
  isDownloadProgress,
  type DownloadErrorCode,
  type DownloadProgress,
} from "../contracts/download";
import { isAnalysisId } from "../contracts/liveAnalysis";
import { isSelectionIntent, type SelectionIntent } from "../contracts/selection";

export interface StartedDownload {
  jobId: string;
}

export type DownloadResult =
  | {
    jobId: string;
    state: "completed";
    errorCode: null;
    displayName: string;
  }
  | {
    jobId: string;
    state: "failed";
    errorCode: DownloadErrorCode;
    displayName: null;
  }
  | {
    jobId: string;
    state: "canceled";
    errorCode: null;
    displayName: null;
  };

const errorCodeSet = new Set<unknown>(downloadErrorCodes);

export class DownloadCommandError extends Error {
  readonly code: DownloadErrorCode;

  constructor(code: DownloadErrorCode) {
    super("Download operation unavailable");
    this.name = "DownloadCommandError";
    this.code = code;
  }
}

function publicError(value: unknown): DownloadCommandError {
  return new DownloadCommandError(
    errorCodeSet.has(value) ? value as DownloadErrorCode : "unknown",
  );
}

export function isStartedDownload(value: unknown): value is StartedDownload {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const record = value as Record<string, unknown>;
  const prototype = Object.getPrototypeOf(value);
  return (prototype === Object.prototype || prototype === null)
    && Object.keys(record).length === 1
    && Object.hasOwn(record, "jobId")
    && isAnalysisId(record.jobId);
}

function isSafeOutputName(value: unknown): value is string {
  return typeof value === "string"
    && value.trim().length > 0
    && [...value].length <= 255
    && !/[<>:"/\\|?*\p{Cc}\u{200e}\u{200f}\u{202a}-\u{202e}\u{2066}-\u{2069}]/u.test(value);
}

export function isDownloadResult(value: unknown): value is DownloadResult {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const record = value as Record<string, unknown>;
  const prototype = Object.getPrototypeOf(value);
  if ((prototype !== Object.prototype && prototype !== null)
    || Object.keys(record).length !== 4
    || !["jobId", "state", "errorCode", "displayName"].every((field) => Object.hasOwn(record, field))
    || !isAnalysisId(record.jobId)) {
    return false;
  }
  if (record.state === "completed") {
    return record.errorCode === null && isSafeOutputName(record.displayName);
  }
  if (record.state === "failed") {
    return errorCodeSet.has(record.errorCode) && record.displayName === null;
  }
  return record.state === "canceled"
    && record.errorCode === null
    && record.displayName === null;
}

export async function startDownload(
  analysisId: string,
  destinationId: string,
  intent: SelectionIntent,
): Promise<StartedDownload> {
  try {
    if (!isAnalysisId(analysisId)
      || !isAnalysisId(destinationId)
      || !isSelectionIntent(intent)) {
      throw new DownloadCommandError("invalid_request");
    }
    const response = await invoke<unknown>("start_download", {
      analysisId,
      destinationId,
      intent,
    });
    if (!isStartedDownload(response)) throw new DownloadCommandError("unknown");
    return response;
  } catch (error) {
    if (error instanceof DownloadCommandError) throw error;
    throw publicError(error);
  }
}

async function invokeJobCommand(command: string, jobId: string): Promise<void> {
  try {
    if (!isAnalysisId(jobId)) throw new DownloadCommandError("invalid_request");
    const response = await invoke<unknown>(command, { jobId });
    if (response !== null) throw new DownloadCommandError("unknown");
  } catch (error) {
    if (error instanceof DownloadCommandError) throw error;
    throw publicError(error);
  }
}

export function cancelDownload(jobId: string): Promise<void> {
  return invokeJobCommand("cancel_download", jobId);
}

export function openCompletedLocation(jobId: string): Promise<void> {
  return invokeJobCommand("open_completed_location", jobId);
}

export async function getDownloadResult(jobId: string): Promise<DownloadResult> {
  try {
    if (!isAnalysisId(jobId)) throw new DownloadCommandError("invalid_request");
    const response = await invoke<unknown>("get_download_result", { jobId });
    if (!isDownloadResult(response)) throw new DownloadCommandError("unknown");
    return response;
  } catch (error) {
    if (error instanceof DownloadCommandError) throw error;
    throw publicError(error);
  }
}

export async function listenDownloadProgress(
  receiver: (progress: DownloadProgress) => void,
): Promise<() => void> {
  let unlisten: UnlistenFn;
  try {
    unlisten = await listen<unknown>("download-progress", (event) => {
      if (!isDownloadProgress(event.payload)) {
        throw new DownloadCommandError("unknown");
      }
      receiver(event.payload);
    });
  } catch (error) {
    if (error instanceof DownloadCommandError) throw error;
    throw publicError(error);
  }
  return () => {
    unlisten();
  };
}
