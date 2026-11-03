import { describe, expect, it } from "vitest";
import {
  downloadErrorCodes,
  downloadMessageKeys,
  downloadStates,
  isDestinationApproval,
  isDownloadProgress,
} from "./download";

describe("download public contracts", () => {
  it("keeps exact closed public states, errors, and message keys", () => {
    expect(downloadStates).toEqual(["preparing", "downloading", "processing", "verifying", "completed", "failed", "canceled"]);
    expect(downloadErrorCodes).toEqual([
      "invalid_request", "analysis_unavailable", "destination_unavailable", "format_unavailable",
      "download_in_progress", "tool_missing", "tool_integrity_failed", "authentication_required",
      "access_denied", "content_unavailable", "content_changed", "rate_limited", "extractor_outdated",
      "network_failed", "download_timeout", "process_failed", "output_invalid",
      "image_content_invalid", "image_dimensions_mismatch", "image_size_mismatch", "image_probe_failed", "disk_full",
      "publication_failed", "unknown",
    ]);
    expect(downloadMessageKeys).toEqual(downloadStates.map((state) => `download.${state}`));
  });

  it("accepts only exact opaque destination approvals", () => {
    expect(isDestinationApproval({ destinationId: "1", displayName: "Downloads" })).toBe(true);
    for (const value of [
      null, [], {}, { destinationId: "0", displayName: "Downloads" },
      { destinationId: "01", displayName: "Downloads" },
      { destinationId: 1, displayName: "Downloads" },
      { destinationId: "1", displayName: "" },
      { destinationId: "1", displayName: "bad\nname" },
      { destinationId: "1", displayName: "x".repeat(81) },
      { destinationId: "1", displayName: "Downloads", path: "C:\\Private" },
    ]) expect(isDestinationApproval(value)).toBe(false);
    expect(isDestinationApproval(Object.create({ destinationId: "1", displayName: "Downloads" }))).toBe(false);
  });

  it("accepts correlated bounded progress without private fields", () => {
    expect(isDownloadProgress({
      jobId: "9", state: "downloading", downloadedBytes: 250, totalBytes: 1000,
      percent: 25, speedBytesPerSecond: 125.5, etaSeconds: 6, messageKey: "download.downloading",
    })).toBe(true);
    expect(isDownloadProgress({
      jobId: "9", state: "preparing", downloadedBytes: null, totalBytes: null,
      percent: null, speedBytesPerSecond: null, etaSeconds: null, messageKey: "download.preparing",
    })).toBe(true);
    const base = {
      jobId: "1", state: "downloading", downloadedBytes: null, totalBytes: null,
      percent: null, speedBytesPerSecond: null, etaSeconds: null, messageKey: "download.downloading",
    };
    for (const value of [
      null, [], {},
      { ...base, jobId: "0" },
      { ...base, downloadedBytes: -1, totalBytes: 100, percent: 0 },
      { ...base, downloadedBytes: 101, totalBytes: 100, percent: 101 },
      { ...base, downloadedBytes: Number.MAX_SAFE_INTEGER + 1 },
      { ...base, percent: Number.NaN },
      { ...base, speedBytesPerSecond: Number.POSITIVE_INFINITY },
      { ...base, etaSeconds: 31_536_001 },
      { ...base, messageKey: "download.verifying" },
      { ...base, path: "C:\\Private" },
    ]) expect(isDownloadProgress(value)).toBe(false);
    expect(isDownloadProgress(Object.create({
      jobId: "1", state: "preparing", downloadedBytes: null, totalBytes: null,
      percent: null, speedBytesPerSecond: null, etaSeconds: null, messageKey: "download.preparing",
    }))).toBe(false);
  });
});
