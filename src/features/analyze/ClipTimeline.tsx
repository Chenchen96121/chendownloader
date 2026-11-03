import type { CSSProperties, KeyboardEvent } from "react";
import type { ClipRange } from "../../shared/contracts/selection";
import type { Locale } from "../../shared/i18n/locale";
import { catalogs } from "../../shared/i18n/catalog";
import { formatClipTime, moveClipBoundary } from "./clipRange";

export function ClipTimeline({durationMs, range, disabled = false, onChange, locale}: {
  durationMs: number; range: ClipRange; disabled?: boolean;
  onChange: (range: ClipRange) => void; locale: Locale;
}) {
  if (!moveClipBoundary(durationMs, range, "start", range.startMs)) return null;
  const copy = catalogs[locale];
  function update(boundary: "start" | "end", value: number) {
    if (disabled) return;
    const next = moveClipBoundary(durationMs, range, boundary, value);
    if (next) onChange(next);
  }
  function keyboard(event: KeyboardEvent<HTMLInputElement>, boundary: "start" | "end") {
    const direction = event.key === "ArrowRight" || event.key === "ArrowUp" ? 1 : event.key === "ArrowLeft" || event.key === "ArrowDown" ? -1 : 0;
    if (!direction || disabled) return;
    event.preventDefault();
    update(boundary, (boundary === "start" ? range.startMs : range.endMs) + direction * (event.shiftKey ? 100 : 1000));
  }
  return <div className="clip-timeline" style={{"--clip-start": `${range.startMs / durationMs * 100}%`, "--clip-end": `${range.endMs / durationMs * 100}%`} as CSSProperties}>
    <input className="clip-range-start" type="range" min={0} max={durationMs} step={1} value={range.startMs} disabled={disabled}
      aria-label={copy.clipTimelineStart} aria-valuemin={0} aria-valuemax={range.endMs - 1} aria-valuetext={formatClipTime(range.startMs)}
      onChange={(event) => update("start", event.currentTarget.valueAsNumber)} onKeyDown={(event) => keyboard(event, "start")} />
    <input className="clip-range-end" type="range" min={0} max={durationMs} step={1} value={range.endMs} disabled={disabled}
      aria-label={copy.clipTimelineEnd} aria-valuemin={range.startMs + 1} aria-valuemax={durationMs} aria-valuetext={formatClipTime(range.endMs)}
      onChange={(event) => update("end", event.currentTarget.valueAsNumber)} onKeyDown={(event) => keyboard(event, "end")} />
  </div>;
}
