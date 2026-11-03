export type Platform = "youtube" | "bilibili" | "x" | "instagram";
export type StreamKind = "video_only" | "audio_only" | "combined";
export type DynamicRange = "SDR" | "HDR" | "HDR10" | "HDR10+" | "HDR12" | "HLG" | "DV";

export interface FileSize {
  bytes: number;
  isApproximate: boolean;
}

export interface MediaFormat {
  id: string;
  container: string;
  kind: StreamKind;
  width?: number;
  height?: number;
  fps?: number;
  videoBitrateKbps?: number;
  audioBitrateKbps?: number;
  audioSampleRateHz?: number;
  audioChannels?: number;
  videoCodec?: string;
  audioCodec?: string;
  dynamicRange?: DynamicRange;
  fileSize?: FileSize;
}

export interface MediaInfo {
  platform: Platform;
  mediaId: string;
  title: string;
  creator?: string;
  durationSeconds?: number;
  formats: MediaFormat[];
}

const platforms = new Set<string>(["youtube", "bilibili", "x", "instagram"]);
const ranges = new Set<string>(["SDR", "HDR", "HDR10", "HDR10+", "HDR12", "HLG", "DV"]);
const mediaFields = new Set(["platform", "mediaId", "title", "creator", "durationSeconds", "formats"]);
const formatFields = new Set(["id", "container", "kind", "width", "height", "fps", "videoBitrateKbps", "audioBitrateKbps", "audioSampleRateHz", "audioChannels", "videoCodec", "audioCodec", "dynamicRange", "fileSize"]);
const sizeFields = new Set(["bytes", "isApproximate"]);
const tokenPattern = /^[A-Za-z0-9._+-]+$/;
const containerPattern = /^[a-z0-9]{1,16}$/;
const invalidScalarPattern = /[\u0000-\u001f\u007f-\u009f\ud800-\udfff]/u;
// Rust str::trim uses Unicode White_Space, not ECMAScript trim (which includes BOM).
const edgeWhitespacePattern = /^\p{White_Space}|\p{White_Space}$/u;

function record(value: unknown, fields: ReadonlySet<string>): value is Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const prototype = Object.getPrototypeOf(value);
  return (prototype === Object.prototype || prototype === null)
    && Object.keys(value).every((key) => fields.has(key));
}

function token(value: unknown, max: number): value is string {
  return typeof value === "string" && value.length <= max && tokenPattern.test(value);
}

export function isFormatId(value: unknown): value is string {
  return token(value, 64);
}

export function isContainer(value: unknown): value is string {
  return typeof value === "string" && containerPattern.test(value);
}

function displayText(value: unknown, max: number): value is string {
  return typeof value === "string" && value.length > 0
    && !edgeWhitespacePattern.test(value) && !invalidScalarPattern.test(value)
    && Array.from(value).length <= max;
}

function optional(row: Record<string, unknown>, key: string, valid: (value: unknown) => boolean): boolean {
  return !Object.hasOwn(row, key) || valid(row[key]);
}

function boundedNumber(value: unknown, min: number, max: number): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= min && value <= max;
}

function dimension(value: unknown): boolean {
  return boundedNumber(value, 1, 16384) && Number.isInteger(value);
}

function codec(value: unknown): boolean {
  return value !== "none" && token(value, 64);
}

function fileSize(value: unknown): boolean {
  return record(value, sizeFields) && typeof value.bytes === "number"
    && Number.isSafeInteger(value.bytes) && value.bytes > 0 && typeof value.isApproximate === "boolean";
}

function mediaFormat(value: unknown): value is MediaFormat {
  if (!record(value, formatFields) || !isFormatId(value.id)
    || !isContainer(value.container)
    || !optional(value, "videoCodec", codec) || !optional(value, "audioCodec", codec)) return false;

  const video = Object.hasOwn(value, "videoCodec");
  const audio = Object.hasOwn(value, "audioCodec");
  const streamsAgree = (value.kind === "video_only" && video && !audio)
    || (value.kind === "audio_only" && !video && audio)
    || (value.kind === "combined" && video && audio);
  if (!streamsAgree) return false;
  if (value.kind === "audio_only"
    && ["width", "height", "fps", "dynamicRange", "videoBitrateKbps"].some((key) => Object.hasOwn(value, key))) return false;
  if (value.kind === "video_only"
    && ["audioBitrateKbps", "audioSampleRateHz", "audioChannels"].some((key) => Object.hasOwn(value, key))) return false;

  return optional(value, "width", dimension) && optional(value, "height", dimension)
    && optional(value, "fps", (fps) => boundedNumber(fps, 0, 240) && fps > 0)
    && optional(value, "videoBitrateKbps", (rate) => boundedNumber(rate, 0, 1_000_000) && rate > 0)
    && optional(value, "audioBitrateKbps", (rate) => boundedNumber(rate, 0, 100_000) && rate > 0)
    && optional(value, "audioSampleRateHz", (rate) => boundedNumber(rate, 1, 768_000) && Number.isInteger(rate))
    && optional(value, "audioChannels", (channels) => boundedNumber(channels, 1, 64) && Number.isInteger(channels))
    && optional(value, "dynamicRange", (range) => typeof range === "string" && ranges.has(range))
    && optional(value, "fileSize", fileSize);
}

export function isPlatform(value: unknown): value is Platform {
  return typeof value === "string" && platforms.has(value);
}

export function isMediaInfo(value: unknown): value is MediaInfo {
  if (!record(value, mediaFields) || !isPlatform(value.platform) || !token(value.mediaId, 128)
    || !displayText(value.title, 512) || !optional(value, "creator", (creator) => displayText(creator, 256))
    || !optional(value, "durationSeconds", (duration) => boundedNumber(duration, 0, 31536000))
    || !Array.isArray(value.formats) || value.formats.length < 1 || value.formats.length > 512) return false;

  const ids = new Set<string>();
  for (const row of value.formats) {
    if (!mediaFormat(row) || ids.has(row.id)) return false;
    ids.add(row.id);
  }
  return true;
}
