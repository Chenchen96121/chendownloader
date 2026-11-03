import type { FileSize, MediaFormat } from "../../shared/contracts/media";
import type { ClipRange } from "../../shared/contracts/selection";
import { catalogs } from "../../shared/i18n/catalog";
import type { Locale } from "../../shared/i18n/locale";

export function formatDuration(duration: number | undefined, locale: Locale = "zh-CN"): string {
  if (duration === undefined) return catalogs[locale].durationUnknown;
  const seconds = Math.floor(duration);
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const remainder = String(seconds % 60).padStart(2, "0");
  return hours > 0 ? `${hours}:${String(minutes).padStart(2, "0")}:${remainder}` : `${minutes}:${remainder}`;
}

export function formatFileSize(size: FileSize | undefined, locale: Locale = "zh-CN"): string {
  if (size === undefined) return catalogs[locale].sizeUnknown;
  const scale = size.bytes >= 1_000_000_000 ? 3 : size.bytes >= 1_000_000 ? 2 : size.bytes >= 1000 ? 1 : 0;
  const amount = scale === 0 ? String(size.bytes) : (size.bytes / 1000 ** scale).toFixed(2);
  return `${size.isApproximate ? catalogs[locale].sizeApproximate + " " : ""}${amount} ${["B", "KB", "MB", "GB"][scale]}`;
}

export function formatResolution(format: MediaFormat, locale: Locale = "zh-CN"): string {
  if (format.kind === "audio_only") return catalogs[locale]["stream.audio_only"];
  return format.width === undefined || format.height === undefined
    ? catalogs[locale].resolutionUnknown : `${format.width} × ${format.height}`;
}

// Presentation aliases only: ranking and compatibility decisions belong to Rust.
export function formatCodec(token: string | undefined, locale: Locale): string {
  const normalized = token?.toLowerCase() ?? "";
  const videoAliases: [string, string[]][] = [
    ["AV1", ["av1", "av01"]], ["VP9", ["vp9", "vp09"]],
    ["HEVC", ["hevc", "h265", "hev1", "hvc1"]], ["H.264", ["h264", "avc1", "avc3"]],
  ];
  for (const [label, aliases] of videoAliases) {
    if (aliases.some((alias) => normalized === alias || normalized.startsWith(alias + "."))) return label;
  }
  const audioAliases: Record<string, string> = {
    aac: "AAC", "mp4a.40.2": "AAC", "mp4a.40.5": "AAC", "mp4a.40.29": "AAC",
    alac: "ALAC", opus: "Opus", vorbis: "Vorbis", mp3: "MP3", flac: "FLAC",
  };
  return Object.hasOwn(audioAliases, normalized) ? audioAliases[normalized]! : catalogs[locale].codecUnknown;
}

export function formatFormatLabel(format: MediaFormat, locale: Locale): string {
  const parts: string[] = [];
  if (format.kind !== "audio_only") parts.push(formatResolution(format, locale));
  parts.push(formatFileSize(format.fileSize, locale), format.container.toUpperCase());
  return parts.join(" · ");
}

export function formatFps(value: number | undefined, locale: Locale): string {
  return value === undefined || !Number.isFinite(value) || value <= 0 ? catalogs[locale].codecUnknown : `${value.toLocaleString(locale, {maximumFractionDigits:3})} FPS`;
}
export function formatBitrate(value: number | undefined, locale: Locale): string {
  return value === undefined || !Number.isFinite(value) || value <= 0 ? catalogs[locale].codecUnknown : `${value.toLocaleString(locale, {maximumFractionDigits:2})} kbps`;
}

export function formatClipRange(clip: ClipRange, locale: Locale): string {
  const time = (ms: number) => {
    const whole = formatDuration(ms / 1000, locale).padStart(5, "0");
    const fraction = ms % 1000;
    return whole + (fraction ? "." + String(fraction).padStart(3, "0") : "");
  };
  return `${time(clip.startMs)}–${time(clip.endMs)} · ${(clip.endMs - clip.startMs) / 1000} ${catalogs[locale].secondsUnit}`;
}
