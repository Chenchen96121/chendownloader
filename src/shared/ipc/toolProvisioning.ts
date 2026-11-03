import { invoke } from "@tauri-apps/api/core";
import {
  isToolProvisioningErrorCode,
  isToolProvisioningStatus,
  type ToolProvisioningStatus,
} from "../contracts/toolProvisioning";

async function invokeStatus(command: "get_tool_provisioning_status" | "repair_managed_tools") {
  try {
    const response = await invoke<unknown>(command);
    if (!isToolProvisioningStatus(response)) {
      throw new Error("Invalid tool provisioning status");
    }
    return response;
  } catch (error) {
    if (isToolProvisioningErrorCode(error)) throw error;
    throw error instanceof Error ? error : new Error("Invalid tool provisioning response");
  }
}

export function getToolProvisioningStatus(): Promise<ToolProvisioningStatus> {
  return invokeStatus("get_tool_provisioning_status");
}

export function repairManagedTools(): Promise<ToolProvisioningStatus> {
  return invokeStatus("repair_managed_tools");
}
