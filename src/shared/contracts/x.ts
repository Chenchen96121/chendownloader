import { isAnalysisId } from "./liveAnalysis";
import {
  analysisErrorCodes,
  downloadErrorCodes,
  type AnalysisErrorCode,
  type DownloadErrorCode,
} from "./errors";
import type { DynamicRange, FileSize } from "./media";

export const xMediaKinds = ["image", "video", "animated_video"] as const;
export type XMediaKind = (typeof xMediaKinds)[number];

export const xItemAvailabilities = ["ready", "unavailable"] as const;
export type XItemAvailability = (typeof xItemAvailabilities)[number];

export const xJobStates = [
  "queued", "downloading", "processing", "verifying", "publishing",
  "completed", "partial", "failed", "canceled",
] as const;
export type XJobState = (typeof xJobStates)[number];

export const xItemOutcomeStates = ["completed", "failed", "canceled"] as const;
export type XItemOutcomeState = (typeof xItemOutcomeStates)[number];

export interface XCapabilities {
  postBatch: true;
  images: true;
  videos: true;
  compatibility: true;
  clip: false;
  manualQuality: false;
  audioOnly: false;
  videoOnly: false;
}

export interface XMediaItem {
  itemId: string;
  ordinal: number;
  kind: XMediaKind;
  width?: number;
  height?: number;
  container: string;
  fileSize?: FileSize;
  fps?: number;
  videoBitrateKbps?: number;
  audioBitrateKbps?: number;
  videoCodec?: string;
  audioCodec?: string;
  dynamicRange?: DynamicRange;
  previewAvailable: boolean;
  availability: XItemAvailability;
  unavailableReason?: AnalysisErrorCode;
}

export interface XAnalysisPreview {
  analysisId: string;
  postId: string;
  title: string;
  creator?: string;
  sourcePageUrl: string;
  capabilities: XCapabilities;
  items: XMediaItem[];
}

export interface XPreviewPayload {
  mimeType: "image/jpeg" | "image/png" | "image/webp" | "image/avif";
  dataBase64: string;
}

export interface XDownloadIntent {
  itemIds: string[];
  compatibilityMode: boolean;
}

export interface XItemOutcome {
  itemId: string;
  ordinal: number;
  kind: XMediaKind;
  state: XItemOutcomeState;
  displayName?: string;
  errorCode?: DownloadErrorCode;
}

export interface XDownloadProgress {
  jobId: string;
  state: XJobState;
  itemIndex: number;
  itemCount: number;
  downloadedBytes: number | null;
  totalBytes: number | null;
  percent: number | null;
}

export interface XDownloadResult {
  jobId: string;
  state: "completed" | "partial" | "failed" | "canceled";
  postItemCount: number;
  unavailableCount: number;
  contentChanged: boolean;
  completedCount: number;
  failedCount: number;
  folderDisplayName?: string;
  items: XItemOutcome[];
}

const analysisErrors = new Set<unknown>(analysisErrorCodes);
const downloadErrors = new Set<unknown>(downloadErrorCodes);
const mediaKinds = new Set<unknown>(xMediaKinds);
const availabilities = new Set<unknown>(xItemAvailabilities);
const jobStates = new Set<unknown>(xJobStates);
const outcomeStates = new Set<unknown>(xItemOutcomeStates);
const tokenPattern = /^[A-Za-z0-9._+-]+$/;
const dynamicRanges = new Set<unknown>(["SDR", "HDR", "HDR10", "HDR10+", "HDR12", "HLG", "DV"]);
const postIdPattern = /^[0-9]{1,20}$/;
const containerPattern = /^[a-z0-9]{1,16}$/;
const invalidScalarPattern = /[\u0000-\u001f\u007f-\u009f\ud800-\udfff]/u;
const edgeWhitespacePattern = /^\p{White_Space}|\p{White_Space}$/u;
const bidiPattern = /[\u200e\u200f\u202a-\u202e\u2066-\u2069]/u;
const MAX_PREVIEW_BASE64_BYTES = 11_184_812;

function exactRecord(
  value: unknown,
  required: readonly string[],
  optional: readonly string[] = [],
): value is Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const prototype = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null) return false;
  const candidate = value as Record<string, unknown>;
  const allowed = new Set([...required, ...optional]);
  return required.every((field) => Object.hasOwn(candidate, field))
    && Object.keys(candidate).every((field) => allowed.has(field));
}

function optional(
  value: Record<string, unknown>,
  key: string,
  valid: (candidate: unknown) => boolean,
): boolean {
  return !Object.hasOwn(value, key) || valid(value[key]);
}

function displayText(value: unknown, max: number): value is string {
  return typeof value === "string" && value.length > 0
    && !edgeWhitespacePattern.test(value) && !invalidScalarPattern.test(value)
    && Array.from(value).length <= max;
}

