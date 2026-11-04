import { useEffect, useRef, useState, type FormEvent } from "react";
import { Icon } from "../../shared/ui/Icon";
import type { LiveAnalysisPreview } from "../../shared/contracts/liveAnalysis";
import type { SelectionMode } from "../../shared/contracts/selection";
import { catalogs, type Catalog } from "../../shared/i18n/catalog";
import type { Locale } from "../../shared/i18n/locale";
import { DownloadIpcError } from "../../shared/ipc/destination";
import { approveDefaultDestination, chooseDefaultDestination } from "../../shared/ipc/destinationSettings";
import { startDownloadInDefaultFolder } from "../downloads/defaultDestinationStart";
import type { VideoResolutionPlan } from "../../shared/contracts/resolutionOptions";
import {
  DownloadCommandError,
  cancelDownload,
  getDownloadResult,
  listenDownloadProgress,
  openCompletedLocation,
} from "../../shared/ipc/downloads";
import type { AnalysisErrorCode } from "../../shared/ipc/liveAnalysis";
import { DownloadControls, type DownloadControlsProps } from "../downloads/DownloadControls";
import {
  applyDownloadProgress,
  applyDownloadResult,
  beginDestinationSelection,
  beginDownloadStart,
  detachSubmittedDownload,
  canStartDownload,
  createDownloadUiState,
  failDownloadStart,
  failDestinationSelection,
  finishDownloadCancelRequest,
  finishDownloadStart,
  finishDestinationSelection,
  invalidateDownloadSelection,
  isDownloadActive,
  recordDownloadActionError,
  requestDownloadCancel,
  retryDownload,
  refreshSavedDownloadDestination,
  synchronizeDownloadAnalysis,
  type DownloadUiState,
} from "../downloads/downloadState";
import { formatClipRange } from "./formatPresentation";
import { SelectionDetails, SelectionSummary } from "./SelectionDetails";
import { resolutionOptions, videoChoiceKey, requestResolutionPlans, type ResolutionPlansResult } from "./resolutionOptions";
import { ClipTimeline } from "./ClipTimeline";
import { fullClipRange } from "./clipRange";
import {
  buildLiveSelectionIntent,
  changeLiveAnalysis,
  createLiveAnalysisState,
  liveClipIsAvailable,
  requestLiveAnalysis,
  requestLiveSelection,
  type LiveAnalysisChange,
  type LiveAnalysisState,
} from "./liveAnalysisState";

import {readClipboardText, ClipboardIpcError, type ClipboardErrorCode} from "../../shared/ipc/clipboard";
import {createPasteState, beginPaste, finishPaste, invalidatePaste} from "./pasteState";

const modes: SelectionMode[] = ["best_available", "manual", "video_only", "audio_only"];

const errorActions: Record<AnalysisErrorCode, keyof Catalog> = {
  invalid_url: "analysisActionEditUrl",
  unsupported_platform: "analysisActionUseYouTube",
  unsupported_media: "analysisActionTryAnother",
  invalid_metadata: "analysisActionRetry",
  format_unavailable: "analysisActionAdjustSelection",
  analysis_unavailable: "analysisActionRetry",
  analysis_expired: "analysisActionRetry",
  tool_missing: "analysisActionCheckTools",
  tool_integrity_failed: "analysisActionCheckTools",
  authentication_required: "analysisActionPublicMedia",
  access_denied: "analysisActionPublicMedia",
  content_unavailable: "analysisActionTryAnother",
  content_changed: "analysisActionRetry",
  rate_limited: "analysisActionRetry",
  extractor_outdated: "analysisActionUpdateTools",
  network_failed: "analysisActionRetry",
  analysis_timeout: "analysisActionRetry",
  process_failed: "analysisActionRetry",
  unknown: "analysisActionRetry",
};

