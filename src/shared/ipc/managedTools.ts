import { invoke } from "@tauri-apps/api/core";
import { isToolInventory, type ToolInventory } from "../contracts/managedTools";

export async function getToolInventory(): Promise<ToolInventory> {
  const response = await invoke<unknown>("get_tool_inventory");

  if (!isToolInventory(response)) {
    throw new Error("Invalid tool inventory response");
  }

  return response;
}
