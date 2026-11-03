import { afterEach, expect, it, vi } from "vitest";
import { getDownloadQueue, setDownloadConcurrency } from "./downloadQueue";
const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));
afterEach(() => invoke.mockReset());
it("uses closed read and save commands and refuses invalid settings before IPC", async () => {
  invoke.mockResolvedValue({ limit: 2, tasks: [] });
  expect(await getDownloadQueue()).toEqual({ limit: 2, tasks: [] });
  expect(invoke).toHaveBeenLastCalledWith("get_download_queue", undefined);
  await setDownloadConcurrency(4);
  expect(invoke).toHaveBeenLastCalledWith("set_download_concurrency", { request: { limit: 4 } });
  for (const invalid of [0, 5, 1.5, NaN]) await expect(setDownloadConcurrency(invalid)).rejects.toMatchObject({ code: "invalid_request" });
  expect(invoke).toHaveBeenCalledTimes(2);
});
it("does not expose malformed snapshots or raw native diagnostics", async () => {
  invoke.mockResolvedValue({ limit: 2, tasks: [], path: "C:\\private" });
  await expect(getDownloadQueue()).rejects.toMatchObject({ code: "unknown" });
  invoke.mockRejectedValue("C:\\private token");
  await expect(getDownloadQueue()).rejects.toMatchObject({ code: "unknown" });
});
