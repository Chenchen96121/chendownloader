import { invoke } from "@tauri-apps/api/core";
import { downloadErrorCodes, isDestinationApproval, type DestinationApproval } from "../contracts/download";
import { DownloadIpcError } from "./destination";

export type DefaultDestinationStatus = { state: "missing" | "unavailable" } | { state: "ready"; displayName: string };
function isStatus(value: unknown): value is DefaultDestinationStatus {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const prototype = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null) return false;
  const record = value as Record<string, unknown>;
  if (record.state === "missing" || record.state === "unavailable") return Object.keys(record).length === 1;
  return record.state === "ready" && Object.keys(record).length === 2
    && typeof record.displayName === "string" && !/[\\/:]/.test(record.displayName)
    && isDestinationApproval({ destinationId: "1", displayName: record.displayName });
}
async function call(command: string): Promise<unknown> {
  try { return await invoke<unknown>(command); }
  catch (error) { throw new DownloadIpcError(downloadErrorCodes.includes(error as never) ? error as DownloadIpcError["code"] : "unknown"); }
}
export async function loadDefaultDestination(): Promise<DefaultDestinationStatus> {
  const value = await call("load_default_destination");
  if (!isStatus(value)) throw new DownloadIpcError("unknown");
  return value;
}
export async function chooseDefaultDestination(): Promise<DefaultDestinationStatus | null> {
  const value = await call("choose_default_destination");
  if (value === null) return null;
  if (!isStatus(value) || value.state !== "ready") throw new DownloadIpcError("unknown");
  return value;
}
export async function approveDefaultDestination(): Promise<DestinationApproval> {
  const value = await call("approve_default_destination");
  if (!isDestinationApproval(value)) throw new DownloadIpcError("unknown");
  return value;
}
