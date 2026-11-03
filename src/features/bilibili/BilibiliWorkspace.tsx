import { useEffect, useRef, useState, type FormEvent } from "react";
import { Icon } from "../../shared/ui/Icon";
import type { SelectionMode } from "../../shared/contracts/selection";
import type { VideoResolutionPlan } from "../../shared/contracts/resolutionOptions";
import { catalogs } from "../../shared/i18n/catalog";
import type { Locale } from "../../shared/i18n/locale";
import { bilibiliIpc, BilibiliDownloadIpcError } from "../../shared/ipc/bilibili";
import { approveDefaultDestination } from "../../shared/ipc/destinationSettings";
import { DownloadIpcError } from "../../shared/ipc/destination";
import { ClipboardIpcError, readClipboardText, type ClipboardErrorCode } from "../../shared/ipc/clipboard";
import { ClipTimeline } from "../analyze/ClipTimeline";
import { fullClipRange } from "../analyze/clipRange";
import { formatClipRange } from "../analyze/formatPresentation";
import { SelectionSummary } from "../analyze/SelectionDetails";
import { resolutionOptions, videoChoiceKey, type ResolutionPlansResult } from "../analyze/resolutionOptions";
import { beginPaste, createPasteState, finishPaste, invalidatePaste } from "../analyze/pasteState";
import { DownloadControls, type DownloadControlsProps } from "../downloads/DownloadControls";
import {
  applyDownloadProgress, applyDownloadResult, beginDownloadStart, canStartDownload, createDownloadUiState,
  detachSubmittedDownload, failDownloadStart, finishDownloadStart, invalidateDownloadSelection, isDownloadActive,
  recordDownloadActionError, refreshSavedDownloadDestination, retryDownload, synchronizeDownloadAnalysis,
  type DownloadUiState,
} from "../downloads/downloadState";
import {
  bilibiliClipAvailable, buildBilibiliSelectionIntent, changeBilibiliState, createBilibiliState,
  requestBilibiliAnalysis, requestBilibiliCover, requestBilibiliResolutions, requestBilibiliSelection,
  type BilibiliCoverState, type BilibiliState, type BilibiliUiChange,
} from "./bilibiliState";
import { BilibiliPreview } from "./BilibiliPreview";

const modes: readonly SelectionMode[] = ["best_available", "manual", "video_only", "audio_only"];
const downloadError = (error: unknown) => error instanceof BilibiliDownloadIpcError || error instanceof DownloadIpcError ? error.code : "unknown" as const;

