import { describe, expect, it } from "vitest";
import type {
  TikTokAnalysisPreview,
  TikTokDownloadProgress,
  TikTokDownloadResult,
} from "../../shared/contracts/tiktok";
import {
  buildTikTokDownloadIntent,
  createTikTokWorkspaceState,
  tiktokCompatibilityApplies,
  tiktokWorkspaceReducer,
  isTikTokWorkspaceLocked,
} from "./tiktokState";
import { readyTikTokState } from "./tiktokTestFixtures";

it("bounds decoded preview cache to 16 MiB without preloading evicted items",()=>{
 let state=readyTikTokState();
 for(const [index,id] of ["11","13","14"].entries()){
  state=tiktokWorkspaceReducer(state,{type:"previewRequested",analysisId:"1",itemId:id,requestId:index+1});
  state=tiktokWorkspaceReducer(state,{type:"previewLoaded",analysisId:"1",itemId:id,requestId:index+1,preview:{mimeType:"image/jpeg",dataBase64:"A".repeat(8*1024*1024)}});
 }
 const bytes=Object.values(state.previews).reduce((sum,p)=>sum+(p.preview?.dataBase64.length??0)*3/4,0);
 expect(bytes).toBeLessThanOrEqual(16*1024*1024);
 expect(state.previews["11"]?.preview).toBeUndefined();
});

it("keeps only two previews even when compressed payloads are tiny",()=>{
 let state=readyTikTokState();
 for(const [index,id]of ["11","13","14"].entries()){
  state=tiktokWorkspaceReducer(state,{type:"previewRequested",analysisId:"1",itemId:id,requestId:index+1});
  state=tiktokWorkspaceReducer(state,{type:"previewLoaded",analysisId:"1",itemId:id,requestId:index+1,preview:{mimeType:"image/jpeg",dataBase64:"AQID"}});
 }
 expect(Object.values(state.previews).filter(p=>p.preview)).toHaveLength(2);
});

const capabilities = {
  postBatch: true, images: true, videos: true, compatibility: true,
  clip: false, manualQuality: false, audioOnly: false, videoOnly: false,
} as const;
const analysis: TikTokAnalysisPreview = {
  analysisId: "1", postId: "12345678901234567890", title: "Post",
  sourcePageUrl: "https://www.tiktok.com/@example/video/12345678901234567890", capabilities,
  items: [
    { itemId: "11", ordinal: 1, kind: "image", container: "jpg", previewAvailable: true, availability: "ready" },
    { itemId: "12", ordinal: 2, kind: "video", container: "mp4", previewAvailable: false, availability: "ready" },
    { itemId: "13", ordinal: 3, kind: "video", container: "mp4", previewAvailable: false, availability: "unavailable", unavailableReason: "content_changed" },
  ],
};

function analyzedState() {
  let state = createTikTokWorkspaceState();
  state = tiktokWorkspaceReducer(state, { type: "urlChanged", url: "https://www.tiktok.com/@example/video/12345678901234567890" });
  state = tiktokWorkspaceReducer(state, { type: "analysisStarted", generation: 2 });
  return tiktokWorkspaceReducer(state, { type: "analysisSucceeded", generation: 2, analysis });
}

function progress(state: TikTokDownloadProgress["state"], overrides: Partial<TikTokDownloadProgress> = {}): TikTokDownloadProgress {
  return {
    jobId: "8", state, itemIndex: 1, itemCount: 2,
    downloadedBytes: null, totalBytes: null, percent: null, ...overrides,
  };
}

