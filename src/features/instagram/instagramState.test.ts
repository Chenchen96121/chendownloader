import { describe, expect, it } from "vitest";
import type {
  InstagramAnalysisPreview,
  InstagramDownloadProgress,
  InstagramDownloadResult,
} from "../../shared/contracts/instagram";
import {
  buildInstagramDownloadIntent,
  createInstagramWorkspaceState,
  instagramCompatibilityApplies,
  instagramWorkspaceReducer,
  isInstagramWorkspaceLocked,
} from "./instagramState";

const capabilities = {
  postBatch: true, images: true, videos: true, compatibility: true,
  clip: false, manualQuality: false, audioOnly: false, videoOnly: false,
} as const;
const analysis: InstagramAnalysisPreview = {
  analysisId: "1", sourceKind: "post", shortcode: "Abc_123-", title: "Post",
  sourcePageUrl: "https://www.instagram.com/p/Abc_123-/", capabilities,
  items: [
    { itemId: "11", ordinal: 1, kind: "image", container: "jpg", previewAvailable: true, availability: "ready" },
    { itemId: "12", ordinal: 2, kind: "video", container: "mp4", previewAvailable: false, availability: "ready" },
    { itemId: "13", ordinal: 3, kind: "video", container: "mp4", previewAvailable: false, availability: "unavailable", unavailableReason: "content_changed" },
  ],
};

function analyzedState() {
  let state = createInstagramWorkspaceState();
  state = instagramWorkspaceReducer(state, { type: "urlChanged", url: "https://www.instagram.com/p/Abc_123-/" });
  state = instagramWorkspaceReducer(state, { type: "analysisStarted", generation: 2 });
  return instagramWorkspaceReducer(state, { type: "analysisSucceeded", generation: 2, analysis });
}

function progress(state: InstagramDownloadProgress["state"], overrides: Partial<InstagramDownloadProgress> = {}): InstagramDownloadProgress {
  return {
    jobId: "8", state, itemIndex: 1, itemCount: 2,
    downloadedBytes: null, totalBytes: null, percent: null, ...overrides,
  };
}

