import type { BilibiliAnalysisPreview, BilibiliCoverPayload } from "../../shared/contracts/bilibili";
import { supportedVideoContainers, type SelectionIntent } from "../../shared/contracts/selection";
import { bilibiliIpc, BilibiliAnalysisIpcError, type BilibiliIpc } from "../../shared/ipc/bilibili";
import type { LiveAnalysisState, LiveAnalysisChange } from "../analyze/liveAnalysisState";
import type { ResolutionPlansResult } from "../analyze/resolutionOptions";
import { formatClipTime, fullClipRange, parseClipTime } from "../analyze/clipRange";

export type BilibiliState = Omit<LiveAnalysisState, "data"> & { data: BilibiliAnalysisPreview | null };
export type BilibiliUiChange = LiveAnalysisChange;
type Generation = { current: number };
type Publish = (update: (previous: BilibiliState) => BilibiliState) => void;
export interface BilibiliCoverState {
  analysisId: string;
  status: "loading" | "ready" | "error";
  payload: BilibiliCoverPayload | null;
}

export function createBilibiliState(): BilibiliState {
  return { url: "", mode: "best_available", compatibilityMode: false, videoFormatId: "", outputContainer: "",
    clipEnabled: false, clipStart: "", clipEnd: "", data: null, outcome: null, status: "idle", error: null };
}
export function bilibiliClipAvailable(state: BilibiliState): boolean {
  return state.mode !== "audio_only" && fullClipRange(state.data?.media.durationSeconds) !== undefined;
}
export function changeBilibiliState(state: BilibiliState, change: BilibiliUiChange): BilibiliState {
  if (change.type === "url") return { ...createBilibiliState(), url: change.value };
  if ((change.type === "videoChoice" && change.value.videoFormatId === state.videoFormatId && change.value.outputContainer === state.outputContainer)
    || (change.type === "video" && change.value === state.videoFormatId)
    || (change.type === "mode" && change.value === state.mode)
    || (change.type === "compatibilityMode" && change.value === state.compatibilityMode)
    || (change.type === "clipEnabled" && change.value === state.clipEnabled)
    || (change.type === "clipStart" && change.value === state.clipStart)
    || (change.type === "clipEnd" && change.value === state.clipEnd)) return state;
  const next: BilibiliState = { ...state, outcome: null, status: "idle", error: null };
  switch (change.type) {
    case "mode": return change.value === "audio_only"
      ? { ...next, mode: change.value, clipEnabled: false, clipStart: "", clipEnd: "" }
      : { ...next, mode: change.value };
    case "compatibilityMode": return { ...next, compatibilityMode: change.value, outputContainer: change.value && next.videoFormatId ? "mp4" : "" };
    case "videoChoice": return { ...next, ...change.value };
    case "video": return { ...next, videoFormatId: change.value, outputContainer: "" };
    case "clipEnabled": {
      if (change.value && !bilibiliClipAvailable(next)) return state;
      const full = fullClipRange(next.data?.media.durationSeconds);
      return change.value && full && next.clipStart === "" && next.clipEnd === ""
        ? { ...next, clipEnabled: true, clipStart: formatClipTime(full.startMs), clipEnd: formatClipTime(full.endMs) }
        : { ...next, clipEnabled: change.value };
    }
    case "clipRange": return { ...next, clipStart: formatClipTime(change.value.startMs), clipEnd: formatClipTime(change.value.endMs) };
    case "clipStart": return { ...next, clipStart: change.value };
    case "clipEnd": return { ...next, clipEnd: change.value };
  }
}
export function buildBilibiliSelectionIntent(state: BilibiliState): SelectionIntent | undefined {
  const intent: SelectionIntent = { mode: state.mode };
  if (state.compatibilityMode) intent.compatibilityMode = true;
  if (state.mode === "manual" || state.mode === "video_only") {
    const format = state.data?.media.formats.find(row => row.id === state.videoFormatId
      && row.kind !== "audio_only" && state.data?.rankedVideoFormatIds.includes(row.id));
    if (!format || state.outputContainer === ""
      || !(state.compatibilityMode ? state.outputContainer === "mp4" : supportedVideoContainers(format.videoCodec).includes(state.outputContainer))) return undefined;
    intent.videoFormatId = format.id; intent.outputContainer = state.outputContainer;
  }
  if (state.clipEnabled) {
    const startMs = parseClipTime(state.clipStart); const endMs = parseClipTime(state.clipEnd);
    const full = fullClipRange(state.data?.media.durationSeconds);
    if (!bilibiliClipAvailable(state) || !full || startMs === undefined || endMs === undefined
      || startMs >= endMs || endMs > full.endMs) return undefined;
    intent.clip = { startMs, endMs };
  }
  return intent;
}
const publicError = (error: unknown) => error instanceof BilibiliAnalysisIpcError ? error.code : "analysis_unavailable" as const;
function selectionMatches(previous: BilibiliState, id: string, intent: SelectionIntent): boolean {
  return previous.data?.analysisId === id && JSON.stringify(buildBilibiliSelectionIntent(previous)) === JSON.stringify(intent);
}
const shareVideoUrl = /\s(https:\/\/(?:www\.)?bilibili\.com\/video\/(?:BV[A-Za-z0-9]{10}|av[0-9]{1,20})\/?(?:[?#]\S*)?)$/u;
function bilibiliInputUrl(input: string): string | undefined {
  const value = input.trim();
  if (!value || input.includes("\\") || /[\p{Cc}\u2028\u2029]/u.test(input)
    || new TextEncoder().encode(value).length > 2048 || (value.match(/[a-z][a-z\d+.-]*:\/\//giu)?.length ?? 0) > 1) return undefined;
  // Preserve the exact candidate: native validation must see raw authorities, paths and selectors.
  return /\s/u.test(value) ? shareVideoUrl.exec(value)?.[1] : value;
}
export async function requestBilibiliAnalysis(state: BilibiliState, generation: Generation, publish: Publish,
  ipc: Pick<BilibiliIpc, "analyze"> = bilibiliIpc): Promise<BilibiliAnalysisPreview | null> {
  const ticket = ++generation.current;
  const url = bilibiliInputUrl(state.url);
  if (!url) {
    publish(previous => ({ ...previous, data: null, outcome: null, status: "error", error: "invalid_url" })); return null;
  }
  publish(previous => ({ ...createBilibiliState(), url: previous.url, status: "analyzing" }));
  try {
    const data = await ipc.analyze(url, { mode: "best_available" });
    if (generation.current !== ticket) return null;
    let applied = false;
    publish(previous => {
      if (generation.current !== ticket || previous.url !== state.url) return previous;
      applied = true;
      return { ...previous, data, outcome: data.outcome, status: "analyzed", error: null };
    });
    return applied ? data : null;
  } catch (error) {
    publish(previous => generation.current === ticket && previous.url === state.url
      ? { ...previous, data: null, outcome: null, status: "error", error: publicError(error) } : previous);
    return null;
  }
}
export async function requestBilibiliSelection(state: BilibiliState, generation: Generation, publish: Publish,
  ipc: Pick<BilibiliIpc, "preview"> = bilibiliIpc): Promise<void> {
  if (!state.data) return;
  const ticket = ++generation.current;
  const intent = buildBilibiliSelectionIntent(state);
  if (!intent) {
    publish(previous => ({ ...previous, outcome: null, status: "error", error: state.clipEnabled ? "clip" : "manual" })); return;
  }
  const id = state.data.analysisId;
  publish(previous => ({ ...previous, outcome: null, status: "previewing", error: null }));
  try {
    const data = await ipc.preview(id, intent);
    publish(previous => generation.current === ticket && selectionMatches(previous, id, intent)
      ? { ...previous, data, outcome: data.outcome, status: "ready", error: null } : previous);
  } catch (error) {
    publish(previous => generation.current === ticket && selectionMatches(previous, id, intent)
      ? { ...previous, outcome: null, status: "error", error: publicError(error) } : previous);
  }
}
export async function requestBilibiliResolutions(data: BilibiliAnalysisPreview, mode: "manual" | "video_only", compatibilityMode: boolean,
  generation: Generation, publish: (result: ResolutionPlansResult) => void,
  ipc: Pick<BilibiliIpc, "resolutionOptions"> = bilibiliIpc): Promise<void> {
  const ticket = ++generation.current;
  try {
    const plans = await ipc.resolutionOptions(data, mode, compatibilityMode);
    if (generation.current === ticket) publish({ analysisId: data.analysisId, mode, compatibilityMode, plans, failed: false });
  } catch {
    if (generation.current === ticket) publish({ analysisId: data.analysisId, mode, compatibilityMode, plans: [], failed: true });
  }
}
export async function requestBilibiliCover(analysisId: string, generation: Generation, publish: (result: BilibiliCoverState) => void,
  ipc: Pick<BilibiliIpc, "cover"> = bilibiliIpc): Promise<void> {
  const ticket = ++generation.current;
  publish({ analysisId, status: "loading", payload: null });
  try {
    const payload = await ipc.cover(analysisId);
    if (generation.current === ticket) publish({ analysisId, status: "ready", payload });
  } catch {
    if (generation.current === ticket) publish({ analysisId, status: "error", payload: null });
  }
}