describe("TikTok workspace reducer", () => {
  it.each(["previewRequested", "previewLoaded", "previewFailed"] as const)("preserves download failures when %s arrives", (type) => {
    let state = tiktokWorkspaceReducer(analyzedState(), { type: "previewRequested", analysisId: "1", itemId: "11", requestId: 1 });
    state = tiktokWorkspaceReducer(state, { type: "downloadStartRequested", generation: 3 });
    state = tiktokWorkspaceReducer(state, { type: "downloadStartFailed", generation: 3, errorCode: "destination_unavailable" });
    const next = tiktokWorkspaceReducer(state, type === "previewRequested"
      ? { type, analysisId: "1", itemId: "11", requestId: 2 }
      : type === "previewLoaded"
        ? { type, analysisId: "1", itemId: "11", requestId: 1, preview: { mimeType: "image/jpeg", dataBase64: "AAAA" } }
        : { type, analysisId: "1", itemId: "11", requestId: 1, errorCode: "network_failed" });
    expect(next.error).toEqual({ scope: "download", code: "destination_unavailable" });
    if (type === "previewFailed") expect(next.previews["11"]).toMatchObject({ status: "error", errorCode: "network_failed" });
  });

  it("submits precisely one, selected, or all ready attachments in original order", () => {
    let state=readyTikTokState();
    expect(buildTikTokDownloadIntent(state)?.itemIds).toEqual(["11","12","13"]);
    state=tiktokWorkspaceReducer(state,{type:"selectionCleared"});
    state=tiktokWorkspaceReducer(state,{type:"selectionToggled",itemId:"13"});
    expect(buildTikTokDownloadIntent(state)).toEqual({itemIds:["13"],compatibilityMode:false});
    state=tiktokWorkspaceReducer(state,{type:"selectionToggled",itemId:"11"});
    expect(buildTikTokDownloadIntent(state)?.itemIds).toEqual(["11","13"]);
    state=tiktokWorkspaceReducer(state,{type:"selectionToggled",itemId:"14"});
    expect(buildTikTokDownloadIntent(state)?.itemIds).toEqual(["11","13"]);
  });
  it("treats silent animated MP4 as video for explicit compatibility only", () => {
    let state = analyzedState();
    const animated = { ...analysis, items: [{ ...analysis.items[1]!, kind: "video" as const }] };
    state = { ...state, analysis: animated, selectedItemIds: ["12"] };
    expect(buildTikTokDownloadIntent(state)).toEqual({ itemIds: ["12"], compatibilityMode: false });
    state = tiktokWorkspaceReducer(state, { type: "compatibilityChanged", value: true });
    expect(tiktokCompatibilityApplies(state)).toBe(true);
    expect(buildTikTokDownloadIntent(state)).toEqual({ itemIds: ["12"], compatibilityMode: true });
  });
  it("starts empty and selects all ready items after analysis", () => {
    expect(createTikTokWorkspaceState()).toMatchObject({
      url: "", analysis: null, selectedItemIds: [], compatibilityMode: false,
      previews: {}, requestGeneration: 0, job: null, result: null, error: null,
    });
    expect(analyzedState().selectedItemIds).toEqual(["11", "12"]);
  });

  it("toggles, selects all, and clears only ready items in platform order", () => {
    const state = analyzedState();
    const one = tiktokWorkspaceReducer(state, { type: "selectionToggled", itemId: "12" });
    expect(one.selectedItemIds).toEqual(["11"]);
    expect(tiktokWorkspaceReducer(one, { type: "selectionToggled", itemId: "13" })).toBe(one);
    expect(tiktokWorkspaceReducer(one, { type: "selectionAll" }).selectedItemIds).toEqual(["11", "12"]);
    expect(tiktokWorkspaceReducer(state, { type: "selectionCleared" }).selectedItemIds).toEqual([]);
  });

  it("retains the opt-in while image-only selection disables its effect", () => {
    let state = tiktokWorkspaceReducer(analyzedState(), { type: "compatibilityChanged", value: true });
    state = tiktokWorkspaceReducer(state, { type: "selectionToggled", itemId: "12" });
    expect(state.compatibilityMode).toBe(true);
    expect(tiktokCompatibilityApplies(state)).toBe(false);
    expect(buildTikTokDownloadIntent(state)).toEqual({ itemIds: ["11"], compatibilityMode: false });
    state = tiktokWorkspaceReducer(state, { type: "selectionToggled", itemId: "12" });
    expect(buildTikTokDownloadIntent(state)).toEqual({ itemIds: ["11", "12"], compatibilityMode: true });
  });

  it("correlates preview requests and ignores late data or errors", () => {
    let state = analyzedState();
    state = tiktokWorkspaceReducer(state, { type: "previewRequested", analysisId: "1", itemId: "11", requestId: 1 });
    state = tiktokWorkspaceReducer(state, { type: "previewRequested", analysisId: "1", itemId: "11", requestId: 2 });
    const late = tiktokWorkspaceReducer(state, {
      type: "previewLoaded", analysisId: "1", itemId: "11", requestId: 1,
      preview: { mimeType: "image/jpeg", dataBase64: "AAAA" },
    });
    expect(late).toBe(state);
    const failed = tiktokWorkspaceReducer(state, {
      type: "previewFailed", analysisId: "1", itemId: "11", requestId: 2,
      errorCode: "network_failed",
    });
    expect(failed.previews["11"]).toMatchObject({ status: "error", errorCode: "network_failed" });
  });

  it("clears prior selection, previews, results, and compatibility for every new analysis", () => {
    let state = tiktokWorkspaceReducer(analyzedState(), {
      type: "compatibilityChanged",
      value: true,
    });
    state = tiktokWorkspaceReducer(state, {
      type: "previewRequested",
      analysisId: "1",
      itemId: "11",
      requestId: 1,
    });
    state = tiktokWorkspaceReducer(state, {
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
    state = tiktokWorkspaceReducer(state, { type: "downloadStartRequested", generation: 3 });
    state = tiktokWorkspaceReducer(state, { type: "downloadStarted", generation: 3, jobId: "8", itemCount: 2 });
    state = tiktokWorkspaceReducer(state, { type: "downloadProgress", progress: progress("downloading") });
    expect(state.job).toMatchObject({ state: "downloading", downloadedBytes: null, totalBytes: null, percent: null });
    state = tiktokWorkspaceReducer(state, { type: "downloadProgress", progress: progress("publishing", { itemIndex: 2 }) });
    state = tiktokWorkspaceReducer(state, { type: "downloadProgress", progress: progress("partial", { itemIndex: 2 }) });
    const result: TikTokDownloadResult = {
      jobId: "8", state: "partial", postItemCount: 3, unavailableCount: 1, contentChanged: false, completedCount: 1, failedCount: 1,
      folderDisplayName: "creator [Abc_123-]",
      items: [
        { itemId: "11", ordinal: 1, kind: "image", state: "completed", displayName: "01.jpg" },
        { itemId: "12", ordinal: 2, kind: "video", state: "failed", errorCode: "content_changed" },
      ],
    };
    state = tiktokWorkspaceReducer(state, { type: "downloadFinished", result });
    expect(state.result).toEqual(result);
    expect(state.job?.state).toBe("partial");
    expect(isTikTokWorkspaceLocked(state)).toBe(false);
  });

  it("locks source and selection while active, then clears all stale state for a new URL", () => {
    let state = analyzedState();
    state = tiktokWorkspaceReducer(state, { type: "downloadStartRequested", generation: 3 });
    expect(isTikTokWorkspaceLocked(state)).toBe(true);
    expect(tiktokWorkspaceReducer(state, { type: "selectionCleared" })).toBe(state);
    expect(tiktokWorkspaceReducer(state, { type: "urlChanged", url: "new" })).toBe(state);
    state = tiktokWorkspaceReducer(state, { type: "downloadStartFailed", generation: 3, errorCode: "network_failed" });
    const replacement = tiktokWorkspaceReducer(state, { type: "urlChanged", url: "new" });
    expect(replacement).toMatchObject({
      url: "new", analysis: null, selectedItemIds: [], compatibilityMode: false,
      previews: {}, job: null, result: null, error: null,
    });
    expect(replacement.requestGeneration).toBeGreaterThan(state.requestGeneration);
  });

  it("keeps canceled jobs terminal when late progress arrives", () => {
    let state = analyzedState();
    state = tiktokWorkspaceReducer(state, { type: "downloadStartRequested", generation: 3 });
    state = tiktokWorkspaceReducer(state, {
      type: "downloadStarted",
      generation: 3,
      jobId: "8",
      itemCount: 2,
    });
    state = tiktokWorkspaceReducer(state, {
      type: "downloadProgress",
      progress: progress("canceled"),
    });
    const canceled = state;
    state = tiktokWorkspaceReducer(state, {
      type: "downloadProgress",
      progress: progress("downloading", {
        downloadedBytes: 10,
        totalBytes: 20,
        percent: 50,
      }),
    });
    expect(state).toBe(canceled);
    expect(state.job?.state).toBe("canceled");
    expect(isTikTokWorkspaceLocked(state)).toBe(false);
  });
});
