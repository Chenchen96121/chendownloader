import { isAnalysisId } from "./liveAnalysis";

export { downloadErrorCodes, type DownloadErrorCode } from "./errors";

export const downloadStates = [
  "preparing",
  "downloading",
  "processing",
  "verifying",
  "completed",
  "failed",
  "canceled",
] as const;

export type DownloadState = (typeof downloadStates)[number];

export const downloadMessageKeys = [
  "download.preparing",
  "download.downloading",
  "download.processing",
  "download.verifying",
  "download.completed",
  "download.failed",
  "download.canceled",
] as const;

export type DownloadMessageKey = (typeof downloadMessageKeys)[number];

export interface DestinationApproval {
  destinationId: string;
  displayName: string;
}

export interface DownloadProgress {
  jobId: string;
  state: DownloadState;
  downloadedBytes: number | null;
  totalBytes: number | null;
  percent: number | null;
  speedBytesPerSecond: number | null;
  etaSeconds: number | null;
  messageKey: DownloadMessageKey;
}

function plainExactRecord(value: unknown, fields: readonly string[]): value is Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const prototype = Object.getPrototypeOf(value);
  const candidate = value as Record<string, unknown>;
  return (prototype === Object.prototype || prototype === null)
    && Object.keys(candidate).length === fields.length
    && fields.every((field) => Object.hasOwn(candidate, field));
}

function isSafeDisplayName(value: unknown): value is string {
  return typeof value === "string"
    && value.trim().length > 0
    && [...value].length <= 80
    && !/\p{Cc}|[\u{200e}\u{200f}\u{202a}-\u{202e}\u{2066}-\u{2069}]/u.test(value);
}

export function isDestinationApproval(value: unknown): value is DestinationApproval {
  if (!plainExactRecord(value, ["destinationId", "displayName"])) return false;
  return isAnalysisId(value.destinationId) && isSafeDisplayName(value.displayName);
}

function isNullableSafeInteger(value: unknown, allowZero: boolean): value is number | null {
  return value === null || (typeof value === "number"
    && Number.isSafeInteger(value)
    && (allowZero ? value >= 0 : value > 0));
}

export function isDownloadProgress(value: unknown): value is DownloadProgress {
  if (!plainExactRecord(value, [
    "jobId", "state", "downloadedBytes", "totalBytes", "percent",
    "speedBytesPerSecond", "etaSeconds", "messageKey",
  ])) return false;
  if (!isAnalysisId(value.jobId)
    || !downloadStates.includes(value.state as DownloadState)
    || !downloadMessageKeys.includes(value.messageKey as DownloadMessageKey)
    || value.messageKey !== `download.${String(value.state)}`
    || !isNullableSafeInteger(value.downloadedBytes, true)
    || !isNullableSafeInteger(value.totalBytes, false)
    || !(value.percent === null || (typeof value.percent === "number" && Number.isFinite(value.percent) && value.percent >= 0 && value.percent <= 100))
    || !(value.speedBytesPerSecond === null || (typeof value.speedBytesPerSecond === "number" && Number.isFinite(value.speedBytesPerSecond) && value.speedBytesPerSecond >= 0 && value.speedBytesPerSecond <= Number.MAX_SAFE_INTEGER))
    || !isNullableSafeInteger(value.etaSeconds, true)
    || (typeof value.etaSeconds === "number" && value.etaSeconds > 31_536_000)) {
    return false;
  }
  return true;
}
