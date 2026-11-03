import { useEffect, useRef, useState } from "react";
import { isPlatform } from "../../shared/contracts/media";
import { supportedVideoContainers, type SelectionMode } from "../../shared/contracts/selection";
import { catalogs } from "../../shared/i18n/catalog";
import type { Locale } from "../../shared/i18n/locale";
import { formatClipRange, formatResolution } from "./formatPresentation";
import { SelectionDetails } from "./SelectionDetails";
import { ClipTimeline } from "./ClipTimeline";
import { videoChoiceKey } from "./resolutionOptions";
import { fullClipRange } from "./clipRange";
import { buildSelectionIntent, changeSelection, clipIsAvailable, createSelectionState, manualOptions, requestSelection, type SelectionState, type SelectionChange } from "./selectionState";

const modes: SelectionMode[] = ["best_available", "manual", "video_only", "audio_only"];

export function SelectionPreview({ locale }: { locale: Locale }) {
  const [state, setState] = useState(createSelectionState);
  const generation = useRef(0);
  useEffect(() => () => { generation.current += 1; }, []);

  function onChange(change: SelectionChange) {
    generation.current += 1;
    setState((previous) => changeSelection(previous, change));
  }
  return <SelectionPreviewView state={state} locale={locale} onChange={onChange}
    onPreview={() => { void requestSelection(state, generation, setState); }} />;
}

