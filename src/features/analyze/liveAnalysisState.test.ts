import { beforeEach, describe, expect, it, vi } from "vitest";
import type { LiveAnalysisPreview } from "../../shared/contracts/liveAnalysis";
import { LiveAnalysisError, analyzeMedia, previewMediaSelection } from "../../shared/ipc/liveAnalysis";
import {
  buildLiveSelectionIntent,
  changeLiveAnalysis,
  createLiveAnalysisState,
  manualLiveOptions,
  requestLiveAnalysis,
  requestLiveSelection,
  validateLiveUrl,
  type LiveAnalysisChange,
  type LiveAnalysisState,
} from "./liveAnalysisState";
import { sample, samplePlan } from "./selectionTestFixtures";

vi.mock("../../shared/ipc/liveAnalysis", async (importOriginal) => {
  const original = await importOriginal<typeof import("../../shared/ipc/liveAnalysis")>();
  return { ...original, analyzeMedia: vi.fn(), previewMediaSelection: vi.fn() };
});

const analyze = vi.mocked(analyzeMedia);
const preview = vi.mocked(previewMediaSelection);
const liveSample: LiveAnalysisPreview = { analysisId: "1", ...sample };

beforeEach(() => {
  analyze.mockReset();
  preview.mockReset();
});

function session(initial?: Partial<LiveAnalysisState>) {
  let state = { ...createLiveAnalysisState(), ...initial };
  const generation = { current: 0 };
  return {
    get state() { return state; },
    generation,
    edit(change: LiveAnalysisChange) {
      generation.current += 1;
      state = changeLiveAnalysis(state, change);
    },
    analyze() {
      return requestLiveAnalysis(state, generation, (update) => { state = update(state); });
    },
    select() {
      return requestLiveSelection(state, generation, (update) => { state = update(state); });
    },
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  void promise.catch(() => {});
  return { promise, resolve, reject };
}

describe("live URL analysis state", () => {
  it("defaults compatibility off, retains source across toggles, resets for a new URL", () => {
    const initial={...createLiveAnalysisState(),data:liveSample,mode:"manual" as const,videoFormatId:"hidden-v",outputContainer:"webm" as const,outcome:sample.outcome};
    expect(initial.compatibilityMode).toBe(false);
    const enabled=changeLiveAnalysis(initial,{type:"compatibilityMode",value:true});
    expect(enabled).toMatchObject({compatibilityMode:true,videoFormatId:"hidden-v",outputContainer:"mp4",outcome:null});
    expect(buildLiveSelectionIntent(enabled)).toEqual({intent:{mode:"manual",compatibilityMode:true,videoFormatId:"hidden-v",outputContainer:"mp4"}});
    expect(changeLiveAnalysis(enabled,{type:"mode",value:"audio_only"}).compatibilityMode).toBe(true);
    const disabled=changeLiveAnalysis(enabled,{type:"compatibilityMode",value:false});
    expect(disabled).toMatchObject({compatibilityMode:false,videoFormatId:"hidden-v",outputContainer:"",outcome:null});
    expect(buildLiveSelectionIntent(disabled)).toEqual({intent:{mode:"manual",videoFormatId:"hidden-v"}});
    expect(changeLiveAnalysis(enabled,{type:"url",value:"https://youtu.be/91YrsgWQcEo"}).compatibilityMode).toBe(false);
  });
  it("late_compatible_preview_cannot_restore_transcode_after_toggle_off", async () => {
    const s=session({data:liveSample,mode:"manual",videoFormatId:"hidden-v",outputContainer:"webm"});
    const old=deferred<LiveAnalysisPreview>();const current=deferred<LiveAnalysisPreview>();
    preview.mockReturnValueOnce(old.promise).mockReturnValueOnce(current.promise);
    s.edit({type:"compatibilityMode",value:true});expect(s.state?.compatibilityMode).toBe(true);const before=s.select();
    s.edit({type:"compatibilityMode",value:false});expect(s.state.outcome).toBeNull();const after=s.select();
    current.resolve(liveSample);await after;
    const approved={...liveSample,outcome:{status:"selected" as const,plan:{...samplePlan,compatibilityMode:true,containerSelection:"compatibility" as const,videoProcessing:"transcode" as const}}};
    old.resolve(approved);await before;
    expect(s.state.compatibilityMode).toBe(false);expect(s.state.videoFormatId).toBe("hidden-v");expect(s.state.outcome).toBe(liveSample.outcome);
    expect(preview.mock.calls[1]?.[1]).toEqual({mode:"manual",videoFormatId:"hidden-v"});
  });
  it("does not clear a valid selection when the same value is selected again", () => {
    const state = {...createLiveAnalysisState(), data: liveSample, outcome: liveSample.outcome, status: "ready" as const, mode: "manual" as const, videoFormatId: "hidden-v"};
    expect(changeLiveAnalysis(state, {type: "video", value: "hidden-v"})).toBe(state);
    expect(changeLiveAnalysis(state, {type: "mode", value: "manual"})).toBe(state);
  });
  it("initializes a full clip and updates slider/text as one atomic valid range", () => {
    const s = session({ data: liveSample });
    s.edit({ type: "clipEnabled", value: true });
    expect(buildLiveSelectionIntent(s.state)).toEqual({ intent: { mode: "best_available", clip: { startMs: 0, endMs: 120000 } } });
    s.edit({ type: "clipRange", value: { startMs: 10234, endMs: 30000 } });
    expect(s.state).toMatchObject({ clipStart: "0:00:10.234", clipEnd: "0:00:30" });
    s.edit({ type: "clipStart", value: "bad" });
    expect(buildLiveSelectionIntent(s.state)).toEqual({ error: "clip" });
    s.edit({ type: "clipRange", value: { startMs: 30000, endMs: 30000 } });
    expect(buildLiveSelectionIntent(s.state)).toEqual({ error: "clip" });
    s.edit({ type: "mode", value: "audio_only" });
    expect(s.state).toMatchObject({ clipEnabled: false, clipStart: "", clipEnd: "" });
  });
  it.each(["manual", "video_only"] as const)("uses the selected resolution in live %s requests", (mode) => {
    const s = session({ data: liveSample });
    s.edit({ type: "mode", value: mode });
    expect(s.state.videoFormatId).toBe("");
    s.edit({type: "videoChoice", value: {videoFormatId: "hidden-v", outputContainer: "mp4"}});
    expect(buildLiveSelectionIntent(s.state)).toEqual({ intent: { mode, videoFormatId: "hidden-v", outputContainer: "mp4" } });
    s.edit({ type: "video", value: "hidden-c" });
    expect(buildLiveSelectionIntent(s.state)).toEqual({ intent: { mode, videoFormatId: "hidden-c" } });
    s.edit({ type: "video", value: "stale" });
    expect(buildLiveSelectionIntent(s.state)).toEqual({ error: "manual" });
  });
  it("starts URL-first without issuing a request", () => {
    expect(createLiveAnalysisState()).toMatchObject({
      url: "", mode: "best_available", data: null, outcome: null, status: "idle", error: null,
    });
    expect(analyze).not.toHaveBeenCalled();
  });

  it.each([
    ["", undefined],
    ["   ", undefined],
    ["https://youtube.com/watch?v=x\n", undefined],
    [`https://youtube.com/${"a".repeat(2040)}`, undefined],
    ["  https://www.youtube.com/watch?v=abc  ", "https://www.youtube.com/watch?v=abc"],
  ])("validates and trims bounded URL input %j", (input, expected) => {
    expect(validateLiveUrl(input)).toBe(expected);
  });

  it("invalidates all source-bound state when the URL changes", () => {
    const s = session({
      url: "https://youtube.com/old", data: liveSample, outcome: liveSample.outcome,
      mode: "manual", videoFormatId: "hidden-v", outputContainer: "mp4",
      clipEnabled: true, clipStart: "0:00:10", clipEnd: "0:00:30", status: "ready",
    });
    s.edit({ type: "url", value: "https://youtube.com/new" });
    expect(s.state).toMatchObject({
      url: "https://youtube.com/new", data: null, outcome: null, mode: "best_available",
      videoFormatId: "", outputContainer: "", clipEnabled: false, clipStart: "", clipEnd: "",
      status: "idle", error: null,
    });
  });

  it("ignores late analysis success and late analysis error after an edit", async () => {
    const lateSuccess = deferred<LiveAnalysisPreview>();
    analyze.mockReturnValueOnce(lateSuccess.promise);
    const first = session({ url: "https://youtube.com/one" });
    const pendingSuccess = first.analyze();
    first.edit({ type: "url", value: "https://youtube.com/two" });
    lateSuccess.resolve(liveSample);
    await pendingSuccess;
    expect(first.state.data).toBeNull();

    const lateError = deferred<LiveAnalysisPreview>();
    analyze.mockReturnValueOnce(lateError.promise);
    const second = session({ url: "https://youtube.com/one" });
    const pendingError = second.analyze();
    second.edit({ type: "url", value: "https://youtube.com/two" });
    lateError.reject(new LiveAnalysisError("network_failed"));
    await pendingError;
    expect(second.state).toMatchObject({ status: "idle", error: null, data: null });
  });

  it("lets a newer analysis replace an older one and resets source choices", async () => {
    const oldRequest = deferred<LiveAnalysisPreview>();
    const newRequest = deferred<LiveAnalysisPreview>();
    analyze.mockReturnValueOnce(oldRequest.promise).mockReturnValueOnce(newRequest.promise);
    const s = session({ url: "https://youtube.com/one" });
    const first = s.analyze();
    s.edit({ type: "url", value: "https://youtube.com/two" });
    const second = s.analyze();
    const replacement = { ...liveSample, analysisId: "2", media: { ...liveSample.media, mediaId: "replacement" } };
    newRequest.resolve(replacement);
    await second;
    oldRequest.resolve(liveSample);
    await first;
    expect(s.state.data?.analysisId).toBe("2");
    expect(s.state.data?.media.mediaId).toBe("replacement");
    expect(s.state.outcome).toEqual(replacement.outcome);
  });

  it("distinguishes an initial analysis result from an updated selection preview", async () => {
    analyze.mockResolvedValueOnce(liveSample);
    preview.mockResolvedValueOnce(liveSample);
    const s = session({ url: "https://youtube.com/watch?v=x" });
    await s.analyze();
    expect(s.state.status).toBe("analyzed");
    await s.select();
    expect(s.state.status).toBe("ready");
  });

  it("builds only valid manual format relationships", () => {
    const s = session({ url: "https://youtube.com/watch?v=x", data: liveSample, outcome: liveSample.outcome, mode: "manual" });
    expect(manualLiveOptions(s.state).video.map((item) => item.id)).toEqual(["hidden-v", "hidden-c"]);
    expect(manualLiveOptions(s.state).audio.map((item) => item.id)).toEqual(["hidden-a"]);
    expect(buildLiveSelectionIntent(s.state)).toEqual({ error: "manual" });
    s.edit({ type: "video", value: "hidden-v" });
    s.edit({ type: "videoChoice", value: {videoFormatId: "hidden-v", outputContainer: "mp4"} });
    expect(buildLiveSelectionIntent(s.state)).toEqual({ intent: { mode: "manual", videoFormatId: "hidden-v", outputContainer: "mp4" } });
    s.edit({ type: "video", value: "hidden-c" });
    expect(buildLiveSelectionIntent(s.state)).toEqual({ intent: { mode: "manual", videoFormatId: "hidden-c" } });
  });

  it("validates clip range before preview IPC", async () => {
    const s = session({
      url: "https://youtube.com/watch?v=x", data: liveSample, outcome: liveSample.outcome,
      clipEnabled: true, clipStart: "0:00:30", clipEnd: "0:00:10",
    });
    await s.select();
    expect(preview).not.toHaveBeenCalled();
    expect(s.state).toMatchObject({ status: "error", error: "clip", outcome: null });
  });

  it("previews against the opaque analysis session and ignores stale responses", async () => {
    const wait = deferred<LiveAnalysisPreview>();
    preview.mockReturnValue(wait.promise);
    const s = session({ url: "https://youtube.com/watch?v=x", data: liveSample, outcome: liveSample.outcome });
    const pending = s.select();
    expect(preview).toHaveBeenCalledExactlyOnceWith("1", { mode: "best_available" });
    s.edit({ type: "mode", value: "video_only" });
    wait.resolve(liveSample);
    await pending;
    expect(s.state).toMatchObject({ mode: "video_only", outcome: null, status: "idle" });
  });
});
it("changing container alone invalidates the old preview and sends the chosen pair", () => {
  const data = {...liveSample, media: {...liveSample.media, formats: liveSample.media.formats.map((row) => row.id === "hidden-v" ? {...row, videoCodec: "vp9"} : row)}};
  const state = {...createLiveAnalysisState(), data, outcome: liveSample.outcome, status: "ready" as const, mode: "manual" as const, videoFormatId: "hidden-v", outputContainer: "mp4" as const};
  const changed = changeLiveAnalysis(state, {type: "videoChoice", value: {videoFormatId: "hidden-v", outputContainer: "webm"}});
  expect(changed.outcome).toBeNull();
  expect(changed.status).toBe("idle");
  expect(buildLiveSelectionIntent(changed)).toEqual({intent: {mode: "manual", videoFormatId: "hidden-v", outputContainer: "webm"}});
  expect(changeLiveAnalysis(changed, {type: "videoChoice", value: {videoFormatId: "hidden-v", outputContainer: "webm"}})).toBe(changed);
});
