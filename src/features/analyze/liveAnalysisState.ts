import type { MediaFormat } from "../../shared/contracts/media";
import type { LiveAnalysisPreview } from "../../shared/contracts/liveAnalysis";
import type { VideoContainer, ClipRange, SelectionIntent, SelectionMode, SelectionOutcome } from "../../shared/contracts/selection";
import {
  LiveAnalysisError,
  analyzeMedia,
  previewMediaSelection,
  type AnalysisErrorCode,
} from "../../shared/ipc/liveAnalysis";
import { supportedVideoContainers } from "../../shared/contracts/selection";
import { parseClipTime, formatClipTime, fullClipRange } from "./clipRange";

export type LiveAnalysisStatus = "idle" | "analyzing" | "previewing" | "analyzed" | "ready" | "error";
export type LiveAnalysisUiError = AnalysisErrorCode | "manual" | "clip";

export interface LiveAnalysisState {
  compatibilityMode: boolean;
  url: string;
  mode: SelectionMode;
  videoFormatId: string;
  outputContainer: VideoContainer | "";
  clipEnabled: boolean;
  clipStart: string;
  clipEnd: string;
  data: LiveAnalysisPreview | null;
  outcome: SelectionOutcome | null;
  status: LiveAnalysisStatus;
  error: LiveAnalysisUiError | null;
}

export type LiveAnalysisChange =
  | { type: "url" | "video" | "clipStart" | "clipEnd"; value: string }
  | { type: "mode"; value: SelectionMode }
  | { type: "videoChoice"; value: {videoFormatId: string; outputContainer: VideoContainer} }
  | { type: "clipRange"; value: ClipRange }
  | { type: "clipEnabled" | "compatibilityMode"; value: boolean };

export interface LiveAnalysisGeneration {
  current: number;
}

type Publish = (update: (previous: LiveAnalysisState) => LiveAnalysisState) => void;

const emptySourceInputs = {
  compatibilityMode: false,
  videoFormatId: "",
  outputContainer: "" as const,
  clipEnabled: false,
  clipStart: "",
  clipEnd: "",
};

export function createLiveAnalysisState(): LiveAnalysisState {
  return {
    url: "",
    mode: "best_available",
    ...emptySourceInputs,
    data: null,
    outcome: null,
    status: "idle",
    error: null,
  };
}

export function validateLiveUrl(value: string): string | undefined {
  if (value.includes("\\") || /\p{Cc}/u.test(value)) return undefined;
  const trimmed = value.trim();
  if (!trimmed || new TextEncoder().encode(trimmed).length > 2048) return undefined;
  return trimmed;
}

export function changeLiveAnalysis(state: LiveAnalysisState, change: LiveAnalysisChange): LiveAnalysisState {
  if ((change.type === "video" && change.value === state.videoFormatId)
    || (change.type === "videoChoice" && change.value.videoFormatId === state.videoFormatId && change.value.outputContainer === state.outputContainer)
    || (change.type === "mode" && change.value === state.mode)
    || (change.type === "compatibilityMode" && change.value === state.compatibilityMode)
    || (change.type === "clipEnabled" && change.value === state.clipEnabled)
    || (change.type === "clipStart" && change.value === state.clipStart)
    || (change.type === "clipEnd" && change.value === state.clipEnd)) return state;
  if (change.type === "url") {
    return {
      ...createLiveAnalysisState(),
      url: change.value,
    };
  }

  const next: LiveAnalysisState = { ...state, outcome: null, status: "idle", error: null };
  switch (change.type) {
    case "compatibilityMode":
      return {...next, compatibilityMode: change.value, outputContainer: change.value && next.videoFormatId !== "" ? "mp4" : ""};
    case "mode":
      return change.value === "audio_only"
        ? { ...next, mode: change.value, clipEnabled: false, clipStart: "", clipEnd: "" }
        : { ...next, mode: change.value,
          videoFormatId: next.videoFormatId };
    case "videoChoice": return { ...next, ...change.value };
    case "video":
      return { ...next, videoFormatId: change.value, outputContainer: "" as const };
    case "clipEnabled":
      if (change.value && next.clipStart === "" && next.clipEnd === "") {
        const range = fullClipRange(next.data?.media.durationSeconds);
        if (range && liveClipIsAvailable(next)) return { ...next, clipEnabled: true, clipStart: formatClipTime(range.startMs), clipEnd: formatClipTime(range.endMs) };
      }
      return { ...next, clipEnabled: change.value };
    case "clipRange":
      return { ...next, clipStart: formatClipTime(change.value.startMs), clipEnd: formatClipTime(change.value.endMs) };
    case "clipStart":
      return { ...next, clipStart: change.value };
    case "clipEnd":
      return { ...next, clipEnd: change.value };
  }
}

