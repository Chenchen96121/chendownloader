import { isFormatId, isMediaInfo, type MediaInfo, type Platform } from "./media";

export type VideoContainer = "mp4" | "webm";
export type TargetAudioCodec = "aac" | "opus" | "vorbis" | "mp3" | "flac" | "alac";
export type OutputContainer = VideoContainer | "mkv" | "m4a" | "mp3" | "flac";
export type ContainerSelection = "automatic" | "explicit" | "compatibility";
export type AudioProcessing = "copy" | "transcode";
export type SelectionMode = "best_available" | "video_only" | "audio_only" | "manual";
export type SelectionOperation = "keep" | "merge" | "extract_audio" | "extract_video";
export type SelectionWarning = "metadata_incomplete" | "container_unverified" | "output_size_unknown" | "clip_boundary_may_vary" | "clip_transfer_may_exceed_range" | "compatibility_may_reencode" | "hdr_to_sdr";
export type SelectionUnavailableCode = "format_unavailable" | "invalid_selection" | "invalid_clip" | "clip_duration_unknown" | "compatibility_unavailable";
export interface ClipRange { startMs: number; endMs: number }
export interface SelectionIntent {
  compatibilityMode?: boolean;
  mode: SelectionMode;
  videoFormatId?: string;
  outputContainer?: VideoContainer;
  clip?: ClipRange;
}
export interface SelectionPlan {
  compatibilityMode: boolean;
  containerSelection: ContainerSelection;
  videoProcessing?: AudioProcessing;
  outputVideoCodec?: string;
  mode: SelectionMode;
  sourceFormatIds: string[];
  videoFormatId?: string;
  audioFormatId?: string;
  operation: SelectionOperation;
  outputContainer: OutputContainer;
  outputAudioCodec?: TargetAudioCodec;
  audioProcessing?: AudioProcessing;
  clip?: ClipRange;
  estimatedSizeBytes?: number;
  warnings: SelectionWarning[];
}
export type SelectionOutcome =
  | { status: "selected"; plan: SelectionPlan }
  | { status: "unavailable"; code: SelectionUnavailableCode };
export interface SelectionPreview {
  media: MediaInfo;
  rankedVideoFormatIds: string[];
  rankedAudioFormatIds: string[];
  outcome: SelectionOutcome;
}

const modes = new Set<unknown>(["best_available", "video_only", "audio_only", "manual"]);
const unavailableCodes = new Set<unknown>(["format_unavailable", "invalid_selection", "invalid_clip", "clip_duration_unknown", "compatibility_unavailable"]);
const warningOrder: readonly string[] = ["metadata_incomplete", "container_unverified", "output_size_unknown", "clip_boundary_may_vary", "clip_transfer_may_exceed_range", "compatibility_may_reencode", "hdr_to_sdr"];
const intentFields = ["mode", "videoFormatId", "outputContainer", "clip", "compatibilityMode"];
const planFields = ["mode", "sourceFormatIds", "videoFormatId", "audioFormatId", "operation", "outputContainer", "outputAudioCodec", "audioProcessing", "clip", "estimatedSizeBytes", "warnings", "compatibilityMode", "containerSelection", "videoProcessing", "outputVideoCodec"];

function record(value: unknown, fields: readonly string[], required: readonly string[]): value is Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const prototype = Object.getPrototypeOf(value);
  return (prototype === Object.prototype || prototype === null)
    && Object.keys(value).every((key) => fields.includes(key))
    && required.every((key) => Object.hasOwn(value, key));
}

function optional(row: Record<string, unknown>, key: string, valid: (value: unknown) => boolean): boolean {
  return !Object.hasOwn(row, key) || valid(row[key]);
}

function endpoint(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 && value <= 31_536_000_000;
}

function clipRange(value: unknown): value is ClipRange {
  return record(value, ["startMs", "endMs"], ["startMs", "endMs"])
    && endpoint(value.startMs) && endpoint(value.endMs) && value.startMs < value.endMs;
}

