import { invoke } from "@tauri-apps/api/core";
import { isPlatform, type Platform } from "../contracts/media";
import { isSelectionIntent, isSelectionPreview, type SelectionIntent, type SelectionPreview } from "../contracts/selection";

export async function previewMediaSelectionFixture(platform: Platform, intent: SelectionIntent): Promise<SelectionPreview> {
  try {
    if (!isPlatform(platform) || !isSelectionIntent(intent)) throw new Error();
    const response = await invoke<unknown>("preview_media_selection_fixture", { platform, intent });
    if (!isSelectionPreview(response, platform, intent)) throw new Error();
    return response;
  } catch {
    // Native text, malformed data and causes must never cross the public boundary.
    throw new Error("Selection preview unavailable");
  }
}