export function BilibiliWorkspace({ locale, active = true, destinationRevision = 0, onOpenSettings }: {
  locale: Locale; active?: boolean; destinationRevision?: number; onOpenSettings?: () => void;
}) {
  const [state, setState] = useState(createBilibiliState);
  const [download, setDownload] = useState(createDownloadUiState);
  const [eventsReady, setEventsReady] = useState(false);
  const [resolutions, setResolutions] = useState<ResolutionPlansResult | null>(null);
  const [cover, setCover] = useState<BilibiliCoverState | null>(null);
  const [coverRevision, setCoverRevision] = useState(0);
  const [openFailed, setOpenFailed] = useState(false);
  const stateRef = useRef(state);
  const downloadRef = useRef(download);
  const mounted = useRef(true);
  const generation = useRef(0);
  const resolutionGeneration = useRef(0);
  const coverGeneration = useRef(0);
  const resolutionCache = useRef(new Map<string, ResolutionPlansResult>());
  const appliedDestinationRevision = useRef(destinationRevision);
  const analysisId = state.data?.analysisId;

  function updateState(update: (previous: BilibiliState) => BilibiliState) {
    if (!mounted.current) return;
    const next = update(stateRef.current);
    if (next !== stateRef.current) { stateRef.current = next; setState(next); }
  }
  function updateDownload(update: (previous: DownloadUiState) => DownloadUiState) {
    if (!mounted.current) return;
    const next = update(downloadRef.current);
    if (next !== downloadRef.current) { downloadRef.current = next; setDownload(next); }
  }
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; generation.current++; resolutionGeneration.current++; coverGeneration.current++; };
  }, []);

  useEffect(() => {
    let disposed = false; let unlisten: (() => void) | undefined;
    void bilibiliIpc.listenProgress(progress => {
      if (disposed) return;
      updateDownload(previous => applyDownloadProgress(previous, progress));
      if (downloadRef.current.jobId === progress.jobId && ["completed", "failed", "canceled"].includes(progress.state)) {
        void bilibiliIpc.getDownloadResult(progress.jobId).then(result => updateDownload(previous => applyDownloadResult(previous, result))).catch(() => {});
      }
    }).then(cleanup => {
      if (disposed) cleanup();
      else { unlisten = cleanup; setEventsReady(true); }
    }).catch(error => { if (!disposed) updateDownload(previous => recordDownloadActionError(previous, downloadError(error))); });
    return () => { disposed = true; unlisten?.(); };
  }, []);

  useEffect(() => {
    if (!analysisId || isDownloadActive(downloadRef.current)) return;
    let current = true;
    void approveDefaultDestination().then(destination => {
      if (current && downloadRef.current.analysisId === analysisId && !isDownloadActive(downloadRef.current)) {
        updateDownload(previous => refreshSavedDownloadDestination(previous, destination, destinationRevision !== appliedDestinationRevision.current));
        appliedDestinationRevision.current = destinationRevision;
      }
    }).catch(() => {
      if (current && downloadRef.current.analysisId === analysisId && !isDownloadActive(downloadRef.current)) updateDownload(previous => ({ ...previous, destination: null }));
    });
    return () => { current = false; };
  }, [analysisId, destinationRevision, download.phase]);

  useEffect(() => {
    setOpenFailed(false);
    setCover(null);
    if (analysisId) void requestBilibiliCover(analysisId, coverGeneration, next => {
      if (mounted.current && stateRef.current.data?.analysisId === analysisId) setCover(next);
    });
    return () => { coverGeneration.current++; };
  }, [analysisId, coverRevision]);

  useEffect(() => {
    const current = stateRef.current;
    if (!current.data || (current.mode !== "manual" && current.mode !== "video_only")) return;
    const { data, mode, compatibilityMode } = current;
    const key = `${data.analysisId}:${mode}:${compatibilityMode}`;
    const cached = resolutionCache.current.get(key);
    if (cached) setResolutions(cached);
    else {
      setResolutions(null);
      void requestBilibiliResolutions(data, mode, compatibilityMode, resolutionGeneration, result => {
        const latest = stateRef.current;
        if (!mounted.current || latest.data?.analysisId !== data.analysisId || latest.mode !== mode || latest.compatibilityMode !== compatibilityMode) return;
        if (!result.failed) resolutionCache.current.set(key, result); // At most four rows of mode/compatibility results per snapshot.
        setResolutions(result);
      });
    }
    return () => { resolutionGeneration.current++; };
  }, [analysisId, state.mode, state.compatibilityMode]);

  useEffect(() => {
    const current = stateRef.current;
    if (!resolutions || resolutions.failed || resolutions.analysisId !== current.data?.analysisId
      || resolutions.mode !== current.mode || resolutions.compatibilityMode !== current.compatibilityMode || isDownloadActive(downloadRef.current)) return;
    const choices = resolutions.plans.filter(row => row.outcome.status === "selected");
    const selected = choices.find(row => row.videoFormatId === current.videoFormatId && row.outputContainer === current.outputContainer)
      ?? choices.find(row => row.videoFormatId === current.videoFormatId) ?? choices[0];
    if (selected && (selected.videoFormatId !== current.videoFormatId || selected.outputContainer !== current.outputContainer)) {
      generation.current++;
      updateState(previous => changeBilibiliState(previous, { type: "videoChoice", value: { videoFormatId: selected.videoFormatId, outputContainer: selected.outputContainer } }));
    }
  }, [resolutions, state.videoFormatId, state.outputContainer, state.mode, state.compatibilityMode]);

  useEffect(() => {
    if (!analysisId || state.status !== "idle" || isDownloadActive(downloadRef.current)) return;
    if ((state.mode === "manual" || state.mode === "video_only") && (!state.videoFormatId || !state.outputContainer)) return;
    const timer = window.setTimeout(() => { void requestBilibiliSelection(stateRef.current, generation, updateState); }, 200);
    return () => window.clearTimeout(timer);
  }, [analysisId, state.status, state.mode, state.compatibilityMode, state.videoFormatId, state.outputContainer, state.clipEnabled, state.clipStart, state.clipEnd]);

  function change(change: BilibiliUiChange) {
    if (isDownloadActive(downloadRef.current)) return;
    const next = changeBilibiliState(stateRef.current, change);
    if (next === stateRef.current) return;
    generation.current++;
    if (change.type === "url") { resolutionGeneration.current++; coverGeneration.current++; resolutionCache.current.clear(); setCover(null); setResolutions(null); }
    updateState(() => next);
    updateDownload(previous => change.type === "url" ? synchronizeDownloadAnalysis(previous, null) : invalidateDownloadSelection(previous));
  }
  async function analyze() {
    if (isDownloadActive(downloadRef.current)) return;
    resolutionGeneration.current++; coverGeneration.current++; resolutionCache.current.clear();
    updateDownload(previous => synchronizeDownloadAnalysis(previous, null));
    const data = await requestBilibiliAnalysis(stateRef.current, generation, updateState);
    if (data && stateRef.current.data?.analysisId === data.analysisId) updateDownload(previous => synchronizeDownloadAnalysis(previous, data.analysisId));
  }
  async function start() {
    const current = stateRef.current; const intent = buildBilibiliSelectionIntent(current);
    if (!current.data || current.outcome?.status !== "selected" || !intent || !eventsReady || !canStartDownload(downloadRef.current, true)) return;
    const id = current.data.analysisId; const begun = beginDownloadStart(downloadRef.current);
    updateDownload(() => begun.state);
    try {
      // Re-approve the shared saved folder immediately before submission, never accept a frontend path.
      const destination = await approveDefaultDestination();
      const started = await bilibiliIpc.startDownload(id, destination.destinationId, intent);
      updateDownload(previous => previous.startGeneration === begun.generation
        ? detachSubmittedDownload(finishDownloadStart({ ...previous, destination }, begun.generation, started.jobId), begun.generation) : previous);
    } catch (error) { updateDownload(previous => failDownloadStart(previous, begun.generation, downloadError(error))); }
  }
  async function openSource() {
    const id = stateRef.current.data?.analysisId;
    if (!id || !active) return;
    setOpenFailed(false);
    try { await bilibiliIpc.openSource(id); }
    catch { if (mounted.current && stateRef.current.data?.analysisId === id) setOpenFailed(true); }
  }
  const matchingResolutions = resolutions?.analysisId === analysisId && resolutions?.mode === state.mode && resolutions.compatibilityMode === state.compatibilityMode ? resolutions : null;
  const intent = buildBilibiliSelectionIntent(state);
  return <BilibiliWorkspaceView state={state} locale={locale} active={active} inputLocked={isDownloadActive(download)}
    onChange={change} onAnalyze={() => { void analyze(); }}
    cover={cover} openFailed={openFailed} onOpenSource={() => { void openSource(); }}
    onRetryCover={() => setCoverRevision(value => value + 1)}
    onImageError={() => { if (analysisId) setCover({ analysisId, status: "error", payload: null }); }}
    resolutionPlans={matchingResolutions?.plans ?? []} resolutionLoading={!matchingResolutions} resolutionFailed={matchingResolutions?.failed ?? false}
    download={{ state: download, canStart: eventsReady && canStartDownload(download, !!intent && state.outcome?.status === "selected"),
      onChooseDestination: () => onOpenSettings?.(), onStart: () => { void start(); },
      onCancel: () => { const id = downloadRef.current.jobId; if (id) void bilibiliIpc.cancelDownload(id).catch(error => updateDownload(previous => recordDownloadActionError(previous, downloadError(error)))); },
      onRetry: () => updateDownload(retryDownload),
      onOpenLocation: () => { const id = downloadRef.current.jobId; if (id) void bilibiliIpc.openCompletedLocation(id).catch(error => updateDownload(previous => recordDownloadActionError(previous, downloadError(error)))); },
    }} />;
}