describe("Instagram workspace reducer", () => {
  it.each(["previewRequested", "previewLoaded", "previewFailed"] as const)("preserves download failures when %s arrives", (type) => {
    let state = instagramWorkspaceReducer(analyzedState(), { type: "previewRequested", analysisId: "1", itemId: "11", requestId: 1 });
    state = instagramWorkspaceReducer(state, { type: "downloadStartRequested", generation: 3 });
    state = instagramWorkspaceReducer(state, { type: "downloadStartFailed", generation: 3, errorCode: "destination_unavailable" });
    const next = instagramWorkspaceReducer(state, type === "previewRequested"
      ? { type, analysisId: "1", itemId: "11", requestId: 2 }
      : type === "previewLoaded"
        ? { type, analysisId: "1", itemId: "11", requestId: 1, preview: { mimeType: "image/jpeg", dataBase64: "AAAA" } }
        : { type, analysisId: "1", itemId: "11", requestId: 1, errorCode: "network_failed" });
    expect(next.error).toEqual({ scope: "download", code: "destination_unavailable" });
    if (type === "previewFailed") expect(next.previews["11"]).toMatchObject({ status: "error", errorCode: "network_failed" });
  });

  it("starts empty and selects all ready items after analysis", () => {
    expect(createInstagramWorkspaceState()).toMatchObject({
      url: "", analysis: null, selectedItemIds: [], compatibilityMode: false,
      previews: {}, requestGeneration: 0, job: null, result: null, error: null,
    });
    expect(analyzedState().selectedItemIds).toEqual(["11", "12"]);
  });

  it("toggles, selects all, and clears only ready items in platform order", () => {
    const state = analyzedState();
    const one = instagramWorkspaceReducer(state, { type: "selectionToggled", itemId: "12" });
    expect(one.selectedItemIds).toEqual(["11"]);
    expect(instagramWorkspaceReducer(one, { type: "selectionToggled", itemId: "13" })).toBe(one);
    expect(instagramWorkspaceReducer(one, { type: "selectionAll" }).selectedItemIds).toEqual(["11", "12"]);
    expect(instagramWorkspaceReducer(state, { type: "selectionCleared" }).selectedItemIds).toEqual([]);
  });

  it("retains the opt-in while image-only selection disables its effect", () => {
    let state = instagramWorkspaceReducer(analyzedState(), { type: "compatibilityChanged", value: true });
    state = instagramWorkspaceReducer(state, { type: "selectionToggled", itemId: "12" });
    expect(state.compatibilityMode).toBe(true);
    expect(instagramCompatibilityApplies(state)).toBe(false);
    expect(buildInstagramDownloadIntent(state)).toEqual({ itemIds: ["11"], compatibilityMode: false });
    state = instagramWorkspaceReducer(state, { type: "selectionToggled", itemId: "12" });
    expect(buildInstagramDownloadIntent(state)).toEqual({ itemIds: ["11", "12"], compatibilityMode: true });
  });

  it("correlates preview requests and ignores late data or errors", () => {
    let state = analyzedState();
    state = instagramWorkspaceReducer(state, { type: "previewRequested", analysisId: "1", itemId: "11", requestId: 1 });
    state = instagramWorkspaceReducer(state, { type: "previewRequested", analysisId: "1", itemId: "11", requestId: 2 });
    const late = instagramWorkspaceReducer(state, {
      type: "previewLoaded", analysisId: "1", itemId: "11", requestId: 1,
      preview: { mimeType: "image/jpeg", dataBase64: "AAAA" },
    });
    expect(late).toBe(state);
    const failed = instagramWorkspaceReducer(state, {
      type: "previewFailed", analysisId: "1", itemId: "11", requestId: 2,
      errorCode: "network_failed",
    });
    expect(failed.previews["11"]).toMatchObject({ status: "error", errorCode: "network_failed" });
  });

  it("clears prior selection, previews, results, and compatibility for every new analysis", () => {
    let state = instagramWorkspaceReducer(analyzedState(), {
      type: "compatibilityChanged",
      value: true,
    });
    state = instagramWorkspaceReducer(state, {
      type: "previewRequested",
      analysisId: "1",
      itemId: "11",
      requestId: 1,
    });
    state = instagramWorkspaceReducer(state, {
      type: "analysisStarted",
      generation: state.requestGeneration + 1,
    });
    expect(state).toMatchObject({
      analysisStatus: "analyzing",
      analysis: null,
      selectedItemIds: [],
      compatibilityMode: false,
      previews: {},
      job: null,
      result: null,
      error: null,
    });
  });

  it("tracks per-item progress with unknown sizes and accepts partial results", () => {
    let state = analyzedState();
    state = instagramWorkspaceReducer(state, { type: "downloadStartRequested", generation: 3 });
    state = instagramWorkspaceReducer(state, { type: "downloadStarted", generation: 3, jobId: "8", itemCount: 2 });
    state = instagramWorkspaceReducer(state, { type: "downloadProgress", progress: progress("downloading") });
    expect(state.job).toMatchObject({ state: "downloading", downloadedBytes: null, totalBytes: null, percent: null });
    state = instagramWorkspaceReducer(state, { type: "downloadProgress", progress: progress("publishing", { itemIndex: 2 }) });
    state = instagramWorkspaceReducer(state, { type: "downloadProgress", progress: progress("partial", { itemIndex: 2 }) });
    const result: InstagramDownloadResult = {
      jobId: "8", state: "partial", completedCount: 1, failedCount: 1,
      folderDisplayName: "creator [Abc_123-]",
      items: [
        { itemId: "11", ordinal: 1, kind: "image", state: "completed", displayName: "01.jpg" },
        { itemId: "12", ordinal: 2, kind: "video", state: "failed", errorCode: "content_changed" },
      ],
    };
    state = instagramWorkspaceReducer(state, { type: "downloadFinished", result });
    expect(state.result).toEqual(result);
    expect(state.job?.state).toBe("partial");
    expect(isInstagramWorkspaceLocked(state)).toBe(false);
  });

  it("locks source and selection while active, then clears all stale state for a new URL", () => {
    let state = analyzedState();
    state = instagramWorkspaceReducer(state, { type: "downloadStartRequested", generation: 3 });
    expect(isInstagramWorkspaceLocked(state)).toBe(true);
    expect(instagramWorkspaceReducer(state, { type: "selectionCleared" })).toBe(state);
    expect(instagramWorkspaceReducer(state, { type: "urlChanged", url: "new" })).toBe(state);
    state = instagramWorkspaceReducer(state, { type: "downloadStartFailed", generation: 3, errorCode: "network_failed" });
    const replacement = instagramWorkspaceReducer(state, { type: "urlChanged", url: "new" });
    expect(replacement).toMatchObject({
      url: "new", analysis: null, selectedItemIds: [], compatibilityMode: false,
      previews: {}, job: null, result: null, error: null,
    });
    expect(replacement.requestGeneration).toBeGreaterThan(state.requestGeneration);
  });

  it("keeps canceled jobs terminal when late progress arrives", () => {
    let state = analyzedState();
    state = instagramWorkspaceReducer(state, { type: "downloadStartRequested", generation: 3 });
    state = instagramWorkspaceReducer(state, {
      type: "downloadStarted",
      generation: 3,
      jobId: "8",
      itemCount: 2,
    });
    state = instagramWorkspaceReducer(state, {
      type: "downloadProgress",
      progress: progress("canceled"),
    });
    const canceled = state;
    state = instagramWorkspaceReducer(state, {
      type: "downloadProgress",
      progress: progress("downloading", {
        downloadedBytes: 10,
        totalBytes: 20,
        percent: 50,
      }),
    });
    expect(state).toBe(canceled);
    expect(state.job?.state).toBe("canceled");
    expect(isInstagramWorkspaceLocked(state)).toBe(false);
  });
});
