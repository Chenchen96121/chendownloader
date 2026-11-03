import { invoke } from "@tauri-apps/api/core";
import { isDownloadQueueSnapshot, type DownloadQueueSnapshot } from "../contracts/downloadQueue";
import { downloadErrorCodes, type DownloadErrorCode } from "../contracts/errors";
import { DownloadCommandError } from "./downloads";
async function read(command: string, args?: Record<string, unknown>): Promise<DownloadQueueSnapshot> {
  try {
    const response = await invoke<unknown>(command, args);
    if (!isDownloadQueueSnapshot(response)) throw new DownloadCommandError("unknown");
    return response;
  } catch (error) {
    if (error instanceof DownloadCommandError) throw error;
    throw new DownloadCommandError((downloadErrorCodes as readonly unknown[]).includes(error) ? error as DownloadErrorCode : "unknown");
  }
}
export function getDownloadQueue(): Promise<DownloadQueueSnapshot> { return read("get_download_queue"); }
export function setDownloadConcurrency(limit: number): Promise<DownloadQueueSnapshot> {
  if (!Number.isInteger(limit) || limit < 1 || limit > 4) return Promise.reject(new DownloadCommandError("invalid_request"));
  return read("set_download_concurrency", { request: { limit } });
}
