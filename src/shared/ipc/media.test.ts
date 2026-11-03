import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { previewMediaFixture } from "./media";
import type { Platform } from "../contracts/media";

// Only native transport is external; validation and the wrapper stay real.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const nativeInvoke = vi.mocked(invoke);
const response = {
  platform: "youtube", mediaId: "sample-01", title: "Synthetic sample",
  formats: [{ id: "c1", container: "mp4", kind: "combined", videoCodec: "avc1", audioCodec: "mp4a" }],
};

beforeEach(() => { nativeInvoke.mockReset(); });

describe("previewMediaFixture", () => {
  it.each(["youtube", "bilibili", "x", "instagram"] as const)("sends only closed platform %s and returns validated metadata", async (platform) => {
    nativeInvoke.mockResolvedValue({ ...response, platform });
    const result = await previewMediaFixture(platform);
    expect(nativeInvoke).toHaveBeenCalledExactlyOnceWith("preview_media_fixture", { platform });
    expect(result.platform).toBe(platform);
    expect(result.title).toBe("Synthetic sample");
    expect(result.formats).toEqual([{ id: "c1", container: "mp4", kind: "combined", videoCodec: "avc1", audioCodec: "mp4a" }]);
  });
  it("rejects runtime-invalid platform arguments without crossing IPC", async () => {
    await expect(previewMediaFixture("unknown" as Platform)).rejects.toThrow("Sample preview unavailable");
    expect(nativeInvoke).not.toHaveBeenCalled();
  });
  it.each([
    null, { ...response, formats: [] }, { ...response, url: "RAW_RESPONSE_SENTINEL" },
    { ...response, formats: [response.formats[0], response.formats[0]] },
    { ...response, platform: "x" },
  ])("rejects malformed or mismatched response %j", async (payload) => {
    nativeInvoke.mockResolvedValue(payload);
    await expect(previewMediaFixture("youtube")).rejects.toThrow("Sample preview unavailable");
  });
  it("does not expose native rejection details through its public error", async () => {
    nativeInvoke.mockRejectedValue(new Error("RAW_REJECTION_SENTINEL credentials/path"));
    try {
      await previewMediaFixture("youtube");
      expect.fail("Native rejection must not return success");
    } catch (error) {
      expect(error).toBeInstanceOf(Error);
      expect((error as Error).message).toBe("Sample preview unavailable");
      expect(String(error)).not.toContain("RAW_REJECTION_SENTINEL");
      expect((error as Error).cause).toBeUndefined();
    }
  });
});