export function BilibiliWorkspaceView({ state, locale, active = true, inputLocked = false, onChange, onAnalyze,
  cover = null, openFailed = false, onOpenSource, onRetryCover, onImageError = () => {},
  resolutionPlans = [], resolutionLoading = false, resolutionFailed = false, download,
}: {
  state: BilibiliState; locale: Locale; active?: boolean; inputLocked?: boolean;
  onChange: (change: BilibiliUiChange) => void; onAnalyze: () => void;
  cover?: BilibiliCoverState | null; openFailed?: boolean; onOpenSource: () => void; onRetryCover: () => void; onImageError?: () => void;
  resolutionPlans?: readonly VideoResolutionPlan[]; resolutionLoading?: boolean; resolutionFailed?: boolean;
  download?: Omit<DownloadControlsProps, "locale" | "namespace">;
}) {
  const copy = catalogs[locale];
  const busy = state.status === "analyzing"; const locked = inputLocked || busy;
  const choices = state.data && (state.mode === "manual" || state.mode === "video_only")
    ? resolutionOptions(state.data, state.mode, locale, resolutionPlans, state.clipEnabled, state.videoFormatId, state.outputContainer) : [];
  const choice = state.outputContainer ? videoChoiceKey(state.videoFormatId, state.outputContainer) : "";
  const intent = buildBilibiliSelectionIntent(state); const clip = intent?.clip;
  const full = fullClipRange(state.data?.media.durationSeconds); const canClip = bilibiliClipAvailable(state);
  const paste = useRef(createPasteState()); const pasteMounted = useRef(true); const pasteAvailable = useRef(false);
  const [pastePending, setPastePending] = useState(false); const [pasteError, setPasteError] = useState<ClipboardErrorCode | null>(null);
  pasteAvailable.current = active && !inputLocked;
  function invalidatePendingPaste() { paste.current = invalidatePaste(paste.current); setPastePending(false); setPasteError(null); }
  useEffect(() => { pasteMounted.current = true; return () => { pasteMounted.current = false; paste.current = invalidatePaste(paste.current); }; }, []);
  useEffect(() => { invalidatePendingPaste(); }, [active, inputLocked, state.url]);
  async function pasteLink() {
    if (!pasteAvailable.current) return;
    const begun = beginPaste(paste.current); if (!begun) return;
    paste.current = begun.state; setPastePending(true); setPasteError(null);
    let text: string | undefined; let errorCode: ClipboardErrorCode = "unavailable";
    try { text = await readClipboardText(); } catch (error) { if (error instanceof ClipboardIpcError) errorCode = error.code; }
    const finished = finishPaste(paste.current, begun.ticket, pasteMounted.current && pasteAvailable.current);
    if (finished.state === paste.current) return;
    paste.current = finished.state;
    if (!pasteMounted.current) return;
    setPastePending(false);
    if (finished.apply) { if (text !== undefined) onChange({ type: "url", value: text }); else setPasteError(errorCode); }
  }
  function submit(event: FormEvent<HTMLFormElement>) { event.preventDefault(); invalidatePendingPaste(); onAnalyze(); }
  const status = state.error === "clip" ? copy.clipInvalidInput : state.error === "manual" ? copy["unavailable.invalid_selection"]
    : state.error === "unsupported_platform" ? copy.bilibiliScopeHint : state.error ? copy[`analysisError.${state.error}`]
      : busy ? copy.analysisLoading : state.status === "previewing" ? copy.selectionLoading
        : state.outcome?.status === "unavailable" ? copy[`unavailable.${state.outcome.code}`]
          : state.status === "analyzed" ? copy.analysisReady : state.status === "ready" ? copy.selectionReady : state.data ? copy.selectionIdle : copy.analysisIdle;
  return <section className="selection-workspace live-analysis-workspace bilibili-workspace" aria-labelledby="bilibili-heading">
    <div className="workspace-heading"><h2 id="bilibili-heading" className="page-title">{copy["route.bilibili"]}</h2></div>
    <form className="url-form" onSubmit={submit}>
      <div className="field"><label htmlFor="bilibili-media-url">{copy.mediaUrlLabel}</label><div className="url-input-wrap">
        <input id="bilibili-media-url" type="text" inputMode="url" autoComplete="off" spellCheck={false} maxLength={2048}
          placeholder={copy.bilibiliUrlPlaceholder} value={state.url} disabled={inputLocked} aria-invalid={state.error === "invalid_url"}
          aria-describedby="bilibili-scope bilibili-status" onInput={invalidatePendingPaste} onChange={event => onChange({ type: "url", value: event.target.value })} />
        <button type="button" aria-label={copy.pasteLink} className="paste-link-button" disabled={!active || inputLocked || pastePending}
          aria-busy={pastePending} onClick={() => { void pasteLink(); }}><Icon name="clipboard" size={16} />{copy.pasteLink}</button>
      </div>{pasteError ? <p className="field-hint error-message" role="status">{copy[`pasteError.${pasteError}`]}</p> : null}</div>
      <button className="analyze-button" type="submit" disabled={busy || inputLocked} aria-busy={busy}>{copy.analyzeMedia}</button>
    </form>
    <p id="bilibili-scope" className="field-hint bilibili-scope">{copy.bilibiliScopeHint}</p>
    <div className="selection-layout">
      <div className="selection-output" aria-busy={busy}>{state.data
        ? <BilibiliPreview data={state.data} locale={locale} cover={cover} active={active && !busy} openFailed={openFailed}
          onOpenSource={onOpenSource} onRetryCover={onRetryCover} onImageError={onImageError} />
        : <div className="empty-preview"><Icon className="empty-mark" name="bilibili" size={32} /><p>{copy.analysisEmpty}</p></div>}</div>
      <div className="selection-controls" aria-busy={state.status === "previewing"}>
        <div className="options-scroll">
        <h3>{copy.downloadOptionsLabel}</h3>
        {state.data ? <><fieldset className="mode-options"><legend>{copy.selectionModesLabel}</legend>
          {modes.map(mode => <label key={mode}><input type="radio" name="bilibili-selection-mode" value={mode} checked={state.mode === mode} disabled={locked}
            onChange={() => onChange({ type: "mode", value: mode })} /><span>{copy[`mode.${mode}`]}</span></label>)}
        </fieldset>
        <div className="compatibility-control"><label className="clip-toggle" htmlFor="bilibili-compatibility">
          <input id="bilibili-compatibility" type="checkbox" checked={state.compatibilityMode} disabled={locked} aria-describedby="bilibili-compatibility-hint"
            onChange={event => onChange({ type: "compatibilityMode", value: event.target.checked })} /><span>{copy.compatibilityLabel}</span></label>
          <p id="bilibili-compatibility-hint" className="field-hint">{copy.compatibilityHint}</p></div>
        {state.mode === "manual" || state.mode === "video_only" ? <div className="manual-controls"><div className="field"><label htmlFor="bilibili-manual-video">{copy.resolutionLabel}</label>
          <select id="bilibili-manual-video" value={choices.some(row => row.value === choice) ? choice : ""} disabled={locked || resolutionLoading}
            onChange={event => { const row = choices.find(option => option.value === event.target.value); if (row) onChange({ type: "videoChoice", value: { videoFormatId: row.videoFormatId, outputContainer: row.outputContainer } }); }}>
            <option value="">{resolutionLoading ? copy.resolutionLoading : resolutionFailed ? copy.resolutionFailed : choices.length ? copy.chooseVideo : copy.noVideoFormats}</option>
            {choices.map(row => <option key={row.value} value={row.value}>{row.label}</option>)}
          </select></div></div> : null}
        <SelectionSummary data={state.data} outcome={state.outcome} locale={locale} />
        <fieldset className="clip-controls"><legend>{copy.clipHeading}</legend><label className="clip-toggle">
          <input type="checkbox" checked={state.clipEnabled} disabled={!canClip || locked} aria-describedby={!canClip ? "bilibili-clip-unavailable" : undefined}
            onChange={event => onChange({ type: "clipEnabled", value: event.target.checked })} /><span>{copy.clipEnabledLabel}</span></label>
          {!canClip ? <p id="bilibili-clip-unavailable" className="field-hint">{state.mode === "audio_only" ? copy.clipAudioOnlyUnavailable : copy.clipUnavailable}</p> : null}
          {state.clipEnabled && canClip ? <div className="clip-inputs">
            {full ? <ClipTimeline durationMs={full.endMs} range={clip ?? full} disabled={locked} locale={locale} onChange={value => onChange({ type: "clipRange", value })} /> : null}
            <div className="field"><label htmlFor="bilibili-clip-start">{copy.clipStartLabel}</label><input id="bilibili-clip-start" type="text" inputMode="text" autoComplete="off" spellCheck={false} maxLength={14}
              value={state.clipStart} disabled={locked} aria-describedby="bilibili-clip-hint" aria-invalid={state.error === "clip"} onChange={event => onChange({ type: "clipStart", value: event.target.value })} /></div>
            <div className="field"><label htmlFor="bilibili-clip-end">{copy.clipEndLabel}</label><input id="bilibili-clip-end" type="text" inputMode="text" autoComplete="off" spellCheck={false} maxLength={14}
              value={state.clipEnd} disabled={locked} aria-describedby="bilibili-clip-hint" aria-invalid={state.error === "clip"} onChange={event => onChange({ type: "clipEnd", value: event.target.value })} /></div>
            <p id="bilibili-clip-hint" className="field-hint">{copy.clipTimeHint}</p>{clip ? <p className="clip-summary">{formatClipRange(clip, locale)}</p> : null}
            <p className="field-hint">{copy.clipCaveat}</p>{clip ? <p className="field-hint">{copy.clipFullMediaNotice}</p> : null}
          </div> : null}
        </fieldset></> : <p className="field-hint">{copy.downloadNeedsSelection}</p>}
        <p id="bilibili-status" role="status" className={state.error ? "preview-status error-message" : "preview-status"}>{status}</p>
        </div>
        <div className="options-footer">
          {download ? <DownloadControls {...download} locale={locale} namespace="bilibili" /> : null}
        </div>
      </div>
    </div>
  </section>;
}
