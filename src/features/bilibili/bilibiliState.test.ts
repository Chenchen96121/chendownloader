import { expect, it } from "vitest";
import type { BilibiliAnalysisPreview, BilibiliCoverPayload } from "../../shared/contracts/bilibili";
import { BilibiliAnalysisIpcError } from "../../shared/ipc/bilibili";
import { biliSample, deferred } from "./bilibiliTestFixtures";
import {
  createBilibiliState, changeBilibiliState, buildBilibiliSelectionIntent,
  requestBilibiliAnalysis, requestBilibiliSelection, requestBilibiliResolutions, requestBilibiliCover,
  type BilibiliState, type BilibiliCoverState,
} from "./bilibiliState";
import { beginDownloadStart, createDownloadUiState, detachSubmittedDownload, finishDownloadStart, synchronizeDownloadAnalysis } from "../downloads/downloadState";

function holder(initial: BilibiliState) {
  let value = initial;
  return { get value() { return value; }, publish(update: (old: BilibiliState) => BilibiliState) { value = update(value); } };
}
it("bilibili_workspace_keeps_independent_state and resets only its own edited source", () => {
  const first = { ...createBilibiliState(), data: biliSample, outcome: biliSample.outcome };
  const other = createBilibiliState();
  const changed = changeBilibiliState(first, { type: "mode", value: "manual" });
  expect(other.mode).toBe("best_available");
  expect(first.mode).toBe("best_available");
  expect(changed.data?.part).toBe(2);
  expect(changed.outcome).toBeNull();
  expect(changeBilibiliState(changed, { type: "url", value: "new" })).toMatchObject({ url: "new", data: null, mode: "best_available", compatibilityMode: false });
});
it("builds only supported manual video/container choices and never selects audio formats", () => {
  let state: BilibiliState = { ...createBilibiliState(), data: biliSample };
  state = changeBilibiliState(state, { type: "mode", value: "manual" });
  expect(buildBilibiliSelectionIntent(state)).toBeUndefined();
  state = changeBilibiliState(state, { type: "videoChoice", value: { videoFormatId: "hidden-v", outputContainer: "mp4" } });
  expect(buildBilibiliSelectionIntent(state)).toEqual({ mode: "manual", videoFormatId: "hidden-v", outputContainer: "mp4" });
  expect(buildBilibiliSelectionIntent({ ...state, outputContainer: "webm" })).toBeUndefined();
  expect(buildBilibiliSelectionIntent({ ...state, videoFormatId: "hidden-a" })).toBeUndefined();
  expect(buildBilibiliSelectionIntent(changeBilibiliState(state, { type: "compatibilityMode", value: true }))).toEqual({ mode: "manual", videoFormatId: "hidden-v", outputContainer: "mp4", compatibilityMode: true });
});
it("synchronizes clip range and exact inputs, rejects out of bounds and disables audio-only clips", () => {
  let state = changeBilibiliState({ ...createBilibiliState(), data: biliSample }, { type: "clipEnabled", value: true });
  expect(buildBilibiliSelectionIntent(state)?.clip).toEqual({ startMs: 0, endMs: 120000 });
  state = changeBilibiliState(state, { type: "clipRange", value: { startMs: 10100, endMs: 30500 } });
  expect(state.clipStart).toBe("0:00:10.100");
  expect(buildBilibiliSelectionIntent(state)?.clip).toEqual({ startMs: 10100, endMs: 30500 });
  expect(buildBilibiliSelectionIntent({ ...state, clipEnd: "0:02:01" })).toBeUndefined();
  state = changeBilibiliState(state, { type: "mode", value: "audio_only" });
  expect(state.clipEnabled).toBe(false);
  expect(buildBilibiliSelectionIntent(state)).toEqual({ mode: "audio_only" });
});
it("new analysis wins over a late prior success and preserves the typed new URL", async () => {
  const old = deferred<BilibiliAnalysisPreview>();
  const next = deferred<BilibiliAnalysisPreview>();
  const store = holder({ ...createBilibiliState(), url: "https://www.bilibili.com/video/BV1xx411c7mD?p=2" });
  const generation = { current: 0 };
  const first = requestBilibiliAnalysis(store.value, generation, store.publish, { analyze: () => old.promise });
  store.publish(state => changeBilibiliState(state, { type: "url", value: "https://www.bilibili.com/video/av170001" }));
  const second = requestBilibiliAnalysis(store.value, generation, store.publish, { analyze: () => next.promise });
  next.resolve({ ...biliSample, analysisId: "18" }); await second;
  old.resolve(biliSample); await first;
  expect(store.value.data?.analysisId).toBe("18");
  expect(store.value.url).toContain("av170001");
});
it("extracts the single ordinary Bili URL from share text while retaining the original input", async () => {
  const source = "【QWER '少年漫画' Official Special Clip】 https://www.bilibili.com/video/BV13Bab6QEir/?share_source=copy_web";
  const store = holder({ ...createBilibiliState(), url: source });
  const requests: unknown[] = [];
  const result = await requestBilibiliAnalysis(store.value, { current: 0 }, store.publish, {
    analyze: async (url, intent) => { requests.push({ url, intent }); return biliSample; },
  });
  expect(requests).toEqual([{ url: "https://www.bilibili.com/video/BV13Bab6QEir/?share_source=copy_web", intent: { mode: "best_available" } }]);
  expect(result).toBe(biliSample);
  expect(store.value).toMatchObject({ url: source, status: "analyzed", data: biliSample });
});
it.each([
  ["  https://www.bilibili.com/video/BV13Bab6QEir/?p=2&share_source=copy_web  ", "https://www.bilibili.com/video/BV13Bab6QEir/?p=2&share_source=copy_web"],
  ["【Part two】 https://bilibili.com/video/av170001?p=2#share", "https://bilibili.com/video/av170001?p=2#share"],
  ["【Native selector validation】 https://www.bilibili.com/video/BV13Bab6QEir?p=02&p=3", "https://www.bilibili.com/video/BV13Bab6QEir?p=02&p=3"],
])("forwards the original URL and selectors to native validation: %s", async (source, expected) => {
  const store = holder({ ...createBilibiliState(), url: source });
  const requests: string[] = [];
  await requestBilibiliAnalysis(store.value, { current: 0 }, store.publish, {
    analyze: async url => { requests.push(url); throw new BilibiliAnalysisIpcError("invalid_url"); },
  });
  expect(requests).toEqual([expected]);
  expect(store.value.error).toBe("invalid_url");
});
it.each([
  "【Two videos】 https://www.bilibili.com/video/BV13Bab6QEir https://www.bilibili.com/video/av170001",
  "【Other site】 http://evil.test https://www.bilibili.com/video/BV13Bab6QEir",
  "【Other scheme】 ftp://evil.test https://www.bilibili.com/video/BV13Bab6QEir",
  "https://www.bilibili.com/video/BV13Bab6QEir?next=https://evil.test",
  "【Embedded URL】 https://evil.test/?next=https://www.bilibili.com/video/BV13Bab6QEir",
  "【Host suffix】 https://www.bilibili.com.evil.test/video/BV13Bab6QEir",
  "【Credential】 https://evil.test@www.bilibili.com/video/BV13Bab6QEir",
  "【Empty credential】 https://@www.bilibili.com/video/BV13Bab6QEir",
  "【Port】 https://www.bilibili.com:443/video/BV13Bab6QEir",
  "【Path normalization】 https://www.bilibili.com/foo/../video/BV13Bab6QEir",
  "【Encoded path】 https://www.bilibili.com/%76ideo/BV13Bab6QEir",
  "【Malformed key】 https://www.bilibili.com/video/BV13Bab6QEir/extra",
  "【Whitespace in link】 https://www.bilibili.com/video/BV13Bab6QEir ?p=2",
  "【Missing scheme】 www.bilibili.com/video/BV13Bab6QEir",
  "【Insecure scheme】 http://www.bilibili.com/video/BV13Bab6QEir",
  "【Short link】 https://b23.tv/example",
  "【Backslash】 https://www.bilibili.com\\video\\BV13Bab6QEir",
  "【Control\tcharacter】 https://www.bilibili.com/video/BV13Bab6QEir",
  "【Newline】\nhttps://www.bilibili.com/video/BV13Bab6QEir",
  "【Separator】\u2028https://www.bilibili.com/video/BV13Bab6QEir",
  `【${"中".repeat(683)}】 https://www.bilibili.com/video/BV13Bab6QEir`,
])("rejects unsafe or ambiguous share text before analysis: %s", async source => {
  const store = holder({ ...createBilibiliState(), url: source });
  let requested = false;
  const result = await requestBilibiliAnalysis(store.value, { current: 0 }, store.publish, {
    analyze: async () => { requested = true; return biliSample; },
  });
  expect(result).toBeNull();
  expect(requested).toBe(false);
  expect(store.value).toMatchObject({ url: source, status: "error", error: "invalid_url", data: null });
});
it("does not apply a late share-text result after its source is edited", async () => {
  const response = deferred<BilibiliAnalysisPreview>();
  const store = holder({ ...createBilibiliState(), url: "【First】 https://www.bilibili.com/video/BV13Bab6QEir" });
  const pending = requestBilibiliAnalysis(store.value, { current: 0 }, store.publish, { analyze: () => response.promise });
  store.publish(state => changeBilibiliState(state, { type: "url", value: "【Second】 https://www.bilibili.com/video/av170001" }));
  response.resolve(biliSample);
  expect(await pending).toBeNull();
  expect(store.value).toMatchObject({ url: "【Second】 https://www.bilibili.com/video/av170001", data: null, status: "idle" });
});
it("a stale selection cannot overwrite a new intent even before its next request begins", async () => {
  const response = deferred<BilibiliAnalysisPreview>();
  const store = holder({ ...createBilibiliState(), data: biliSample });
  const request = requestBilibiliSelection(store.value, { current: 0 }, store.publish, { preview: () => response.promise });
  store.publish(state => changeBilibiliState(state, { type: "mode", value: "audio_only" }));
  response.resolve(biliSample); await request;
  expect(store.value.mode).toBe("audio_only"); expect(store.value.outcome).toBeNull();
});
it("new selection wins and maps only safe typed errors", async () => {
  const old = deferred<BilibiliAnalysisPreview>();
  const store = holder({ ...createBilibiliState(), data: biliSample });
  const generation = { current: 0 };
  const first = requestBilibiliSelection(store.value, generation, store.publish, { preview: () => old.promise });
  store.publish(state => changeBilibiliState(state, { type: "mode", value: "audio_only" }));
  await requestBilibiliSelection(store.value, generation, store.publish, { preview: async () => { throw new BilibiliAnalysisIpcError("analysis_expired"); } });
  old.reject(new Error("private URL")); await first;
  expect(store.value.error).toBe("analysis_expired");
});
it("stale resolution and cover responses cannot replace a newer snapshot", async () => {
  const a = deferred<[]>(); const b = deferred<BilibiliCoverPayload>();
  const generation = { current: 0 }; const covers = { current: 0 };
  const received: string[] = []; const images: BilibiliCoverState[] = [];
  const first = requestBilibiliResolutions(biliSample, "manual", false, generation, result => received.push(result.analysisId), { resolutionOptions: () => a.promise });
  const cover = requestBilibiliCover("17", covers, result => images.push(result), { cover: () => b.promise });
  await requestBilibiliResolutions({ ...biliSample, analysisId: "18" }, "video_only", true, generation, result => received.push(result.analysisId), { resolutionOptions: async () => [] });
  await requestBilibiliCover("18", covers, result => images.push(result), { cover: async () => { throw new Error("private"); } });
  a.resolve([]); b.resolve({ mimeType: "image/png", dataBase64: "iVBORw0KGgo=" }); await Promise.all([first, cover]);
  expect(received).toEqual(["18"]);
  expect(images.at(-1)).toEqual({ analysisId: "18", status: "error", payload: null });
});
it("submitted Bili jobs unlock a new source without canceling or retaining the old job", () => {
  const begun = beginDownloadStart({ ...createDownloadUiState("17"), destination: { destinationId: "4", displayName: "Downloads" } });
  const submitted = finishDownloadStart(begun.state, begun.generation, "80");
  const detached = detachSubmittedDownload(submitted, begun.generation);
  expect(detached).toMatchObject({ phase: "idle", jobId: null, analysisId: "17" });
  expect(synchronizeDownloadAnalysis(detached, "18")).toMatchObject({ analysisId: "18", phase: "idle", jobId: null });
  expect(submitted.jobId).toBe("80");
});
