import { invoke } from "@tauri-apps/api/core";
import {
  downloadErrorCodes,
  isDestinationApproval,
  type DestinationApproval,
  type DownloadErrorCode,
} from "../contracts/download";

const errorCodeSet = new Set<unknown>(downloadErrorCodes);

export class DownloadIpcError extends Error {
  readonly code: DownloadErrorCode;

  constructor(code: DownloadErrorCode) {
    super("Download operation unavailable");
    this.name = "DownloadIpcError";
    this.code = code;
  }
}

function publicError(value: unknown): DownloadIpcError {
  return new DownloadIpcError(
    errorCodeSet.has(value) ? value as DownloadErrorCode : "unknown",
  );
}

export async function chooseDestination(): Promise<DestinationApproval | null> {
  try {
    const response = await invoke<unknown>("choose_destination");
    if (response === null) return null;
    if (!isDestinationApproval(response)) throw new DownloadIpcError("unknown");
    return response;
  } catch (error) {
    if (error instanceof DownloadIpcError) throw error;
    throw publicError(error);
  }
}
