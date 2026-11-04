import type { DestinationApproval } from "../../shared/contracts/download";
import { Icon } from "../../shared/ui/Icon";
import { catalogs } from "../../shared/i18n/catalog";
import type { Locale } from "../../shared/i18n/locale";
import { tiktokMessage, tiktokResultSummary } from "./tiktokResultSummary";
import {
  tiktokCompatibilityApplies,
  isTikTokWorkspaceLocked,
  type TikTokWorkspaceState,
} from "./tiktokState";

function interpolate(template: string, values: Record<string, string | number>): string {
  return template.replace(/\{(\w+)\}/gu, (_, key: string) => String(values[key] ?? ""));
}

function compactBytes(bytes: number | undefined, unknown: string): string {
  if (bytes === undefined) return unknown;
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
  if (bytes >= 1024 ** 2) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
}

function progressMessage(state: TikTokWorkspaceState, locale: Locale): string | null {
  const copy = catalogs[locale];
  if (state.job === null) return null;
  if (state.job.cancelRequested) return copy.downloadCanceling;
  const phases = {
    starting: copy.downloadStarting,
    queued: copy.downloadPreparing,
    downloading: copy.downloadDownloading,
    processing: copy.downloadProcessing,
    verifying: copy.downloadVerifying,
    publishing: copy.downloadFinalizing,
    completed: copy.downloadCompleted,
    partial: copy.downloadCompleted,
    failed: copy.downloadFailed,
    canceled: copy.downloadCanceled,
  } as const;
  const itemProgress = state.job.itemIndex > 0 && state.job.itemCount > 1
    ? interpolate(copy.tiktokItemProgress, {
        current: state.job.itemIndex,
        total: state.job.itemCount,
      })
    : null;
  return itemProgress ? `${phases[state.job.state]} ${itemProgress}` : phases[state.job.state];
}

