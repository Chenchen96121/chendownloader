import { invoke } from "@tauri-apps/api/core";
import { isMediaInfo, isPlatform, type MediaInfo, type Platform } from "../contracts/media";

export async function previewMediaFixture(platform: Platform): Promise<MediaInfo> {
  try {
    if (!isPlatform(platform)) throw new Error();
    const response = await invoke<unknown>("preview_media_fixture", { platform });
    if (!isMediaInfo(response) || response.platform !== platform) throw new Error();
    return response;
  } catch {
    // Do not propagate raw native rejection text, malformed payloads or causes.
    throw new Error("Sample preview unavailable");
  }
}
