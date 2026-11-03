import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { chooseDestination, DownloadIpcError } from "./destination";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const nativeInvoke = vi.mocked(invoke);

beforeEach(() => nativeInvoke.mockReset());

describe("native destination IPC", () => {
  it("invokes only the narrow native picker command and accepts cancellation", async () => {
    nativeInvoke.mockResolvedValueOnce(null);
    await expect(chooseDestination()).resolves.toBeNull();
    expect(nativeInvoke).toHaveBeenCalledExactlyOnceWith("choose_destination");
  });

  it("returns a strict path-free approval", async () => {
    const approval = { destinationId: "8", displayName: "Downloads" };
    nativeInvoke.mockResolvedValueOnce(approval);
    await expect(chooseDestination()).resolves.toEqual(approval);
  });

  it.each([undefined, {}, [], { destinationId: "8", displayName: "Downloads", path: "C:\\Private" }])("rejects malformed response %j", async (response) => {
    nativeInvoke.mockResolvedValueOnce(response);
    await expect(chooseDestination()).rejects.toMatchObject({ name: "DownloadIpcError", code: "unknown", message: "Download operation unavailable" });
  });

  it("preserves only allowlisted native codes and hides raw rejection text", async () => {
    nativeInvoke.mockRejectedValueOnce("destination_unavailable");
    await expect(chooseDestination()).rejects.toEqual(new DownloadIpcError("destination_unavailable"));
    nativeInvoke.mockRejectedValueOnce(new Error("C:\\Users\\private\\secret failed"));
    const error = await chooseDestination().catch((reason: unknown) => reason);
    expect(error).toMatchObject({ name: "DownloadIpcError", code: "unknown", message: "Download operation unavailable" });
    expect(JSON.stringify(error)).not.toContain("private");
  });
});
