import type { MediaFormat, Platform } from "../../shared/contracts/media";
import type { VideoContainer, ClipRange, SelectionIntent, SelectionMode, SelectionOutcome, SelectionPreview } from "../../shared/contracts/selection";
import { previewMediaSelectionFixture } from "../../shared/ipc/selection";
import { supportedVideoContainers } from "../../shared/contracts/selection";
import { parseClipTime, formatClipTime, fullClipRange } from "./clipRange";

export interface SelectionState {
  compatibilityMode: boolean;
  platform: Platform;
  mode: SelectionMode;
  videoFormatId: string;
  outputContainer: VideoContainer | "";
  clipEnabled: boolean;
  clipStart: string;
  clipEnd: string;
  data: SelectionPreview | null;
  outcome: SelectionOutcome | null;
  status: "idle" | "loading" | "ready" | "error";
  error: "preview" | "manual" | "clip" | null;
}
export type SelectionChange =
  | { type: "platform"; value: Platform }
  | { type: "mode"; value: SelectionMode }
  | { type: "videoChoice"; value: {videoFormatId: string; outputContainer: VideoContainer} }
  | { type: "clipRange"; value: ClipRange }
  | { type: "video" | "clipStart" | "clipEnd"; value: string }
  | { type: "clipEnabled" | "compatibilityMode"; value: boolean };
export type Generation = { current: number };

const emptySourceInputs = { compatibilityMode: false, videoFormatId: "", outputContainer: "" as const, clipEnabled: false, clipStart: "", clipEnd: "" };

export function createSelectionState(): SelectionState {
  return { platform: "youtube", mode: "best_available", ...emptySourceInputs, data: null, outcome: null, status: "idle", error: null };
}

export function changeSelection(state: SelectionState, change: SelectionChange): SelectionState {
  const next: SelectionState = { ...state, outcome: null, error: null, status: "idle" };
  switch (change.type) {
    case "compatibilityMode": return {...next, compatibilityMode:change.value,outputContainer:change.value && next.videoFormatId !== "" ? "mp4" : ""};
    case "platform": return { ...next, ...emptySourceInputs, platform: change.value, data: null };
    case "mode": return change.value === "audio_only"
      ? { ...next, mode: change.value, clipEnabled: false, clipStart: "", clipEnd: "" }
      : { ...next, mode: change.value,
        videoFormatId: next.videoFormatId };
    case "videoChoice": return { ...next, ...change.value };
    case "video": return { ...next, videoFormatId: change.value, outputContainer: "" as const };
    case "clipEnabled": {
      const range = fullClipRange(next.data?.media.durationSeconds);
      return change.value && next.clipStart === "" && next.clipEnd === "" && range && clipIsAvailable(next)
        ? { ...next, clipEnabled: true, clipStart: formatClipTime(range.startMs), clipEnd: formatClipTime(range.endMs) }
        : { ...next, clipEnabled: change.value };
    }
    case "clipRange": return { ...next, clipStart: formatClipTime(change.value.startMs), clipEnd: formatClipTime(change.value.endMs) };
    case "clipStart": return { ...next, clipStart: change.value };
    case "clipEnd": return { ...next, clipEnd: change.value };
  }
}

export function manualOptions(state: SelectionState): { video: MediaFormat[]; audio: MediaFormat[] } {
  const data = state.data;
  if (!data) return { video: [], audio: [] };
  const formats = new Map(data.media.formats.map((format) => [format.id, format]));
  return {
    video: data.rankedVideoFormatIds.flatMap((id) => {
      const format = formats.get(id);
      return format && format.kind !== "audio_only" ? [format] : [];
    }),
    audio: data.rankedAudioFormatIds.flatMap((id) => {
      const format = formats.get(id);
      return format?.kind === "audio_only" ? [format] : [];
    }),
  };
}

export function clipIsAvailable(state: SelectionState): boolean {
  return state.mode !== "audio_only" && fullClipRange(state.data?.media.durationSeconds) !== undefined;
}

export function buildSelectionIntent(state: SelectionState): { intent: SelectionIntent } | { error: "manual" | "clip" } {
  const intent: SelectionIntent = { mode: state.mode };
  if (state.compatibilityMode) intent.compatibilityMode = true;
  // The first manual request can load the source and return its explicit unavailable outcome.
  if ((state.mode === "manual" || state.mode === "video_only") && state.data) {
    const choices = manualOptions(state);
    const video = choices.video.find((format) => format.id === state.videoFormatId);
    if (!video || (state.outputContainer !== "" && !(state.compatibilityMode ? state.outputContainer === "mp4" : supportedVideoContainers(video.videoCodec).includes(state.outputContainer)))) return { error: "manual" };
    intent.videoFormatId = video.id;
    if (state.outputContainer !== "") intent.outputContainer = state.outputContainer;
  }
  if (state.clipEnabled) {
    const startMs = parseClipTime(state.clipStart);
    const endMs = parseClipTime(state.clipEnd);
    const duration = state.data?.media.durationSeconds;
    if (!clipIsAvailable(state) || startMs === undefined || endMs === undefined
      || duration === undefined || startMs >= endMs || endMs > Math.floor(duration * 1000)) return { error: "clip" };
    intent.clip = { startMs, endMs };
  }
  return { intent };
}

export async function requestSelection(
  state: SelectionState,
  generation: Generation,
  publish: (update: (previous: SelectionState) => SelectionState) => void,
): Promise<void> {
  const request = ++generation.current;
  const built = buildSelectionIntent(state);
  if ("error" in built) {
    publish((previous) => ({ ...previous, outcome: null, status: "error", error: built.error }));
    return;
  }
  publish((previous) => ({ ...previous, outcome: null, error: null, status: "loading" }));
  try {
    const data = await previewMediaSelectionFixture(state.platform, built.intent);
    if (generation.current !== request) return;
    publish((previous) => {
      if (generation.current !== request) return previous;
      const changedMedia = previous.data !== null && JSON.stringify(previous.data.media) !== JSON.stringify(data.media);
      return changedMedia
        ? { ...previous, ...emptySourceInputs, data, outcome: null, status: "idle", error: null }
        : { ...previous, data, outcome: data.outcome, status: "ready", error: null };
    });
  } catch {
    if (generation.current !== request) return;
    publish((previous) => generation.current === request
      ? { ...previous, outcome: null, status: "error", error: "preview" } : previous);
  }
}
