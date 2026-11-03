import { expect, it } from "vitest";
import { createDownloadUiState, detachSubmittedDownload, canStartDownload } from "./downloadState";
it("releases the YouTube selection after submit without changing the saved destination or analysis", () => {
  const before = { ...createDownloadUiState("7"), destination: { destinationId: "8", displayName: "Downloads" }, phase: "preparing" as const, jobId: "9", startGeneration: 2 };
  const after = detachSubmittedDownload(before, 2);
  expect(after.analysisId).toBe("7"); expect(after.destination).toEqual(before.destination);
  expect(after.jobId).toBeNull(); expect(canStartDownload(after, true)).toBe(true);
  expect(detachSubmittedDownload(before, 1)).toBe(before);
});
