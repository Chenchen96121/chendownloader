import { beforeEach, expect, it, vi } from "vitest";
import { approveDefaultDestination } from "../../shared/ipc/destinationSettings";
import { startDownload } from "../../shared/ipc/downloads";
import { startDownloadInDefaultFolder } from "./defaultDestinationStart";
vi.mock("../../shared/ipc/destinationSettings", () => ({ approveDefaultDestination: vi.fn() }));
vi.mock("../../shared/ipc/downloads", () => ({ startDownload: vi.fn() }));
beforeEach(() => vi.clearAllMocks());
it("uses a newly revalidated folder for every start, not the previous approval", async () => {
  vi.mocked(approveDefaultDestination).mockResolvedValueOnce({ destinationId: "7", displayName: "New Folder" }).mockResolvedValueOnce({ destinationId: "8", displayName: "New Folder" });
  vi.mocked(startDownload).mockResolvedValue({ jobId: "1" });
  const intent = { mode: "video_only" as const, videoFormatId: "720" };
  expect(await startDownloadInDefaultFolder("1", intent)).toEqual({ started: { jobId: "1" }, destination: { destinationId: "7", displayName: "New Folder" } });
  await startDownloadInDefaultFolder("1", intent);
  expect(startDownload).toHaveBeenNthCalledWith(1, "1", "7", intent);
  expect(startDownload).toHaveBeenNthCalledWith(2, "1", "8", intent);
});
it("does not start when the stored folder is no longer available", async () => {
  vi.mocked(approveDefaultDestination).mockRejectedValue("destination_unavailable");
  await expect(startDownloadInDefaultFolder("1", { mode: "best_available" })).rejects.toBe("destination_unavailable");
  expect(startDownload).not.toHaveBeenCalled();
});
