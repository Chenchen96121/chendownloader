import { describe, expect, it } from "vitest";
import type { DownloadProgress } from "../../shared/contracts/download";
import {
  applyDownloadProgress,
  applyDownloadResult,
  beginDestinationSelection,
  beginDownloadStart,
  canStartDownload,
  createDownloadUiState,
  failDownloadStart,
  finishDownloadStart,
  finishDestinationSelection,
  invalidateDownloadSelection,
  isDownloadActive,
  isRetryableDownloadError,
  requestDownloadCancel,
  retryDownload,
  refreshSavedDownloadDestination,
  selectDownloadDestination,
  synchronizeDownloadAnalysis,
} from "./downloadState";

const destination = { destinationId: "4", displayName: "下载" };

function progress(state: DownloadProgress["state"], overrides: Partial<DownloadProgress> = {}): DownloadProgress {
  return {
    jobId: "8",
    state,
    downloadedBytes: null,
    totalBytes: null,
    percent: null,
    speedBytesPerSecond: null,
    etaSeconds: null,
    messageKey: `download.${state}`,
    ...overrides,
  };
}

function activeState() {
  let state = selectDownloadDestination(synchronizeDownloadAnalysis(createDownloadUiState(), "7"), destination);
  const begun = beginDownloadStart(state);
  state = finishDownloadStart(begun.state, begun.generation, "8");
  return state;
}