export function SelectionPreviewView({ state, locale, onChange, onPreview }: {
  state: SelectionState; locale: Locale; onChange: (change: SelectionChange) => void; onPreview: () => void;
}) {
  const copy = catalogs[locale];
  const choices = manualOptions(state).video.flatMap((format) => (state.compatibilityMode ? ["mp4" as const] : supportedVideoContainers(format.videoCodec)).map((outputContainer) => ({format, outputContainer, value: videoChoiceKey(format.id, outputContainer)})));
  const canClip = clipIsAvailable(state);
  const built = buildSelectionIntent(state);
  const clip = "intent" in built ? built.intent.clip : undefined;
  const fullRange = fullClipRange(state.data?.media.durationSeconds);
  const statusText = state.error === "clip" ? copy.clipInvalidInput
    : state.error === "manual" ? copy["unavailable.invalid_selection"]
      : state.error === "preview" ? copy.previewError
        : state.status === "loading" ? copy.selectionLoading
          : state.outcome?.status === "unavailable"
            ? copy[`unavailable.${state.outcome.code}`]
            : state.outcome ? copy.selectionReady : state.data ? copy.selectionIdle : copy.sampleIdle;

  return (
    <section className="selection-workspace" aria-labelledby="selection-heading">
      <div className="workspace-heading">
        <h2 id="selection-heading">{copy.sampleHeading}</h2>
        <p id="sample-provenance" className="sample-provenance">{copy.sampleNotice}</p>
      </div>
      <div className="selection-layout">
        <div className="selection-controls">
          <div className="field">
            <label htmlFor="sample-platform">{copy.platformLabel}</label>
            <select id="sample-platform" value={state.platform} aria-describedby="sample-provenance"
              onChange={(event) => { if (isPlatform(event.target.value)) onChange({ type: "platform", value: event.target.value }); }}>
              <option value="youtube">YouTube</option>
              <option value="bilibili">Bilibili</option>
              <option value="x">X / Twitter</option>
              <option value="instagram">Instagram</option>
            </select>
          </div>
          <fieldset className="mode-options">
            <legend>{copy.selectionModesLabel}</legend>
            {modes.map((mode) => (
              <label key={mode}>
                <input type="radio" name="selection-mode" value={mode} checked={state.mode === mode}
                  onChange={() => onChange({ type: "mode", value: mode })} />
                <span>{copy[`mode.${mode}`]}</span>
              </label>
            ))}
          </fieldset>
          <div className="compatibility-control">
            <label className="clip-toggle" htmlFor="sample-compatibility"><input id="sample-compatibility" type="checkbox" checked={state.compatibilityMode} disabled={state.status === "loading"} onChange={event=>onChange({type:"compatibilityMode",value:event.target.checked})}/><span>{copy.compatibilityLabel}</span></label>
            <p className="field-hint">{copy.compatibilityHint}</p>
          </div>
          {state.mode === "manual" || state.mode === "video_only" ? (
            <div className="manual-controls">
              <div className="field">
                <label htmlFor="manual-video">{copy.resolutionLabel}</label>
                <select id="manual-video" value={state.outputContainer !== "" ? videoChoiceKey(state.videoFormatId, state.outputContainer) : ""} disabled={!state.data}
                  onChange={(event) => { const choice = choices.find((row) => row.value === event.target.value); if (choice) onChange({type: "videoChoice", value: {videoFormatId: choice.format.id, outputContainer: choice.outputContainer}}); }}>
                  <option value="">{choices.length ? copy.chooseVideo : copy.noVideoFormats}</option>
                  {choices.map((choice) => <option key={choice.value} value={choice.value}>{`${formatResolution(choice.format, locale)} · ${copy.sizeUnknown} · ${choice.outputContainer.toUpperCase()}`}</option>)}
                </select>
              </div>
            </div>
          ) : null}
          <fieldset className="clip-controls">
            <legend>{copy.clipHeading}</legend>
            <label className="clip-toggle">
              <input type="checkbox" checked={state.clipEnabled} disabled={!canClip}
                aria-describedby={!canClip ? "clip-unavailable" : undefined}
                onChange={(event) => onChange({ type: "clipEnabled", value: event.target.checked })} />
              <span>{copy.clipEnabledLabel}</span>
            </label>
            {!canClip ? <p id="clip-unavailable" className="field-hint">{state.mode === "audio_only" ? copy.clipAudioOnlyUnavailable : copy.clipUnavailable}</p> : null}
            {state.clipEnabled && canClip ? (
              <div className="clip-inputs">
                {fullRange ? <ClipTimeline durationMs={fullRange.endMs} range={clip ?? fullRange} locale={locale}
                  onChange={(value) => onChange({type: "clipRange", value})} /> : null}
                <div className="field">
                  <label htmlFor="clip-start">{copy.clipStartLabel}</label>
                  <input id="clip-start" type="text" inputMode="text" autoComplete="off" spellCheck={false} maxLength={14}
                    placeholder="0:00:10" value={state.clipStart} aria-describedby="clip-hint"
                    aria-invalid={state.error === "clip"} onChange={(event) => onChange({ type: "clipStart", value: event.target.value })} />
                </div>
                <div className="field">
                  <label htmlFor="clip-end">{copy.clipEndLabel}</label>
                  <input id="clip-end" type="text" inputMode="text" autoComplete="off" spellCheck={false} maxLength={14}
                    placeholder="0:00:30" value={state.clipEnd} aria-describedby="clip-hint"
                    aria-invalid={state.error === "clip"} onChange={(event) => onChange({ type: "clipEnd", value: event.target.value })} />
                </div>
                <p id="clip-hint" className="field-hint">{copy.clipTimeHint}</p>
                {clip ? <p className="clip-summary">{formatClipRange(clip, locale)}</p> : null}
                <p className="field-hint">{copy.clipCaveat}</p>
              </div>
            ) : null}
          </fieldset>
          <button className="preview-button" type="button" disabled={state.status === "loading"} onClick={onPreview}>{copy.previewSelection}</button>
          <p className={state.error ? "preview-status error-message" : "preview-status"} role="status">{statusText}</p>
        </div>
        <div className="selection-output" aria-busy={state.status === "loading"}>
          {state.data ? <SelectionDetails data={state.data} outcome={state.outcome} mode={state.mode} locale={locale} />
            : <div className="empty-preview"><span className="empty-mark" aria-hidden="true">◫</span><p>{copy.emptyPreview}</p></div>}
        </div>
      </div>
    </section>
  );
}