function safeDisplayName(value: unknown, max: number): value is string {
  return displayText(value, max) && !/[\\/:]/u.test(value) && !bidiPattern.test(value);
}

function positiveInteger(value: unknown, max: number): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= 1 && value <= max;
}

function optionalDimension(value: unknown): boolean {
  return positiveInteger(value, 16_384);
}

function codec(value: unknown): value is string {
  return typeof value === "string" && value !== "none"
    && value.length > 0 && value.length <= 64 && tokenPattern.test(value);
}

function fileSize(value: unknown): value is FileSize {
  return exactRecord(value, ["bytes", "isApproximate"])
    && typeof value.bytes === "number" && Number.isSafeInteger(value.bytes) && value.bytes > 0
    && typeof value.isApproximate === "boolean";
}

function exactCapabilities(value: unknown): value is XCapabilities {
  return exactRecord(value, [
    "postBatch", "images", "videos", "compatibility", "clip", "manualQuality",
    "audioOnly", "videoOnly",
  ])
    && value.postBatch === true
    && value.images === true
    && value.videos === true
    && value.compatibility === true
    && value.clip === false
    && value.manualQuality === false
    && value.audioOnly === false
    && value.videoOnly === false;
}

export function isXMediaItem(value: unknown): value is XMediaItem {
  if (!exactRecord(
    value,
    ["itemId", "ordinal", "kind", "container", "previewAvailable", "availability"],
    ["width", "height", "fileSize", "fps", "videoBitrateKbps", "audioBitrateKbps", "videoCodec", "audioCodec", "dynamicRange", "unavailableReason"],
  )
    || !isAnalysisId(value.itemId)
    || !positiveInteger(value.ordinal, 32)
    || !mediaKinds.has(value.kind)
    || typeof value.container !== "string" || !containerPattern.test(value.container)
    || typeof value.previewAvailable !== "boolean"
    || !availabilities.has(value.availability)
    || !optional(value, "width", optionalDimension)
    || !optional(value, "height", optionalDimension)
    || !optional(value, "fileSize", fileSize)
    || !optional(value, "fps", (fps) => typeof fps === "number" && Number.isFinite(fps) && fps > 0 && fps <= 240)
    || !optional(value, "videoBitrateKbps", (rate) => typeof rate === "number" && Number.isFinite(rate) && rate > 0 && rate <= 1_000_000)
    || !optional(value, "audioBitrateKbps", (rate) => typeof rate === "number" && Number.isFinite(rate) && rate > 0 && rate <= 100_000)
    || !optional(value, "videoCodec", codec)
    || !optional(value, "audioCodec", codec)
    || !optional(value, "dynamicRange", (range) => dynamicRanges.has(range))
    || !optional(value, "unavailableReason", (reason) => analysisErrors.has(reason))) return false;

  if (value.kind === "image"
    && ["fps", "videoBitrateKbps", "audioBitrateKbps", "videoCodec", "audioCodec", "dynamicRange"]
      .some((field) => Object.hasOwn(value, field))) return false;
  if (value.kind !== "image"
    && ((["videoBitrateKbps", "dynamicRange"].some((field) => Object.hasOwn(value, field))
      && !Object.hasOwn(value, "videoCodec"))
      || (Object.hasOwn(value, "audioBitrateKbps") && !Object.hasOwn(value, "audioCodec")))) return false;
  return (value.availability === "ready" && !Object.hasOwn(value, "unavailableReason"))
    || (value.availability === "unavailable" && Object.hasOwn(value, "unavailableReason"));
}

export function isXAnalysisPreview(value: unknown): value is XAnalysisPreview {
  if (!exactRecord(
    value,
    ["analysisId", "postId", "title", "sourcePageUrl", "capabilities", "items"],
    ["creator"],
  )
    || !isAnalysisId(value.analysisId)
    || typeof value.postId !== "string" || !postIdPattern.test(value.postId)
    || !displayText(value.title, 512)
    || !optional(value, "creator", (creator) => displayText(creator, 256))
    || value.sourcePageUrl !== `https://x.com/i/web/status/${value.postId}`
    || !exactCapabilities(value.capabilities)
    || !Array.isArray(value.items) || value.items.length < 1 || value.items.length > 32) return false;

  const ids = new Set<string>();
  for (let index = 0; index < value.items.length; index += 1) {
    const item = value.items[index];
    if (!isXMediaItem(item) || item.ordinal !== index + 1 || ids.has(item.itemId)) return false;
    ids.add(item.itemId);
  }
  return true;
}

function base64Payload(value: unknown): value is string {
  if (typeof value !== "string" || value.length === 0
    || value.length > MAX_PREVIEW_BASE64_BYTES || value.length % 4 !== 0) return false;
  return /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/u.test(value);
}

