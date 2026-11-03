import {beforeEach, expect, it, vi} from "vitest";
import {invoke} from "@tauri-apps/api/core";
import {readClipboardText, ClipboardIpcError} from "./clipboard";
vi.mock("@tauri-apps/api/core", () => ({invoke: vi.fn()}));
const native = vi.mocked(invoke);
beforeEach(() => { native.mockReset(); });
it("reads only the closed native command and returns a single bounded string", async () => {
  native.mockResolvedValue("https://youtu.be/91YrsgWQcEo");
  expect(await readClipboardText()).toBe("https://youtu.be/91YrsgWQcEo");
  expect(native).toHaveBeenCalledExactlyOnceWith("read_clipboard_text");
});
it.each([null, {}, [], "", " x ", "a\nb", "a\0b", "a\u2028b", "x".repeat(2049), "中".repeat(683)])("rejects malformed native text", async (value) => {
  native.mockResolvedValue(value);
  await expect(readClipboardText()).rejects.toMatchObject({code: "unavailable"});
});
it.each(["empty", "unsupported_content", "too_long", "invalid_text", "unavailable"])("preserves only the closed code %s", async (code) => {
  native.mockRejectedValue(code);
  await expect(readClipboardText()).rejects.toMatchObject({code, message: "Clipboard unavailable"});
});
it("never exposes native clipboard text, paths, or causes in errors", async () => {
  native.mockRejectedValue(new Error("private clipboard contents", {cause: "private"}));
  const error = await readClipboardText().catch((error: unknown) => error);
  expect(error).toBeInstanceOf(ClipboardIpcError);
  expect(String(error)).not.toContain("private");
  expect((error as Error).cause).toBeUndefined();
});
