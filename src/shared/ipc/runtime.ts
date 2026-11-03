import { invoke } from "@tauri-apps/api/core";
import { isRuntimeStatus, type RuntimeStatus } from "../contracts/runtime";

export async function getRuntimeStatus(): Promise<RuntimeStatus> {
  const response = await invoke<unknown>("get_runtime_status");

  if (!isRuntimeStatus(response)) {
    throw new Error("Invalid runtime status response");
  }

  return response;
}