export function TikTokDownloadPanel({
  locale,
  state,
  destination,
  eventsReady,
  onSelectAll,
  onClearSelection,
  onCompatibilityChange,
  onOpenSettings,
  onStart,
  onCancel,
  onOpenLocation,
}: {
  locale: Locale;
  state: TikTokWorkspaceState;
  destination: DestinationApproval | null;
  eventsReady: boolean;
  onSelectAll: () => void;
  onClearSelection: () => void;
  onCompatibilityChange: (enabled: boolean) => void;
  onOpenSettings: () => void;
  onStart: () => void;
  onCancel: () => void;
  onOpenLocation: () => void;
}) {
  const copy = catalogs[locale];
  const locked = isTikTokWorkspaceLocked(state);
  const selected = new Set(state.selectedItemIds);
  const selectedItems = state.analysis?.items.filter((item) => item.availability === "ready" && selected.has(item.itemId)) ?? [];
  const postCount = state.analysis?.items.length ?? 0;
  const readyCount = state.analysis?.items.filter(item => item.availability === "ready").length ?? 0;
  const unavailableCount = postCount - readyCount;
  const includesVideo = selectedItems.some((item) => item.kind !== "image");
  const selectedCount = selectedItems.length;
  const canStart = state.analysis !== null
    && selectedCount > 0
    && destination !== null
    && eventsReady
    && !locked;
  const status = progressMessage(state, locale);
  const error = state.error === null || state.error.scope === "preview"
    ? null
    : state.error.scope === "analysis" || state.error.scope === "source"
      ? copy[`analysisError.${state.error.code}`]
      : copy[`downloadError.${state.error.code}`];

  return <aside className="x-download-panel" aria-labelledby="tiktok-download-heading">
    <div className="options-scroll">
    <h3 id="tiktok-download-heading">{copy.downloadOptionsLabel}</h3>

    <section className="x-panel-section" aria-labelledby="tiktok-selection-heading">
      <div className="x-panel-heading">
        <h4 id="tiktok-selection-heading">{copy.tiktokSelectionRange}</h4>
        <span>{interpolate(copy.tiktokSelectedCount, { count: selectedCount })}</span>
      </div>
      <div className="x-selection-actions">
        <button type="button" disabled={locked || state.analysis === null} onClick={onSelectAll}>
          {copy.tiktokSelectAll}
        </button>
        <button type="button" disabled={locked || selectedCount === 0} onClick={onClearSelection}>
          {copy.tiktokClearSelection}
        </button>
      </div>
      {selectedCount === 0 ? <p className="field-hint">{copy.tiktokNoSelection}</p> : null}
      {unavailableCount > 0 ? <p className="field-hint">{tiktokMessage(copy.tiktokUnavailableCount, { count: unavailableCount })}</p> : null}
    </section>

    {state.analysis?.items.some(item=>item.kind==="video")?<section className="x-panel-section x-compatibility">
      <label htmlFor="tiktok-compatibility">
        <input
          id="tiktok-compatibility"
          type="checkbox"
          checked={state.compatibilityMode}
          disabled={locked || !includesVideo}
          onChange={(event) => onCompatibilityChange(event.target.checked)}
        />
        <span>{copy.compatibilityLabel}</span>
      </label>
      <p className="field-hint">{includesVideo ? copy.compatibilityHint : copy.tiktokCompatibilityVideoHint}</p>
      {tiktokCompatibilityApplies(state) ? <p className="x-compatibility-active">{selectedItems.some(item => item.audioCodec !== undefined) ? "H.264 + AAC · MP4" : "H.264 · MP4"}</p> : null}
    </section>:null}

    <section className="x-panel-section" aria-labelledby="tiktok-summary-heading">
      <h4 id="tiktok-summary-heading">{copy.tiktokDownloadSummary}</h4>
      {selectedItems.length > 0 ? <ol className="x-selected-summary">
        {selectedItems.map((item) => <li key={item.itemId}>
          <span>{interpolate(copy.tiktokItemLabel, { ordinal: item.ordinal })}</span>
          <div>
            <strong>{item.width !== undefined && item.height !== undefined
              ? `${item.width} × ${item.height} · `
              : ""}{item.kind==="video"?(item.fps===undefined?"FPS —":`${item.fps} FPS`):""}</strong>
            <details className="media-extra-details">
              <summary>{copy.sourceDetailsLabel}<Icon name="chevron-down" size={14} /></summary>
              <small>{[
                  `${copy.formatLabel}: ${item.container.toUpperCase()}`,
                  `${copy.sizeLabel}: ${compactBytes(item.fileSize?.bytes, copy.sizeUnknown)}`,
                  item.kind==="image"?null:`${copy.videoCodecLabel}: ${item.videoCodec?.toUpperCase() ?? copy.codecUnknown}`,
                  item.kind==="image"?null:`${copy.audioCodecLabel}: ${item.audioCodec?.toUpperCase() ?? copy.noAudio}`,
                  item.dynamicRange ?? null,
                  item.videoBitrateKbps === undefined ? null : `${copy.videoBitrateLabel}: ${Math.round(item.videoBitrateKbps)} kbps`,
                  item.audioBitrateKbps === undefined ? null : `${copy.audioBitrateLabel}: ${Math.round(item.audioBitrateKbps)} kbps`,
                ].filter((value): value is string => value !== null).join(" · ")}</small>
            </details>
          </div>
        </li>)}
      </ol> : <p className="field-hint">{copy.tiktokNoSelection}</p>}
    </section>

    {state.result ? <section className={`x-result x-result--${state.result.state}`} aria-live="polite">
      <strong>{tiktokResultSummary(state.result, copy)}</strong>
      <p>{tiktokMessage(copy.tiktokOriginalCounts, { total: state.result.postItemCount, unavailable: state.result.unavailableCount })}</p>
      {state.result.contentChanged ? <p role="status">{copy.tiktokContentChangedHint}</p> : null}
      {state.result.items.some((item) => item.state === "failed") ? <ul>
        {state.result.items.filter((item) => item.state === "failed").map((item) => <li key={item.itemId}>
          <span>{interpolate(copy.tiktokFailedItem, { ordinal: item.ordinal })}</span>
          <span>{item.errorCode ? copy[`downloadError.${item.errorCode}`] : copy.downloadFailed}</span>
        </li>)}
      </ul> : null}
      {state.result.completedCount > 0 ? <button type="button" onClick={onOpenLocation}>
        {copy.downloadOpenFolder}
      </button> : null}
    </section> : null}

    {error ? <p className="x-panel-error" role="alert">{error}</p> : null}
    {!eventsReady && state.analysis ? <p className="field-hint">{copy.tiktokEventsUnavailable}</p> : null}
    </div>
    <div className="options-footer">
    <section className="x-panel-section x-destination">
      <span>{copy.downloadDestinationLabel}</span>
      <Icon name="folder" size={16} /><strong title={destination?.displayName}>{destination?.displayName ?? copy.downloadNoDestination}</strong>
      <button type="button" disabled={locked} onClick={onOpenSettings}>{copy.openSettings}</button>
    </section>

    {state.job ? <section className="x-job-status" aria-live="polite">
      {status ? <p>{status}</p> : null}
      {locked ? <div
        className={`x-progress-track${state.job.percent === null ? " is-indeterminate" : ""}`}
        role="progressbar"
        aria-label={copy.downloadProgressLabel}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={state.job.percent ?? undefined}
      ><span style={state.job.percent === null ? undefined : { width: `${state.job.percent}%` }} /></div> : null}
      {locked ? <button type="button" className="x-cancel" disabled={state.job.cancelRequested} onClick={onCancel}>
        {copy.downloadCancel}
      </button> : null}
    </section> : null}

    {!locked ? <button type="button" className="x-download-start" disabled={!canStart} onClick={onStart}>
      <Icon name="download" size={18} />{state.result ? copy.downloadRetry : selectedCount > 0 && selectedCount === readyCount
        ? unavailableCount > 0 ? tiktokMessage(copy.tiktokDownloadAvailable, {count:selectedCount, total:postCount}) : tiktokMessage(copy.tiktokDownloadAll, {count:selectedCount})
        : tiktokMessage(copy.tiktokDownloadSelected, {count:selectedCount})}
    </button> : null}
    </div>
  </aside>;
}