export function LiveAnalysisWorkspace({ locale, onReady, destinationRevision = 0, active = true, onOpenSettings }: {
  locale: Locale;
  onReady?: (preview: LiveAnalysisPreview) => void;
  destinationRevision?: number;
  active?: boolean;
  onOpenSettings?: () => void;
}) {
  const [state, setState] = useState(createLiveAnalysisState);
  const [downloadState, setDownloadState] = useState(createDownloadUiState);
  const [downloadEventsReady, setDownloadEventsReady] = useState(false);
  const [resolutionData, setResolutionData] = useState<ResolutionPlansResult | null>(null);
  const downloadStateRef = useRef(downloadState);
  const terminalResultRequests = useRef(new Set<string>());
  const mounted = useRef(true);
  const generation = useRef(0);
  const resolutionGeneration = useRef(0);
  const appliedDestinationRevision = useRef(destinationRevision);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      generation.current += 1;
      resolutionGeneration.current += 1;
    };
  }, []);

  useEffect(() => {
    const analysisId = state.data?.analysisId;
    if (!analysisId || isDownloadActive(downloadStateRef.current)) return;
    let current = true;
    void approveDefaultDestination().then((destination) => {
      if (current && downloadStateRef.current.analysisId === analysisId && !isDownloadActive(downloadStateRef.current)) {
        updateDownload((previous) => refreshSavedDownloadDestination(previous, destination, destinationRevision !== appliedDestinationRevision.current));
        appliedDestinationRevision.current = destinationRevision;
      }
    }).catch(() => {
      if (current && downloadStateRef.current.analysisId === analysisId && !isDownloadActive(downloadStateRef.current)) {
        updateDownload((previous) => ({ ...previous, destination: null }));
      }
    });
    return () => { current = false; };
  }, [state.data?.analysisId, destinationRevision, downloadState.phase]);

  useEffect(() => {
    if (!state.data || (state.mode !== "manual" && state.mode !== "video_only")) return;
    const data = state.data; const mode = state.mode; const compatibilityMode = state.compatibilityMode;
    void requestResolutionPlans(data,mode,compatibilityMode,resolutionGeneration,setResolutionData);
    return () => { resolutionGeneration.current += 1; };
  }, [state.data?.analysisId, state.mode, state.compatibilityMode]);

  useEffect(() => {
    if (!resolutionData || resolutionData.failed || resolutionData.analysisId !== state.data?.analysisId
      || resolutionData.mode !== state.mode || resolutionData.compatibilityMode !== state.compatibilityMode || isDownloadActive(downloadStateRef.current)) return;
    const choices = resolutionData.plans.filter((row) => row.outcome.status === "selected");
    const chosen = choices.find((row) => row.videoFormatId === state.videoFormatId && row.outputContainer === state.outputContainer)
      ?? choices.find(row=>row.videoFormatId === state.videoFormatId)
      ?? (state.videoFormatId === "" ? choices[0] : undefined);
    if (!chosen || (chosen.videoFormatId === state.videoFormatId && chosen.outputContainer === state.outputContainer)) return;
    // Loading an unchanged pair must not invalidate its in-flight preview.
    generation.current += 1;
    setState((previous) => previous === state
      ? changeLiveAnalysis(previous, {type: "videoChoice", value: {videoFormatId: chosen.videoFormatId, outputContainer: chosen.outputContainer}})
      : previous);
  }, [resolutionData, state]);

  useEffect(() => {
    if (!state.data || state.status !== "idle" || isDownloadActive(downloadStateRef.current)) return;
    if ((state.mode === "manual" || state.mode === "video_only") && (state.videoFormatId === "" || state.outputContainer === "")) return;
    const timer = window.setTimeout(() => { void requestLiveSelection(state, generation, setState); }, 200);
    return () => window.clearTimeout(timer);
  }, [state.data?.analysisId, state.mode, state.compatibilityMode, state.videoFormatId, state.outputContainer, state.clipEnabled, state.clipStart, state.clipEnd, state.status]);

  function updateDownload(update: (previous: DownloadUiState) => DownloadUiState) {
    if (!mounted.current) return;
    const previous = downloadStateRef.current;
    const next = update(previous);
    if (next === previous) return;
    downloadStateRef.current = next;
    setDownloadState(next);
  }

  function publicDownloadError(error: unknown) {
    return error instanceof DownloadCommandError || error instanceof DownloadIpcError
      ? error.code
      : "unknown" as const;
  }

  async function loadTerminalResult(jobId: string, startGeneration: number, terminalEvent: boolean) {
    if (terminalEvent) {
      if (terminalResultRequests.current.has(jobId)) return;
      terminalResultRequests.current.add(jobId);
    }
    try {
      const result = await getDownloadResult(jobId);
      updateDownload((current) => current.startGeneration === startGeneration
        ? applyDownloadResult(current, result)
        : current);
    } catch (error) {
      if (!terminalEvent && error instanceof DownloadCommandError && error.code === "invalid_request") return;
      updateDownload((current) => current.jobId === jobId && current.startGeneration === startGeneration
        ? recordDownloadActionError(current, publicDownloadError(error))
        : current);
    }
  }

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listenDownloadProgress((progress) => {
      const current = downloadStateRef.current;
      const next = applyDownloadProgress(current, progress);
      if (next === current) return;
      downloadStateRef.current = next;
      setDownloadState(next);
      if (progress.state === "completed" || progress.state === "failed" || progress.state === "canceled") {
        void loadTerminalResult(progress.jobId, next.startGeneration, true);
      }
    }).then((cleanup) => {
      if (disposed) cleanup();
      else {
        unlisten = cleanup;
        setDownloadEventsReady(true);
      }
    }).catch((error: unknown) => {
      if (!disposed) {
        setDownloadEventsReady(false);
        updateDownload((current) => recordDownloadActionError(current, publicDownloadError(error)));
      }
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  function onChange(change: LiveAnalysisChange) {
    if (isDownloadActive(downloadStateRef.current)) return;
    if (state.status === "analyzing" || changeLiveAnalysis(state, change) === state) return;
    generation.current += 1;
    if (change.type === "url" || change.type === "mode" || change.type === "compatibilityMode") {
      resolutionGeneration.current += 1;
      setResolutionData(null);
    }
    setState((previous) => changeLiveAnalysis(previous, change));
    updateDownload((previous) => change.type === "url"
      ? synchronizeDownloadAnalysis(previous, null)
      : invalidateDownloadSelection(previous));
  }

  async function analyze() {
    if (isDownloadActive(downloadStateRef.current)) return;
    const ready = await requestLiveAnalysis(state, generation, setState);
    if (ready) {
      updateDownload((previous) => synchronizeDownloadAnalysis(previous, ready.analysisId));
      onReady?.(ready);
    }
  }

  async function preview() {
    if (isDownloadActive(downloadStateRef.current)) return;
    const ready = await requestLiveSelection(state, generation, setState);
    if (ready) {
      updateDownload((previous) => synchronizeDownloadAnalysis(previous, ready.analysisId));
      onReady?.(ready);
    }
  }

  async function chooseDownloadDestination() {
    const analysisId = downloadStateRef.current.analysisId;
    if (!analysisId || isDownloadActive(downloadStateRef.current)) return;
    const choosing = beginDestinationSelection(downloadStateRef.current);
    if (choosing === downloadStateRef.current) return;
    downloadStateRef.current = choosing;
    setDownloadState(choosing);
    try {
      const saved = await chooseDefaultDestination();
      const selected = saved ? await approveDefaultDestination() : null;
      updateDownload((previous) => finishDestinationSelection(previous, analysisId, selected));
    } catch (error) {
      updateDownload((previous) => failDestinationSelection(
        previous,
        analysisId,
        publicDownloadError(error),
      ));
    }
  }

  async function beginDownload() {
    const current = downloadStateRef.current;
    const built = buildLiveSelectionIntent(state);
    if (!state.data
      || state.outcome?.status !== "selected"
      || "error" in built
      || !downloadEventsReady
      || !canStartDownload(current, true)
      || !current.destination) return;

    const begun = beginDownloadStart(current);
    downloadStateRef.current = begun.state;
    setDownloadState(begun.state);
    try {
      const { started, destination } = await startDownloadInDefaultFolder(state.data.analysisId, built.intent);
      updateDownload((previous) => previous.startGeneration === begun.generation
        ? detachSubmittedDownload(finishDownloadStart({ ...previous, destination }, begun.generation, started.jobId), begun.generation) : previous);
    } catch (error) {
      updateDownload((previous) => failDownloadStart(
        previous,
        begun.generation,
        publicDownloadError(error),
      ));
    }
  }

  async function cancelActiveDownload() {
    const jobId = downloadStateRef.current.jobId;
    if (!jobId) return;
    updateDownload((previous) => requestDownloadCancel(previous, jobId));
    try {
      await cancelDownload(jobId);
    } catch (error) {
      updateDownload((previous) => finishDownloadCancelRequest(
        previous,
        jobId,
        publicDownloadError(error),
      ));
    }
  }

  async function openDownloadLocation() {
    const current = downloadStateRef.current;
    if (current.phase !== "completed" || !current.jobId || !current.displayName) return;
    try {
      await openCompletedLocation(current.jobId);
    } catch (error) {
      updateDownload((previous) => recordDownloadActionError(previous, publicDownloadError(error)));
    }
  }

  const built = buildLiveSelectionIntent(state);
  const selectionReady = state.outcome?.status === "selected" && !("error" in built);
  const downloadProps: Omit<DownloadControlsProps, "locale"> = {
    state: downloadState,
    canStart: downloadEventsReady && canStartDownload(downloadState, selectionReady),
    onChooseDestination: () => { if (onOpenSettings) onOpenSettings(); else void chooseDownloadDestination(); },
    onStart: () => { void beginDownload(); },
    onCancel: () => { void cancelActiveDownload(); },
    onRetry: () => { updateDownload(retryDownload); },
    onOpenLocation: () => { void openDownloadLocation(); },
  };

  return <LiveAnalysisWorkspaceView
    state={state}
    locale={locale}
    active={active}
    resolutionPlans={resolutionData && resolutionData.analysisId === state.data?.analysisId && resolutionData.mode === state.mode && resolutionData.compatibilityMode === state.compatibilityMode ? resolutionData.plans : []}
    resolutionLoading={resolutionData?.analysisId !== state.data?.analysisId || resolutionData?.mode !== state.mode || resolutionData?.compatibilityMode !== state.compatibilityMode}
    resolutionFailed={resolutionData?.analysisId === state.data?.analysisId && resolutionData?.mode === state.mode && resolutionData?.compatibilityMode === state.compatibilityMode && resolutionData.failed}
    onChange={onChange}
    onAnalyze={() => { void analyze(); }}
    onPreview={() => { void preview(); }}
    inputLocked={isDownloadActive(downloadState)}
    download={downloadProps}
  />;
}

function statusContent(state: LiveAnalysisState, copy: Catalog): { message: string; action?: string } {
  if (state.error === "clip") return { message: copy.clipInvalidInput };
  if (state.error === "manual") return { message: copy["unavailable.invalid_selection"], action: copy.analysisActionAdjustSelection };
  if (state.error) {
    return {
      message: copy[`analysisError.${state.error}`],
      action: copy[errorActions[state.error]],
    };
  }
  if (state.status === "analyzing") return { message: copy.analysisLoading };
  if (state.status === "previewing") return { message: copy.selectionLoading };
  if (state.outcome?.status === "unavailable") {
    return { message: copy[`unavailable.${state.outcome.code}`] };
  }
  if (state.status === "analyzed") return { message: copy.analysisReady };
  if (state.status === "ready") return { message: copy.selectionReady };
  if (state.data) return { message: copy.selectionIdle };
  return { message: copy.analysisIdle };
}

export function LiveAnalysisWorkspaceView({ state, locale, onChange, onAnalyze, inputLocked = false, download, active = true, resolutionPlans = [], resolutionLoading = false, resolutionFailed = false }: {
  state: LiveAnalysisState;
  locale: Locale;
  onChange: (change: LiveAnalysisChange) => void;
  onAnalyze: () => void;
  onPreview: () => void;
  inputLocked?: boolean;
  download?: Omit<DownloadControlsProps, "locale">;
  active?: boolean;
  resolutionPlans?: readonly VideoResolutionPlan[];
  resolutionLoading?: boolean;
  resolutionFailed?: boolean;
}) {
  const copy = catalogs[locale];
  const choices = state.data && (state.mode === "manual" || state.mode === "video_only") ? resolutionOptions(state.data, state.mode, locale, resolutionPlans, state.clipEnabled, state.videoFormatId, state.outputContainer) : [];
  const selectedChoice = state.outputContainer !== "" ? videoChoiceKey(state.videoFormatId, state.outputContainer) : "";
  const canClip = liveClipIsAvailable(state);
  const built = buildLiveSelectionIntent(state);
  const clip = "intent" in built ? built.intent.clip : undefined;
  const fullRange = fullClipRange(state.data?.media.durationSeconds);
  const status = statusContent(state, copy);
  const analysisBusy = state.status === "analyzing";
  const controlsLocked = inputLocked || analysisBusy;
  const selectionBusy = state.status === "previewing";

  const pasteState = useRef(createPasteState());
  const pasteMounted = useRef(true);
  const pasteAvailable = useRef(false);
  pasteAvailable.current = active && !controlsLocked;
  const [pastePending, setPastePending] = useState(false);
  const [pasteError, setPasteError] = useState<ClipboardErrorCode | null>(null);
  function invalidatePendingPaste() {
    pasteState.current = invalidatePaste(pasteState.current);
    setPastePending(false);
    setPasteError(null);
  }
  useEffect(() => {
    pasteMounted.current = true;
    return () => {
      pasteMounted.current = false;
      pasteState.current = invalidatePaste(pasteState.current);
    };
  }, []);
  useEffect(() => { invalidatePendingPaste(); }, [active, controlsLocked, state.url]);
  async function pasteLink() {
    if (!pasteAvailable.current) return;
    const begun = beginPaste(pasteState.current);
    if (!begun) return;
    pasteState.current = begun.state;
    setPastePending(true);
    setPasteError(null);
    let text: string | undefined;
    let errorCode: ClipboardErrorCode | null = null;
    try { text = await readClipboardText(); }
    catch (error) { errorCode = error instanceof ClipboardIpcError ? error.code : "unavailable"; }
    const finished = finishPaste(pasteState.current, begun.ticket, pasteMounted.current && pasteAvailable.current);
    if (finished.state === pasteState.current) return;
    pasteState.current = finished.state;
    if (!pasteMounted.current) return;
    setPastePending(false);
    if (!finished.apply) return;
    if (text !== undefined) onChange({type: "url", value: text});
    else setPasteError(errorCode ?? "unavailable");
  }
  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    invalidatePendingPaste();
    onAnalyze();
  }

  return (
    <section className="selection-workspace live-analysis-workspace" aria-labelledby="live-analysis-heading">
      <div className="workspace-heading">
        <h2 id="live-analysis-heading" className="page-title">{copy["route.youtube"]}</h2>
      </div>
          <form className="url-form" onSubmit={submit}>
            <div className="field">
              <label htmlFor="media-url">{copy.mediaUrlLabel}</label>
              <div className="url-input-wrap">
              <input
                id="media-url"
                type="url"
                inputMode="url"
                autoComplete="off"
                spellCheck={false}
                maxLength={2048}
                placeholder={copy.mediaUrlPlaceholder}
                value={state.url}
                disabled={analysisBusy || inputLocked}
                aria-invalid={state.error === "invalid_url"}
                aria-describedby="analysis-status"
                onInput={invalidatePendingPaste}
                onChange={(event) => onChange({ type: "url", value: event.target.value })}
              />
              <button type="button" aria-label={copy.pasteLink} className="paste-link-button"
                disabled={!active || controlsLocked || pastePending} aria-busy={pastePending}
                onClick={() => { void pasteLink(); }}><Icon name="clipboard" size={16} />{copy.pasteLink}</button>
              </div>
              {pasteError ? <p className="field-hint error-message" role="status">{copy[`pasteError.${pasteError}`]}</p> : null}
            </div>
            <button className="analyze-button" type="submit" disabled={analysisBusy || inputLocked} aria-busy={analysisBusy}>{copy.analyzeMedia}</button>
          </form>
      <div className="selection-layout">
        <div className="selection-output" aria-busy={analysisBusy}>
          {state.data
            ? <SelectionDetails data={state.data} outcome={state.outcome} mode={state.mode} locale={locale} preview={{ analysisId: state.data.analysisId, active: active && !analysisBusy }} />
            : <div className="empty-preview"><Icon className="empty-mark" name="youtube" size={32} /><p>{copy.analysisEmpty}</p></div>}
        </div>
        <div className="selection-controls" aria-busy={selectionBusy}>
          <div className="options-scroll">
          <h3>{copy.downloadOptionsLabel}</h3>
          {state.data ? (
            <>
              <fieldset className="mode-options">
                <legend>{copy.selectionModesLabel}</legend>
                {modes.map((mode) => (
                  <label key={mode}>
                    <input
                      type="radio"
                      name="live-selection-mode"
                      value={mode}
                      checked={state.mode === mode}
                      disabled={controlsLocked}
                      onChange={() => onChange({ type: "mode", value: mode })}
                    />
                    <span>{copy[`mode.${mode}`]}</span>
                  </label>
                ))}
              </fieldset>
              <div className="compatibility-control">
                <label className="clip-toggle" htmlFor="live-compatibility">
                  <input id="live-compatibility" type="checkbox" checked={state.compatibilityMode} disabled={controlsLocked} aria-describedby="live-compatibility-hint"
                    onChange={event=>onChange({type:"compatibilityMode",value:event.target.checked})}/>
                  <span>{copy.compatibilityLabel}</span>
                </label>
                <p id="live-compatibility-hint" className="field-hint">{copy.compatibilityHint}</p>
              </div>
              {state.mode === "manual" || state.mode === "video_only" ? (
                <div className="manual-controls">
                  <div className="field">
                    <label htmlFor="live-manual-video">{copy.resolutionLabel}</label>
                    <select
                      id="live-manual-video"
                      value={choices.some((row) => row.value === selectedChoice) ? selectedChoice : ""}
                      disabled={controlsLocked || resolutionLoading}
                      onChange={(event) => {
                        const row = choices.find((choice) => choice.value === event.target.value);
                        if (row) onChange({ type: "videoChoice", value: {videoFormatId: row.videoFormatId, outputContainer: row.outputContainer} });
                      }}
                    >
                      <option value="">{resolutionLoading ? copy.resolutionLoading : resolutionFailed ? copy.resolutionFailed : choices.length ? copy.chooseVideo : copy.noVideoFormats}</option>
                      {choices.map((row) => <option key={row.value} value={row.value}>{row.label}</option>)}
                    </select>
                  </div>
                </div>
              ) : null}
              <SelectionSummary data={state.data} outcome={state.outcome} locale={locale} />
              <fieldset className="clip-controls">
                <legend>{copy.clipHeading}</legend>
                <label className="clip-toggle">
                  <input
                    type="checkbox"
                    checked={state.clipEnabled}
                    disabled={!canClip || controlsLocked}
                    aria-describedby={!canClip ? "live-clip-unavailable" : undefined}
                    onChange={(event) => onChange({ type: "clipEnabled", value: event.target.checked })}
                  />
                  <span>{copy.clipEnabledLabel}</span>
                </label>
                {!canClip ? <p id="live-clip-unavailable" className="field-hint">{state.mode === "audio_only" ? copy.clipAudioOnlyUnavailable : copy.clipUnavailable}</p> : null}
                {state.clipEnabled && canClip ? (
                  <div className="clip-inputs">
                    {fullRange ? <ClipTimeline durationMs={fullRange.endMs} range={clip ?? fullRange} disabled={controlsLocked} locale={locale}
                      onChange={(value) => onChange({type: "clipRange", value})} /> : null}
                    <div className="field">
                      <label htmlFor="live-clip-start">{copy.clipStartLabel}</label>
                      <input id="live-clip-start" type="text" inputMode="text" autoComplete="off" spellCheck={false} maxLength={14} disabled={controlsLocked}
                        placeholder="0:00:10" value={state.clipStart} aria-describedby="live-clip-hint"
                        aria-invalid={state.error === "clip"} onChange={(event) => onChange({ type: "clipStart", value: event.target.value })} />
                    </div>
                    <div className="field">
                      <label htmlFor="live-clip-end">{copy.clipEndLabel}</label>
                      <input id="live-clip-end" type="text" inputMode="text" autoComplete="off" spellCheck={false} maxLength={14} disabled={controlsLocked}
                        placeholder="0:00:30" value={state.clipEnd} aria-describedby="live-clip-hint"
                        aria-invalid={state.error === "clip"} onChange={(event) => onChange({ type: "clipEnd", value: event.target.value })} />
                    </div>
                    <p id="live-clip-hint" className="field-hint">{copy.clipTimeHint}</p>
                    {clip ? <p className="clip-summary">{formatClipRange(clip, locale)}</p> : null}
                    <p className="field-hint">{copy.clipCaveat}</p>
                    {clip ? <p className="field-hint">{copy.clipFullMediaNotice}</p> : null}
                  </div>
                ) : null}
              </fieldset>

            </>
          ) : <p className="field-hint">{copy.downloadNeedsSelection}</p>}

          <p id="analysis-status" className={state.error ? "preview-status error-message" : "preview-status"} role="status">
            {status.message}{status.action ? <> <span className="status-action">{status.action}</span></> : null}
          </p>
          </div>
          <div className="options-footer">
            {download ? <DownloadControls locale={locale} {...download} /> : null}
          </div>
        </div>
      </div>
    </section>
  );
}
