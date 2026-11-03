import type { ClipRange } from "../../shared/contracts/selection";

// The final negative lookahead requires the actual end, including after line terminators.
const clipTimePattern = /^([0-9]{1,4}):([0-5][0-9]):([0-5][0-9])(?:\.([0-9]{1,3}))?(?![\s\S])/;

export function parseClipTime(value: string): number | undefined {
  if (value.length > 14) return undefined;
  const match = clipTimePattern.exec(value);
  if (!match) return undefined;
  const milliseconds = (Number(match[1]) * 3600 + Number(match[2]) * 60 + Number(match[3])) * 1000
    + Number((match[4] ?? "").padEnd(3, "0"));
  return milliseconds <= 31_536_000_000 ? milliseconds : undefined;
}

export function formatClipTime(milliseconds: number): string {
  const seconds = Math.floor(milliseconds / 1000);
  const fraction = milliseconds % 1000;
  return `${Math.floor(seconds / 3600)}:${String(Math.floor(seconds / 60) % 60).padStart(2, "0")}:${String(seconds % 60).padStart(2, "0")}${fraction ? `.${String(fraction).padStart(3, "0")}` : ""}`;
}

export function fullClipRange(durationSeconds: number | undefined): ClipRange | undefined {
  if (durationSeconds === undefined || !Number.isFinite(durationSeconds)) return undefined;
  const endMs = Math.floor(durationSeconds * 1000);
  return endMs > 0 && endMs <= 31_536_000_000 ? { startMs: 0, endMs } : undefined;
}

export function moveClipBoundary(durationMs: number, range: ClipRange, boundary: "start" | "end", value: number): ClipRange | undefined {
  if (!Number.isSafeInteger(durationMs) || durationMs <= 0 || durationMs > 31_536_000_000
    || !Number.isSafeInteger(range.startMs) || !Number.isSafeInteger(range.endMs)
    || range.startMs < 0 || range.startMs >= range.endMs || range.endMs > durationMs || !Number.isFinite(value)) return undefined;
  return boundary === "start"
    ? { startMs: Math.max(0, Math.min(Math.round(value), range.endMs - 1)), endMs: range.endMs }
    : { startMs: range.startMs, endMs: Math.min(durationMs, Math.max(Math.round(value), range.startMs + 1)) };
}
