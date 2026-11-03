import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SelectionPreview } from "../../shared/contracts/selection";
import { createSelectionState, changeSelection, buildSelectionIntent, manualOptions, requestSelection, type SelectionState, type SelectionChange } from "./selectionState";
import { sample } from "./selectionTestFixtures";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const nativeInvoke = vi.mocked(invoke);
beforeEach(() => { nativeInvoke.mockReset(); });

function session(initial?: Partial<SelectionState>) {
  let state = { ...createSelectionState(), ...initial };
  const generation = { current: 0 };
  return {
    get state() { return state; },
    generation,
    edit(change: SelectionChange) { generation.current += 1; state = changeSelection(state, change); },
    request() { return requestSelection(state, generation, (update) => { state = update(state); }); },
  };
}
function deferred() {
  let resolve!: (value: SelectionPreview) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<SelectionPreview>((yes, no) => { resolve = yes; reject = no; });
  void promise.catch(() => {});
  return { promise, resolve, reject };
}
const loaded = { data: sample, outcome: sample.outcome, status: "ready" as const };

describe("selection events and request freshness", () => {
  it("keeps fixture compatibility explicit and resets it on platform replacement", () => {
    const initial={...createSelectionState(),...loaded,mode:"video_only" as const,videoFormatId:"hidden-v",outputContainer:"webm" as const};
    expect(initial.compatibilityMode).toBe(false);
    const enabled=changeSelection(initial,{type:"compatibilityMode",value:true});
    expect(buildSelectionIntent(enabled)).toEqual({intent:{mode:"video_only",compatibilityMode:true,videoFormatId:"hidden-v",outputContainer:"mp4"}});
    const disabled=changeSelection(enabled,{type:"compatibilityMode",value:false});expect(disabled.videoFormatId).toBe("hidden-v");expect(buildSelectionIntent(disabled)).toEqual({intent:{mode:"video_only",videoFormatId:"hidden-v"}});
    expect(changeSelection(enabled,{type:"platform",value:"bilibili"}).compatibilityMode).toBe(false);
  });
  it("initializes and synchronizes the same full timeline interval in fixtures", () => {
    const s = session(loaded);
    s.edit({ type: "clipEnabled", value: true });
    expect(buildSelectionIntent(s.state)).toEqual({ intent: { mode: "best_available", clip: {startMs: 0, endMs: 120000} } });
    s.edit({type: "clipRange", value: {startMs: 1000, endMs: 21000}});
    expect(s.state).toMatchObject({clipStart: "0:00:01", clipEnd: "0:00:21"});
  });
  it.each(["manual", "video_only"] as const)("initializes a selectable %s mode from the native recommendation", (mode) => {
    const s = session(loaded);
    s.edit({ type: "mode", value: mode });
    expect(s.state.videoFormatId).toBe("");
    s.edit({type: "videoChoice", value: {videoFormatId: "hidden-v", outputContainer: "mp4"}});
    expect(buildSelectionIntent(s.state)).toEqual({ intent: { mode, videoFormatId: "hidden-v", outputContainer: "mp4" } });
    s.edit({ type: "video", value: "hidden-c" });
    s.edit({ type: "mode", value: "best_available" });
    s.edit({ type: "mode", value: mode });
    expect(s.state.videoFormatId).toBe("hidden-c");
  });
  it("builds a selected video-only resolution with only the container choice", () => {
    const s = session({ ...loaded, mode: "video_only" as SelectionState["mode"], videoFormatId: "hidden-v", outputContainer: "mp4" });
    expect(buildSelectionIntent(s.state)).toEqual({ intent: { mode: "video_only", videoFormatId: "hidden-v", outputContainer: "mp4" } });
  });
  it("automatically pairs native-ranked audio for a manual resolution when none was explicitly chosen", () => {
    const s = session({ ...loaded, mode: "manual", videoFormatId: "hidden-v", outputContainer: "" });
    expect(buildSelectionIntent(s.state)).toEqual({ intent: { mode: "manual", videoFormatId: "hidden-v" } });
  });
  it("starts without a request and requests the default intent only on an explicit action", async () => {
    const s = session();
    expect(s.state).toMatchObject({ platform: "youtube", mode: "best_available", clipEnabled: false, data: null });
    expect(nativeInvoke).not.toHaveBeenCalled();
    nativeInvoke.mockResolvedValue(sample);
    await s.request();
    expect(nativeInvoke).toHaveBeenCalledExactlyOnceWith("preview_media_selection_fixture", { platform: "youtube", intent: { mode: "best_available" } });
    expect(s.state.outcome).toEqual(sample.outcome);
  });
  it.each(["best_available", "video_only", "audio_only", "manual"] as const)("sends mode %s without a fallback", async (mode) => {
    const s = session();
    s.edit({ type: "mode", value: mode });
    const code = mode === "manual" ? "invalid_selection" : "format_unavailable";
    nativeInvoke.mockResolvedValue({ ...sample, outcome: { status: "unavailable", code } });
    await s.request();
    expect(nativeInvoke).toHaveBeenCalledExactlyOnceWith("preview_media_selection_fixture", { platform: "youtube", intent: { mode } });
    expect(s.state.data?.media.title).toBe("<img src=x onerror=alert(1)>");
    expect(s.state.outcome).toEqual({ status: "unavailable", code });
  });
  it("filters ranked manual roles and resets target when source changes", () => {
    const s = session({ ...loaded, mode: "manual", videoFormatId: "hidden-v", outputContainer: "mp4" });
    expect(manualOptions(s.state).video.map((f) => f.id)).toEqual(["hidden-v", "hidden-c"]);
    expect(manualOptions(s.state).audio.map((f) => f.id)).toEqual(["hidden-a"]);
    expect(buildSelectionIntent(s.state)).toEqual({ intent: { mode: "manual", videoFormatId: "hidden-v", outputContainer: "mp4" } });
    s.edit({ type: "video", value: "hidden-c" });
    expect(s.state.outputContainer).toBe("");
    expect(buildSelectionIntent(s.state)).toEqual({ intent: { mode: "manual", videoFormatId: "hidden-c" } });
  });
  it.each([["", ""], ["hidden-v", "webm"], ["foreign", "mp4"]])("blocks invalid manual pair %s / %s before IPC", async (videoFormatId, outputContainer) => {
    const s = session({ ...loaded, mode: "manual", videoFormatId, outputContainer: outputContainer as SelectionState["outputContainer"] });
    await s.request();
    expect(nativeInvoke).not.toHaveBeenCalled();
    expect(s.state.error).toBe("manual");
    expect(s.state.outcome).toBeNull();
  });
  it("clears and disables clip intent on entering audio-only", () => {
    const s = session({ ...loaded, clipEnabled: true, clipStart: "0:00:10", clipEnd: "0:00:30" });
    s.edit({ type: "mode", value: "audio_only" });
    expect(s.state).toMatchObject({ clipEnabled: false, clipStart: "", clipEnd: "", outcome: null });
    expect(buildSelectionIntent(s.state)).toEqual({ intent: { mode: "audio_only" } });
  });
  it("validates literal clip endpoints against duration before IPC", async () => {
    const s = session({ ...loaded, clipEnabled: true, clipStart: "0:00:10.025", clipEnd: "0:00:30.075" });
    if (sample.outcome.status !== "selected") throw new Error("Expected selected fixture");
    const clip = { startMs: 10025, endMs: 30075 };
    nativeInvoke.mockResolvedValue({ ...sample, outcome: { status: "selected", plan: {
      ...sample.outcome.plan, clip,
      warnings: [...sample.outcome.plan.warnings, "clip_boundary_may_vary", "clip_transfer_may_exceed_range"],
    } } });
    await s.request();
    expect(nativeInvoke).toHaveBeenCalledExactlyOnceWith("preview_media_selection_fixture", { platform: "youtube", intent: { mode: "best_available", clip } });
    expect(s.state.outcome).toMatchObject({ status: "selected", plan: { clip } });
  });
  it.each([["", "0:00:30"], ["1e2", "0:00:30"], ["0:00:30", "0:00:10"], ["0:00:10", "0:00:10"], ["0:00:00", "0:02:00.001"], ["8760:00:00", "8760:00:01"]])("blocks invalid clip %s to %s without a full-media retry", async (clipStart, clipEnd) => {
    const s = session({ ...loaded, clipEnabled: true, clipStart, clipEnd });
    await s.request();
    expect(nativeInvoke).not.toHaveBeenCalled();
    expect(s.state.error).toBe("clip");
  });
  it.each([undefined, 0])("blocks clips when duration is %s", async (duration) => {
    const media = { ...sample.media };
    if (duration === undefined) delete media.durationSeconds;
    else media.durationSeconds = duration;
    const s = session({ ...loaded, data: { ...sample, media }, clipEnabled: true, clipStart: "0:00:00", clipEnd: "0:00:01" });
    await s.request();
    expect(nativeInvoke).not.toHaveBeenCalled();
    expect(s.state.error).toBe("clip");
  });
  it("accepts the floored duration boundary", () => {
    const s = session({ ...loaded, data: { ...sample, media: { ...sample.media, durationSeconds: 1.2349 } }, clipEnabled: true, clipStart: "0:00:00", clipEnd: "0:00:01.234" });
    expect(buildSelectionIntent(s.state)).toEqual({ intent: { mode: "best_available", clip: { startMs: 0, endMs: 1234 } } });
  });
  it.each([
    { type: "platform", value: "x" }, { type: "mode", value: "audio_only" },
    { type: "video", value: "hidden-c" }, { type: "videoChoice", value: { videoFormatId: "hidden-v", outputContainer: "mp4" } },
    { type: "clipEnabled", value: true }, { type: "clipStart", value: "0:00:01" }, { type: "clipEnd", value: "0:00:02" },
  ] as SelectionChange[])("invalidates pending success on $type edits", async (edit) => {
    const wait = deferred();
    nativeInvoke.mockReturnValue(wait.promise);
    const s = session(loaded);
    const pending = s.request();
    expect(s.state.status).toBe("loading");
    s.edit(edit);
    expect(s.state.outcome).toBeNull();
    expect(s.state.status).toBe("idle");
    wait.resolve(sample);
    await pending;
    expect(s.state.outcome).toBeNull();
    expect(s.state.error).toBeNull();
  });
  it("ignores an obsolete error after an input edit", async () => {
    const wait = deferred();
    nativeInvoke.mockReturnValue(wait.promise);
    const s = session(loaded);
    const pending = s.request();
    s.edit({ type: "mode", value: "manual" });
    wait.reject(new Error("PRIVATE native error"));
    await pending;
    expect(s.state.status).toBe("idle");
    expect(s.state.error).toBeNull();
  });
  it("newer success wins over older success and error", async () => {
    const old = deferred();
    const newer = deferred();
    nativeInvoke.mockReturnValueOnce(old.promise).mockReturnValueOnce(newer.promise);
    const s = session();
    const first = s.request();
    const second = s.request();
    newer.resolve(sample);
    await second;
    old.reject(new Error("PRIVATE"));
    await first;
    expect(s.state.outcome).toEqual(sample.outcome);
    expect(s.state.status).toBe("ready");
  });
  it("ignores success after unmount invalidation", async () => {
    const wait = deferred();
    nativeInvoke.mockReturnValue(wait.promise);
    const s = session();
    const pending = s.request();
    s.generation.current += 1;
    wait.resolve(sample);
    await pending;
    expect(s.state.data).toBeNull();
  });
  it("platform change clears every source-bound input and result", () => {
    const s = session({ ...loaded, mode: "manual", videoFormatId: "hidden-v", outputContainer: "mp4", clipEnabled: true, clipStart: "0:00:10", clipEnd: "0:00:30", error: "clip" });
    s.edit({ type: "platform", value: "bilibili" });
    expect(s.state).toMatchObject({ platform: "bilibili", data: null, outcome: null, videoFormatId: "", outputContainer: "", clipEnabled: false, clipStart: "", clipEnd: "", error: null });
  });
  it("a changed media response clears inputs bound to the former source", async () => {
    const s = session({ ...loaded, videoFormatId: "hidden-v", outputContainer: "mp4", clipEnabled: true, clipStart: "0:00:10", clipEnd: "0:00:30" });
    if (sample.outcome.status !== "selected") throw new Error("Expected selected fixture");
    nativeInvoke.mockResolvedValue({ ...sample, media: { ...sample.media, mediaId: "new-media" }, outcome: { status: "selected", plan: {
      ...sample.outcome.plan, clip: { startMs: 10000, endMs: 30000 },
      warnings: [...sample.outcome.plan.warnings, "clip_boundary_may_vary", "clip_transfer_may_exceed_range"],
    } } });
    await s.request();
    expect(s.state).toMatchObject({ videoFormatId: "", outputContainer: "", clipEnabled: false, clipStart: "", clipEnd: "", outcome: null, error: null });
    expect(s.state.data?.media.mediaId).toBe("new-media");
  });
  it("same media retains all inputs after a valid unavailable response", async () => {
    const s = session({ ...loaded, clipEnabled: true, clipStart: "0:00:10", clipEnd: "0:00:30" });
    nativeInvoke.mockResolvedValue({ ...sample, outcome: { status: "unavailable", code: "format_unavailable" } });
    await s.request();
    expect(s.state).toMatchObject({ clipEnabled: true, clipStart: "0:00:10", clipEnd: "0:00:30", outcome: { status: "unavailable", code: "format_unavailable" } });
  });
  it.each([null, { private: "PRIVATE" }])("sanitizes malformed native response %j", async (response) => {
    nativeInvoke.mockResolvedValue(response);
    const s = session(loaded);
    await s.request();
    expect(s.state).toMatchObject({ status: "error", error: "preview", outcome: null });
    expect(s.state.data?.media.mediaId).toBe("hidden-media");
    expect(JSON.stringify(s.state)).not.toContain("PRIVATE native");
  });
});