export function isVideoContainer(value: unknown): value is VideoContainer { return value === "mp4" || value === "webm"; }
export function isSelectionIntent(value: unknown): value is SelectionIntent {
  return record(value, intentFields, ["mode"]) && modes.has(value.mode)
    && optional(value, "compatibilityMode", (flag) => typeof flag === "boolean")
    && optional(value, "videoFormatId", isFormatId) && optional(value, "outputContainer", isVideoContainer)
    && optional(value, "clip", clipRange)
    && (value.mode === "manual" || value.mode === "video_only" || (!Object.hasOwn(value, "videoFormatId") && !Object.hasOwn(value, "outputContainer")))
    && (value.mode !== "audio_only" || !Object.hasOwn(value, "clip"));
}

function uniqueIds(value: unknown): value is string[] {
  if (!Array.isArray(value)) return false;
  const seen = new Set<string>();
  for (const id of value) {
    if (!isFormatId(id) || seen.has(id)) return false;
    seen.add(id);
  }
  return true;
}

function rankedIds(value: unknown, media: MediaInfo, video: boolean): value is string[] {
  if (!uniqueIds(value)) return false;
  const eligible = new Set(media.formats.filter((format) => video ? format.kind !== "audio_only" : format.kind !== "video_only").map((format) => format.id));
  // Check coverage and role, never reconstruct the native recommendation order.
  return value.length === eligible.size && value.every((id) => eligible.has(id));
}

function warnings(value: unknown): value is SelectionWarning[] {
  if (!Array.isArray(value)) return false;
  let previous = -1;
  for (const warning of value) {
    const index = warningOrder.indexOf(warning);
    if (index <= previous) return false;
    previous = index;
  }
  return true;
}

function videoGroup(codec: string | undefined): number {
  const alias = codec?.split(".")[0]?.toLowerCase();
  if (alias === "av1" || alias === "av01") return 4;
  if (alias === "vp9" || alias === "vp09") return 3;
  if (["hevc", "h265", "hev1", "hvc1"].includes(alias ?? "")) return 2;
  if (["h264", "avc1", "avc3"].includes(alias ?? "")) return 1;
  return 0;
}

function audioGroup(codec: string | undefined): number {
  switch (codec?.toLowerCase()) {
    case "flac": case "alac": return 5;
    case "opus": return 4;
    case "vorbis": return 3;
    case "aac": case "mp4a.40.2": case "mp4a.40.5": case "mp4a.40.29": return 2;
    case "mp3": return 1;
    default: return 0;
  }
}

export function supportedVideoContainers(codec: string | undefined): readonly VideoContainer[] {
  const alias = codec?.split(".")[0]?.toLowerCase();
  if (videoGroup(codec) === 4 || videoGroup(codec) === 3) return ["mp4", "webm"];
  if (videoGroup(codec) !== 0) return ["mp4"];
  if (alias === "vp8" || alias === "vp08") return ["webm"];
  return [];
}
function normalizedAudio(codec: string | undefined): TargetAudioCodec | undefined {
  if (audioGroup(codec) === 2) return "aac";
  const name = codec?.toLowerCase();
  return ["opus", "vorbis", "mp3", "flac", "alac"].includes(name ?? "") ? name as TargetAudioCodec : undefined;
}
function normalizedVideo(codec: string | undefined): string | undefined {
  const group = videoGroup(codec);
  if (group) return [undefined, "h264", "hevc", "vp9", "av1"][group];
  return ["vp8", "vp08"].includes(codec?.split(".")[0]?.toLowerCase() ?? "") ? "vp8" : undefined;
}
function losslessContainers(video: string, audio?: TargetAudioCodec): OutputContainer[] {
  const webmAudio = audio === undefined || audio === "opus" || audio === "vorbis";
  if (video === "h264" || video === "hevc") return ["mp4", "mkv"];
  if (video === "av1" || video === "vp9") return webmAudio ? ["mp4", "webm", "mkv"] : ["mp4", "mkv"];
  if (video === "vp8") return webmAudio ? ["webm", "mkv"] : ["mkv"];
  return [];
}
function unavailableMatches(code: unknown, media: MediaInfo, intent: SelectionIntent): boolean {
  if (intent.clip) {
    const duration = media.durationSeconds;
    if (duration === undefined || duration <= 0) return code === "clip_duration_unknown";
    if (intent.clip.endMs > Math.floor(duration * 1000)) return code === "invalid_clip";
  }
  if (code === "compatibility_unavailable") return intent.compatibilityMode === true;
  return intent.mode === "manual" || (intent.mode === "video_only" && intent.videoFormatId !== undefined)
    ? code === "invalid_selection" : code === "format_unavailable";
}

