import { isAnalysisId } from "./liveAnalysis";
import { downloadErrorCodes, type DownloadErrorCode } from "./errors";
export const queuePhases = ["queued", "preparing", "downloading", "processing", "verifying", "publishing", "completed", "partial", "failed", "canceled"] as const;
export type QueuePhase = typeof queuePhases[number];
export type QueuePlatform = "youtube" | "instagram" | "x" | "bilibili";
export interface DownloadTask {
  jobId: string; platform: QueuePlatform; title: string; state: QueuePhase;
  percent: number | null; errorCode: DownloadErrorCode | null; displayName: string | null;
}
export interface DownloadQueueSnapshot { limit: number; tasks: DownloadTask[] }
export function isQueueTerminal(state: QueuePhase): boolean {
  return state === "completed" || state === "partial" || state === "failed" || state === "canceled";
}
const errors = new Set<unknown>(downloadErrorCodes);
function record(value: unknown, keys: string[]): value is Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const proto = Object.getPrototypeOf(value);
  return (proto === Object.prototype || proto === null) && Object.keys(value).length === keys.length && keys.every(key => Object.hasOwn(value, key));
}
export function isDownloadTask(value: unknown): value is DownloadTask {
  return record(value, ["jobId", "platform", "title", "state", "percent", "errorCode", "displayName"])
    && isAnalysisId(value.jobId) && (value.platform === "youtube" || value.platform === "instagram" || value.platform === "x" || value.platform === "bilibili")
    && typeof value.title === "string" && value.title.trim().length > 0 && Array.from(value.title).length <= 512 && !/\p{Cc}/u.test(value.title)
    && (queuePhases as readonly unknown[]).includes(value.state)
    && (value.percent === null || (typeof value.percent === "number" && Number.isFinite(value.percent) && value.percent >= 0 && value.percent <= 100))
    && (value.errorCode === null || errors.has(value.errorCode))
    && (value.displayName === null || (typeof value.displayName === "string" && value.displayName.trim().length > 0 && Array.from(value.displayName).length <= 255 && !/[<>:"/\\|?*\p{Cc}\u200e\u200f\u202a-\u202e\u2066-\u2069]/u.test(value.displayName)));
}
export function isDownloadQueueSnapshot(value: unknown): value is DownloadQueueSnapshot {
  if (!record(value, ["limit", "tasks"]) || typeof value.limit !== "number" || !Number.isInteger(value.limit) || value.limit < 1 || value.limit > 4
    || !Array.isArray(value.tasks) || value.tasks.length > 132 || !value.tasks.every(isDownloadTask)) return false;
  return new Set(value.tasks.map(task => task.jobId)).size === value.tasks.length;
}