export function isXPreviewPayload(value: unknown): value is XPreviewPayload {
  return exactRecord(value, ["mimeType", "dataBase64"])
    && ["image/jpeg", "image/png", "image/webp", "image/avif"].includes(value.mimeType as string)
    && base64Payload(value.dataBase64);
}

export function isXDownloadIntent(value: unknown): value is XDownloadIntent {
  if (!exactRecord(value, ["itemIds", "compatibilityMode"])
    || !Array.isArray(value.itemIds) || value.itemIds.length < 1 || value.itemIds.length > 32
    || typeof value.compatibilityMode !== "boolean") return false;
  const ids = new Set<string>();
  return value.itemIds.every((id) => isAnalysisId(id) && !ids.has(id) && Boolean(ids.add(id)));
}

export function isXItemOutcome(value: unknown): value is XItemOutcome {
  if (!exactRecord(value, ["itemId", "ordinal", "kind", "state"], ["displayName", "errorCode"])
    || !isAnalysisId(value.itemId) || !positiveInteger(value.ordinal, 32)
    || !mediaKinds.has(value.kind) || !outcomeStates.has(value.state)
    || !optional(value, "displayName", (name) => safeDisplayName(name, 160))
    || !optional(value, "errorCode", (code) => downloadErrors.has(code))) return false;
  const display = Object.hasOwn(value, "displayName");
  const error = Object.hasOwn(value, "errorCode");
  return (value.state === "completed" && display && !error)
    || (value.state === "failed" && !display && error)
    || (value.state === "canceled" && !display && !error);
}

function nullableSafeInteger(value: unknown, allowZero: boolean): value is number | null {
  return value === null || (typeof value === "number" && Number.isSafeInteger(value)
    && (allowZero ? value >= 0 : value > 0));
}

export function isXDownloadProgress(value: unknown): value is XDownloadProgress {
  if (!exactRecord(value, [
    "jobId", "state", "itemIndex", "itemCount", "downloadedBytes", "totalBytes", "percent",
  ])
    || !isAnalysisId(value.jobId) || !jobStates.has(value.state)
    || !positiveInteger(value.itemCount, 32) || !positiveInteger(value.itemIndex, value.itemCount)
    || !nullableSafeInteger(value.downloadedBytes, true)
    || !nullableSafeInteger(value.totalBytes, false)
    || (typeof value.downloadedBytes === "number" && typeof value.totalBytes === "number"
      && value.downloadedBytes > value.totalBytes)
    || !(value.percent === null || (typeof value.percent === "number" && Number.isFinite(value.percent)
      && value.percent >= 0 && value.percent <= 100))) return false;
  return value.percent === null
    || (typeof value.downloadedBytes === "number" && typeof value.totalBytes === "number");
}

export function isXDownloadResult(value: unknown): value is XDownloadResult {
  if (!exactRecord(
    value,
    ["jobId", "state", "completedCount", "failedCount", "items", "postItemCount", "unavailableCount", "contentChanged"],
    ["folderDisplayName"],
  )
    || !isAnalysisId(value.jobId)
    || !positiveInteger(value.postItemCount, 32)
    || typeof value.unavailableCount !== "number" || !Number.isInteger(value.unavailableCount) || value.unavailableCount < 0 || value.unavailableCount > value.postItemCount
    || typeof value.contentChanged !== "boolean"
    || !["completed", "partial", "failed", "canceled"].includes(value.state as string)
    || !Array.isArray(value.items) || value.items.length < 1 || value.items.length > 32
    || typeof value.completedCount !== "number" || !Number.isInteger(value.completedCount) || value.completedCount < 0
    || typeof value.failedCount !== "number" || !Number.isInteger(value.failedCount) || value.failedCount < 0
    || !optional(value, "folderDisplayName", (name) => safeDisplayName(name, 160))) return false;

  const ids = new Set<string>();
  let completed = 0;
  let failed = 0;
  let canceled = 0;
  let previousOrdinal = 0;
  for (let index = 0; index < value.items.length; index += 1) {
    const item = value.items[index];
    if (!isXItemOutcome(item) || item.ordinal <= previousOrdinal || ids.has(item.itemId)) return false;
    previousOrdinal = item.ordinal;
    ids.add(item.itemId);
    if (item.state === "completed") completed += 1;
    if (item.state === "failed") failed += 1;
    if (item.state === "canceled") canceled += 1;
  }
  const stateValid = (value.state === "completed" && completed === value.items.length)
    || (value.state === "partial" && completed > 0 && failed > 0 && canceled === 0)
    || (value.state === "failed" && completed === 0 && failed === value.items.length)
    || (value.state === "canceled" && canceled > 0);
  const folderExpected = completed > 0;
  return value.items.length + value.unavailableCount <= value.postItemCount && previousOrdinal <= value.postItemCount
    && stateValid && completed === value.completedCount && failed === value.failedCount
    && folderExpected === Object.hasOwn(value, "folderDisplayName");
}