export function manualLiveOptions(state: LiveAnalysisState): { video: MediaFormat[]; audio: MediaFormat[] } {
  if (!state.data) return { video: [], audio: [] };
  const formats = new Map(state.data.media.formats.map((format) => [format.id, format]));
  return {
    video: state.data.rankedVideoFormatIds.flatMap((id) => {
      const format = formats.get(id);
      return format && format.kind !== "audio_only" ? [format] : [];
    }),
    audio: state.data.rankedAudioFormatIds.flatMap((id) => {
      const format = formats.get(id);
      return format?.kind === "audio_only" ? [format] : [];
    }),
  };
}

export function liveClipIsAvailable(state: LiveAnalysisState): boolean {
  return state.mode !== "audio_only"
    && fullClipRange(state.data?.media.durationSeconds) !== undefined;
}

export function buildLiveSelectionIntent(
  state: LiveAnalysisState,
): { intent: SelectionIntent } | { error: "manual" | "clip" } {
  const intent: SelectionIntent = { mode: state.mode };
  if (state.compatibilityMode) intent.compatibilityMode = true;
  if (state.mode === "manual" || state.mode === "video_only") {
    const choices = manualLiveOptions(state);
    const video = choices.video.find((format) => format.id === state.videoFormatId);
    if (!video || (state.outputContainer !== "" && !(state.compatibilityMode ? state.outputContainer === "mp4" : supportedVideoContainers(video.videoCodec).includes(state.outputContainer)))) return { error: "manual" };
    intent.videoFormatId = video.id;
    if (state.outputContainer !== "") intent.outputContainer = state.outputContainer;
  }
  if (state.clipEnabled) {
    const startMs = parseClipTime(state.clipStart);
    const endMs = parseClipTime(state.clipEnd);
    const duration = state.data?.media.durationSeconds;
    if (!liveClipIsAvailable(state)
      || startMs === undefined
      || endMs === undefined
      || duration === undefined
      || startMs >= endMs
      || endMs > Math.floor(duration * 1000)) {
      return { error: "clip" };
    }
    intent.clip = { startMs, endMs };
  }
  return { intent };
}

function publicError(error: unknown): AnalysisErrorCode {
  return error instanceof LiveAnalysisError ? error.code : "analysis_unavailable";
}

export async function requestLiveAnalysis(
  state: LiveAnalysisState,
  generation: LiveAnalysisGeneration,
  publish: Publish,
): Promise<LiveAnalysisPreview | null> {
  const url = validateLiveUrl(state.url);
  if (!url) {
    publish((previous) => ({ ...previous, outcome: null, status: "error", error: "invalid_url" }));
    return null;
  }

  const request = ++generation.current;
  publish((previous) => ({ ...previous, outcome: null, status: "analyzing", error: null }));
  try {
    const data = await analyzeMedia(url, { mode: "best_available" });
    if (generation.current !== request) return null;
    publish((previous) => generation.current === request ? {
      ...previous,
      mode: "best_available",
      ...emptySourceInputs,
      data,
      outcome: data.outcome,
      status: "analyzed",
      error: null,
    } : previous);
    return data;
  } catch (error) {
    if (generation.current !== request) return null;
    publish((previous) => generation.current === request
      ? { ...previous, data: null, outcome: null, status: "error", error: publicError(error) }
      : previous);
    return null;
  }
}

export async function requestLiveSelection(
  state: LiveAnalysisState,
  generation: LiveAnalysisGeneration,
  publish: Publish,
): Promise<LiveAnalysisPreview | null> {
  if (!state.data) return null;
  const built = buildLiveSelectionIntent(state);
  if ("error" in built) {
    publish((previous) => ({ ...previous, outcome: null, status: "error", error: built.error }));
    return null;
  }

  const request = ++generation.current;
  const analysisId = state.data.analysisId;
  publish((previous) => ({ ...previous, outcome: null, status: "previewing", error: null }));
  try {
    const data = await previewMediaSelection(analysisId, built.intent);
    if (generation.current !== request) return null;
    publish((previous) => generation.current === request
      ? { ...previous, data, outcome: data.outcome, status: "ready", error: null }
      : previous);
    return data;
  } catch (error) {
    if (generation.current !== request) return null;
    publish((previous) => generation.current === request
      ? { ...previous, outcome: null, status: "error", error: publicError(error) }
      : previous);
    return null;
  }
}