describe("download UI state", () => {
  it.each(["destination_unavailable", "disk_full", "publication_failed"] as const)("recovers %s after Settings saves and approves a replacement folder", (code) => {
    const ready = selectDownloadDestination(createDownloadUiState("7"), destination);
    const begun = beginDownloadStart(ready);
    const failed = failDownloadStart(begun.state, begun.generation, code);
    const replacement = {destinationId: "9", displayName: "新目录"};
    const routine = refreshSavedDownloadDestination(failed, replacement, false);
    expect(routine.phase).toBe("failed");
    const recovered = refreshSavedDownloadDestination(failed, replacement, true);
    expect(recovered).toMatchObject({phase: "idle", errorCode: null, destination: replacement});
    expect(canStartDownload(recovered, true)).toBe(true);
    expect(beginDownloadStart(recovered).state.phase).toBe("starting");
  });

  it("default approval refresh preserves an active job and completed result", () => {
    const active = activeState();
    expect(refreshSavedDownloadDestination(active, destination, true)).toBe(active);
    const completed = {...active, phase: "completed" as const, displayName: "result.mp4"};
    expect(refreshSavedDownloadDestination(completed, destination, true)).toMatchObject({phase: "completed", displayName: "result.mp4", jobId: "8"});
    const networkFailure = {...active, phase: "failed" as const, errorCode: "network_failed" as const};
    expect(refreshSavedDownloadDestination(networkFailure, destination, true)).toMatchObject({phase: "failed", errorCode: "network_failed"});
  });
  it("keeps a destination when the picker is canceled and safely replaces it", () => {
    const ready = synchronizeDownloadAnalysis(createDownloadUiState(), "7");
    const selected = selectDownloadDestination(ready, destination);
    expect(selectDownloadDestination(selected, null)).toBe(selected);
    expect(selectDownloadDestination(selected, { destinationId: "5", displayName: "桌面" })).toMatchObject({
      destination: { destinationId: "5", displayName: "桌面" },
      phase: "idle",
      jobId: null,
    });
    const choosing = beginDestinationSelection(selected);
    expect(canStartDownload(choosing, true)).toBe(false);
    expect(finishDestinationSelection(choosing, "7", null)).toMatchObject({
      destination,
      destinationPending: false,
    });
  });

  it("starts only with current analysis, selection, and destination readiness", () => {
    const initial = createDownloadUiState();
    const analyzed = synchronizeDownloadAnalysis(initial, "7");
    const ready = selectDownloadDestination(analyzed, destination);
    expect(canStartDownload(initial, true)).toBe(false);
    expect(canStartDownload(analyzed, true)).toBe(false);
    expect(canStartDownload(ready, false)).toBe(false);
    expect(canStartDownload(ready, true)).toBe(true);
  });

  it("correlates start generations and rejects late start responses", () => {
    const state = selectDownloadDestination(synchronizeDownloadAnalysis(createDownloadUiState(), "7"), destination);
    const first = beginDownloadStart(state);
    const failed = failDownloadStart(first.state, first.generation, "network_failed");
    const second = beginDownloadStart(retryDownload(failed));
    expect(finishDownloadStart(second.state, first.generation, "8")).toBe(second.state);
    expect(failDownloadStart(second.state, first.generation, "network_failed")).toBe(second.state);
  });

  it("keeps byte and percentage progress monotonic and supports missing totals", () => {
    const started = activeState();
    const first = applyDownloadProgress(started, progress("downloading", {
      downloadedBytes: 500,
      totalBytes: 1000,
      percent: 50,
    }));
    const regressed = applyDownloadProgress(first, progress("downloading", {
      downloadedBytes: 400,
      totalBytes: null,
      percent: 40,
    }));
    expect(regressed).toMatchObject({ downloadedBytes: 500, totalBytes: 1000, percent: 50 });
    const unknown = applyDownloadProgress(activeState(), progress("downloading", { downloadedBytes: 25 }));
    expect(unknown).toMatchObject({ downloadedBytes: 25, totalBytes: null, percent: null });
  });

  it("advances through processing and verifying but ignores stale jobs and regressions", () => {
    const started = activeState();
    expect(applyDownloadProgress(started, progress("downloading", { jobId: "9" }))).toBe(started);
    const processing = applyDownloadProgress(started, progress("processing"));
    expect(processing.phase).toBe("processing");
    expect(applyDownloadProgress(processing, progress("downloading"))).toBe(processing);
    expect(applyDownloadProgress(processing, progress("verifying")).phase).toBe("verifying");
  });

  it("requests cancellation only for the matching active job", () => {
    const started = activeState();
    expect(requestDownloadCancel(started, "9")).toBe(started);
    const canceling = requestDownloadCancel(started, "8");
    expect(canceling.cancelRequested).toBe(true);
    expect(isDownloadActive(canceling)).toBe(true);
  });

  it.each(["completed", "failed", "canceled"] as const)("settles %s exactly once and resets for another deliberate run", (terminal) => {
    const terminalProgress = applyDownloadProgress(activeState(), progress(terminal));
    const result = terminal === "completed"
      ? { jobId: "8", state: terminal, errorCode: null, displayName: "示例.mp4" } as const
      : terminal === "failed"
        ? { jobId: "8", state: terminal, errorCode: "network_failed", displayName: null } as const
        : { jobId: "8", state: terminal, errorCode: null, displayName: null } as const;
    expect(applyDownloadProgress(terminalProgress, progress(terminal))).toBe(terminalProgress);
    const settled = applyDownloadResult(terminalProgress, result);
    expect(applyDownloadResult(settled, result)).toBe(settled);
    const reset = retryDownload(settled);
    expect(reset).toMatchObject({ phase: "idle", jobId: null, analysisId: "7", destination });
  });

  it("invalidates destination and terminal data for a different analysis without modeling locale", () => {
    const completed = applyDownloadResult(
      applyDownloadProgress(activeState(), progress("completed")),
      { jobId: "8", state: "completed", errorCode: null, displayName: "示例.mp4" },
    );
    const replacement = synchronizeDownloadAnalysis(completed, "12");
    expect(replacement).toMatchObject({ analysisId: "12", destination: null, phase: "idle", displayName: null });
    expect(replacement).not.toHaveProperty("locale");
  });

  it("offers retry only for deliberate transient failures", () => {
    expect(isRetryableDownloadError("network_failed")).toBe(true);
    expect(isRetryableDownloadError("rate_limited")).toBe(true);
    expect(isRetryableDownloadError("download_timeout")).toBe(true);
    expect(isRetryableDownloadError("authentication_required")).toBe(false);
    expect(isRetryableDownloadError("disk_full")).toBe(false);
  });
});