function selectionPlan(value: unknown, media: MediaInfo, intent: SelectionIntent): value is SelectionPlan {
  if (!record(value, planFields, ["mode", "sourceFormatIds", "operation", "outputContainer", "warnings", "compatibilityMode", "containerSelection"])
    || value.mode !== intent.mode || !uniqueIds(value.sourceFormatIds)
    || value.compatibilityMode !== (intent.compatibilityMode ?? false)
    || value.containerSelection !== (intent.compatibilityMode ? "compatibility" : intent.outputContainer ? "explicit" : "automatic")
    || value.sourceFormatIds.length < 1 || value.sourceFormatIds.length > 2
    || !optional(value, "videoFormatId", isFormatId) || !optional(value, "audioFormatId", isFormatId)
    || !["mp4", "webm", "mkv", "m4a", "mp3", "flac"].includes(value.outputContainer as string)
    || !optional(value, "outputAudioCodec", (codec) => ["aac", "opus", "vorbis", "mp3", "flac", "alac"].includes(codec as string))
    || !optional(value, "outputVideoCodec", (codec) => ["h264", "hevc", "av1", "vp9", "vp8"].includes(codec as string))
    || !optional(value, "videoProcessing", (action) => action === "copy" || action === "transcode")
    || !optional(value, "audioProcessing", (action) => action === "copy" || action === "transcode") || !optional(value, "clip", clipRange)
    || !optional(value, "estimatedSizeBytes", (size) => typeof size === "number" && Number.isSafeInteger(size) && size > 0)
    || !warnings(value.warnings)) return false;

  const clip = value.clip as ClipRange | undefined;
  if (clip?.startMs !== intent.clip?.startMs || clip?.endMs !== intent.clip?.endMs) return false;
  if (clip && (value.mode === "audio_only" || media.durationSeconds === undefined
    || media.durationSeconds <= 0 || clip.endMs > Math.floor(media.durationSeconds * 1000))) return false;
  if ((clip || value.compatibilityMode || value.operation === "extract_audio" || value.operation === "extract_video") && Object.hasOwn(value, "estimatedSizeBytes")) return false;
  if (!value.warnings.includes("container_unverified")
    || value.warnings.includes("output_size_unknown") !== !Object.hasOwn(value, "estimatedSizeBytes")
    || value.warnings.includes("clip_boundary_may_vary") !== Boolean(clip)
    || value.warnings.includes("clip_transfer_may_exceed_range") !== Boolean(clip)
    || value.warnings.includes("compatibility_may_reencode") !== value.compatibilityMode) return false;

  const video = media.formats.find((format) => format.id === value.videoFormatId);
  const audio = media.formats.find((format) => format.id === value.audioFormatId);
  if (video) {
    const codec = normalizedVideo(video.videoCodec);
    if (!codec || video.kind === "audio_only" || value.outputVideoCodec !== (value.compatibilityMode ? "h264" : codec)
      || value.videoProcessing !== (value.compatibilityMode ? "transcode" : "copy")) return false;
    if (value.compatibilityMode && ((video.width !== undefined && video.width % 2 !== 0)
      || (video.height !== undefined && video.height % 2 !== 0) || ["DV", "HDR10+", "HDR12"].includes(video.dynamicRange ?? ""))) return false;
  } else if (Object.hasOwn(value, "videoProcessing") || Object.hasOwn(value, "outputVideoCodec")) return false;
  const hdrConversion = Boolean(value.compatibilityMode && video && ["HDR", "HDR10", "HLG"].includes(video.dynamicRange ?? ""));
  if (value.warnings.includes("hdr_to_sdr") !== hdrConversion) return false;
  const originalAudio = normalizedAudio(audio?.audioCodec);
  const originalVideo = normalizedVideo(video?.videoCodec);
  const audioContainer: Record<TargetAudioCodec, OutputContainer> = { aac: "m4a", alac: "m4a", opus: "webm", vorbis: "webm", mp3: "mp3", flac: "flac" };
  const candidates = originalVideo ? losslessContainers(originalVideo, value.mode === "video_only" ? undefined : originalAudio) : originalAudio ? [audioContainer[originalAudio]] : [];
  const expectedContainer = value.compatibilityMode ? video ? "mp4" : "m4a" : intent.outputContainer ?? candidates[0];
  if (value.outputContainer !== expectedContainer || (!value.compatibilityMode && !candidates.includes(value.outputContainer as OutputContainer))
    || (intent.outputContainer !== undefined && value.outputContainer !== intent.outputContainer)) return false;
  if (value.mode === "video_only") {
    if (!video || video.kind === "audio_only" || Object.hasOwn(value, "audioFormatId")
      || value.sourceFormatIds.length !== 1 || value.sourceFormatIds[0] !== video.id
      || (intent.videoFormatId !== undefined && video.id !== intent.videoFormatId)
      || value.operation !== (video.kind === "combined" ? "extract_video" : "keep")
      || Object.hasOwn(value, "outputAudioCodec") || Object.hasOwn(value, "audioProcessing")
      ) return false;
    const incomplete = video.width === undefined || video.height === undefined
      || video.fps === undefined || video.dynamicRange === undefined;
    if (incomplete && !value.warnings.includes("metadata_incomplete")) return false;
    const expectedSize = !clip && !value.compatibilityMode && value.operation === "keep" ? video.fileSize?.bytes : undefined;
    return value.estimatedSizeBytes === expectedSize;
  }
  if (!audio || audio.kind === "video_only") return false;
  if (!originalAudio) return false;
  const target = value.compatibilityMode ? "aac" : originalAudio;
  if (value.outputAudioCodec !== target
    || value.audioProcessing !== (originalAudio === target ? "copy" : "transcode")) return false;
  const incompleteVideo = video && (video.width === undefined || video.height === undefined
    || video.fps === undefined || video.dynamicRange === undefined);
  const incompleteAudio = audio.audioChannels === undefined || audio.audioBitrateKbps === undefined
    || audio.audioSampleRateHz === undefined;
  // Unknown codec families can also cause this native warning; do not classify them here.
  if ((incompleteVideo || incompleteAudio) && !value.warnings.includes("metadata_incomplete")) return false;
  if (intent.mode === "manual" && value.videoFormatId !== intent.videoFormatId) return false;

  if (value.mode === "audio_only") {
    return !Object.hasOwn(value, "videoFormatId") && value.sourceFormatIds.length === 1
      && value.sourceFormatIds[0] === audio.id
      && value.operation === (audio.kind === "audio_only" ? "keep" : "extract_audio")
      && value.estimatedSizeBytes === (!value.compatibilityMode && audio.kind === "audio_only" ? audio.fileSize?.bytes : undefined);
  }
  if (!video || video.kind === "audio_only") return false;
  let expectedSize: number | undefined;
  if (!clip && !value.compatibilityMode && value.audioProcessing === "copy" && video.fileSize) {
    const bytes = value.operation === "merge"
      ? video.kind === "video_only" && audio.kind === "audio_only" && audio.fileSize ? video.fileSize.bytes + audio.fileSize.bytes : undefined
      : video.fileSize.bytes;
    if (bytes !== undefined && Number.isSafeInteger(bytes)) expectedSize = bytes;
  }
  if (value.estimatedSizeBytes !== expectedSize) return false;
  if (value.operation === "keep") {
    return video.kind === "combined" && video.id === audio.id
      && value.sourceFormatIds.length === 1 && value.sourceFormatIds[0] === video.id;
  }
  return value.operation === "merge" && video.id !== audio.id
    && value.sourceFormatIds.length === 2 && value.sourceFormatIds[0] === video.id && value.sourceFormatIds[1] === audio.id;
}

export function isSelectionPreview(value: unknown, platform: Platform, intent: SelectionIntent): value is SelectionPreview {
  const fields = ["media", "rankedVideoFormatIds", "rankedAudioFormatIds", "outcome"];
  if (!isSelectionIntent(intent) || !record(value, fields, fields)
    || !isMediaInfo(value.media) || value.media.platform !== platform
    || !rankedIds(value.rankedVideoFormatIds, value.media, true)
    || !rankedIds(value.rankedAudioFormatIds, value.media, false)) return false;
  const outcome = value.outcome;
  if (record(outcome, ["status", "code"], ["status", "code"]) && outcome.status === "unavailable") {
    return unavailableCodes.has(outcome.code) && unavailableMatches(outcome.code, value.media, intent);
  }
  return record(outcome, ["status", "plan"], ["status", "plan"]) && outcome.status === "selected"
    && selectionPlan(outcome.plan, value.media, intent);
}
