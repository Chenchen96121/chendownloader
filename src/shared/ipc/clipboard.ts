import {invoke} from "@tauri-apps/api/core";
const codes = ["empty", "unsupported_content", "too_long", "invalid_text", "unavailable"] as const;
export type ClipboardErrorCode = typeof codes[number];
export class ClipboardIpcError extends Error {
  constructor(readonly code: ClipboardErrorCode) { super("Clipboard unavailable"); }
}
export async function readClipboardText(): Promise<string> {
  try {
    const text = await invoke<unknown>("read_clipboard_text");
    if (typeof text !== "string" || text === "" || text !== text.trim() || text.length > 2048 || new TextEncoder().encode(text).length > 2048 || /[\p{Cc}\u2028\u2029]/u.test(text)) throw new ClipboardIpcError("unavailable");
    return text;
  } catch (error) {
    throw new ClipboardIpcError(typeof error === "string" && codes.some((code) => code === error) ? error as ClipboardErrorCode : "unavailable");
  }
}
