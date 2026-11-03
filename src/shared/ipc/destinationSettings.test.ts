import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { loadDefaultDestination, chooseDefaultDestination, approveDefaultDestination } from "./destinationSettings";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const nativeInvoke = vi.mocked(invoke);
beforeEach(() => { nativeInvoke.mockReset(); });
it("loads only path-free native status and accepts cancellation", async () => {
  nativeInvoke.mockResolvedValueOnce({ state: "ready", displayName: "Downloads" });
  expect(await loadDefaultDestination()).toEqual({ state: "ready", displayName: "Downloads" });
  expect(nativeInvoke).toHaveBeenCalledExactlyOnceWith("load_default_destination");
  nativeInvoke.mockReset(); nativeInvoke.mockResolvedValue(null);
  expect(await chooseDefaultDestination()).toBeNull();
  expect(nativeInvoke).toHaveBeenCalledExactlyOnceWith("choose_default_destination");
});
it.each([{}, { state: "ready", displayName: "Downloads", path: "C:\\Private" }, { state: "ready", displayName: "C:\\Private" }, { state: "ready", displayName: "" }, { state: "missing", path: "private" }])("rejects status %j", async (value) => {
  nativeInvoke.mockResolvedValue(value);
  await expect(loadDefaultDestination()).rejects.toMatchObject({ code: "unknown" });
});
it("obtains a fresh strict approval without arguments or saved paths", async () => {
  nativeInvoke.mockResolvedValue({ destinationId: "2", displayName: "Downloads" });
  expect(await approveDefaultDestination()).toEqual({ destinationId: "2", displayName: "Downloads" });
  expect(nativeInvoke).toHaveBeenCalledExactlyOnceWith("approve_default_destination");
  nativeInvoke.mockResolvedValue({ destinationId: "2", displayName: "Downloads", path: "PRIVATE" });
  await expect(approveDefaultDestination()).rejects.toMatchObject({ code: "unknown" });
});
it("never exposes private native rejection details", async () => {
  nativeInvoke.mockRejectedValue(new Error("C:\\PRIVATE signed URL"));
  const failure = await chooseDefaultDestination().catch((error: unknown) => error);
  expect((failure as {code: string}).code).toBe("unknown");
  expect((failure as Error).message).toBe("Download operation unavailable");
});
