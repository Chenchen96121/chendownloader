import type { DestinationApproval } from "../../shared/contracts/download";
import { Icon } from "../../shared/ui/Icon";
import { catalogs } from "../../shared/i18n/catalog";
import type { Locale } from "../../shared/i18n/locale";
import {
  instagramCompatibilityApplies,
  isInstagramWorkspaceLocked,
  type InstagramWorkspaceState,
} from "./instagramState";

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

function progressMessage(state: InstagramWorkspaceState, locale: Locale): string | null {
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
    ? interpolate(copy.instagramItemProgress, {
        current: state.job.itemIndex,
        total: state.job.itemCount,
      })
    : null;
  return itemProgress ? `${phases[state.job.state]} ${itemProgress}` : phases[state.job.state];
}

export function InstagramDownloadPanel({
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
  state: InstagramWorkspaceState;
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
  const locked = isInstagramWorkspaceLocked(state);
  const selected = new Set(state.selectedItemIds);
  const selectedItems = state.analysis?.items.filter((item) => selected.has(item.itemId)) ?? [];
  const includesVideo = selectedItems.some((item) => item.kind === "video");
  const selectedCount = selectedItems.length;
  const canStart = state.analysis !== null
    && selectedCount > 0
    && destination !== null
    && eventsReady
    && !locked;
  const status = progressMessage(state, locale);
  const error = state.error === null
    ? null
    : state.error.scope === "analysis" || state.error.scope === "preview" || state.error.scope === "source"
      ? copy[`analysisError.${state.error.code}`]
      : copy[`downloadError.${state.error.code}`];

  return <aside className="instagram-download-panel" aria-labelledby="instagram-download-heading">
    <div className="options-scroll">
    <h3 id="instagram-download-heading">{copy.downloadOptionsLabel}</h3>

    <section className="instagram-panel-section" aria-labelledby="instagram-selection-heading">
      <div className="instagram-panel-heading">
        <h4 id="instagram-selection-heading">{copy.instagramSelectionRange}</h4>
        <span>{interpolate(copy.instagramSelectedCount, { count: selectedCount })}</span>
      </div>
      <div className="instagram-selection-actions">
        <button type="button" disabled={locked || state.analysis === null} onClick={onSelectAll}>
          {copy.instagramSelectAll}
        </button>
        <button type="button" disabled={locked || selectedCount === 0} onClick={onClearSelection}>
          {copy.instagramClearSelection}
        </button>
      </div>
      {selectedCount === 0 ? <p className="field-hint">{copy.instagramNoSelection}</p> : null}
    </section>

    <section className="instagram-panel-section instagram-compatibility">
      <label htmlFor="instagram-compatibility">
        <input
          id="instagram-compatibility"
          type="checkbox"
          checked={state.compatibilityMode}
          disabled={locked || !includesVideo}
          onChange={(event) => onCompatibilityChange(event.target.checked)}
        />
        <span>{copy.compatibilityLabel}</span>
      </label>
      <p className="field-hint">{includesVideo ? copy.compatibilityHint : copy.instagramCompatibilityVideoHint}</p>
      {instagramCompatibilityApplies(state) ? <p className="instagram-compatibility-active">H.264 + AAC · MP4</p> : null}
    </section>

    <section className="instagram-panel-section" aria-labelledby="instagram-summary-heading">
      <h4 id="instagram-summary-heading">{copy.instagramDownloadSummary}</h4>
      {selectedItems.length > 0 ? <ol className="instagram-selected-summary">
        {selectedItems.map((item) => <li key={item.itemId}>
          <span>{interpolate(copy.instagramItemLabel, { ordinal: item.ordinal })}</span>
          <div>
            <strong>{item.width !== undefined && item.height !== undefined
              ? `${item.width} × ${item.height} · `
              : ""}{item.container.toUpperCase()}</strong>
            <small>{compactBytes(item.fileSize?.bytes, copy.sizeUnknown)}</small>
            {item.kind === "video" ? <details className="media-extra-details">
              <summary>{copy.sourceDetailsLabel}<Icon name="chevron-down" size={14} /></summary>
              <small>{[
                  item.fps === undefined ? null : `${item.fps} FPS`,
                  `${copy.videoCodecLabel}: ${item.videoCodec?.toUpperCase() ?? copy.codecUnknown}`,
                  `${copy.audioCodecLabel}: ${item.audioCodec?.toUpperCase() ?? copy.noAudio}`,
                  item.dynamicRange ?? null,
                  item.videoBitrateKbps === undefined ? null : `${copy.videoBitrateLabel}: ${Math.round(item.videoBitrateKbps)} kbps`,
                  item.audioBitrateKbps === undefined ? null : `${copy.audioBitrateLabel}: ${Math.round(item.audioBitrateKbps)} kbps`,
                ].filter((value): value is string => value !== null).join(" · ")}</small>
            </details> : null}
          </div>
        </li>)}
      </ol> : <p className="field-hint">{copy.instagramNoSelection}</p>}
    </section>

    {state.result ? <section className={`instagram-result instagram-result--${state.result.state}`} aria-live="polite">
      <strong>{state.result.state === "partial"
        ? interpolate(copy.instagramPartialSummary, {
            completed: state.result.completedCount,
            failed: state.result.failedCount,
          })
        : state.result.state === "completed"
          ? interpolate(copy.instagramCompletedSummary, { count: state.result.completedCount })
          : state.result.state === "failed"
            ? interpolate(copy.instagramFailedSummary, { count: state.result.failedCount })
            : interpolate(copy.instagramCanceledSummary, {
                completed: state.result.completedCount,
              })}</strong>
      {state.result.items.some((item) => item.state === "failed") ? <ul>
        {state.result.items.filter((item) => item.state === "failed").map((item) => <li key={item.itemId}>
          <span>{interpolate(copy.instagramFailedItem, { ordinal: item.ordinal })}</span>
          <span>{item.errorCode ? copy[`downloadError.${item.errorCode}`] : copy.downloadFailed}</span>
        </li>)}
      </ul> : null}
      {state.result.completedCount > 0 ? <button type="button" onClick={onOpenLocation}>
        {copy.downloadOpenFolder}
      </button> : null}
    </section> : null}

    {error ? <p className="instagram-panel-error" role="alert">{error}</p> : null}
    {!eventsReady && state.analysis ? <p className="field-hint">{copy.instagramEventsUnavailable}</p> : null}
    </div>
    <div className="options-footer">
    <section className="instagram-panel-section instagram-destination">
      <span>{copy.downloadDestinationLabel}</span>
      <Icon name="folder" size={16} /><strong title={destination?.displayName}>{destination?.displayName ?? copy.downloadNoDestination}</strong>
      <button type="button" disabled={locked} onClick={onOpenSettings}>{copy.openSettings}</button>
    </section>

    {state.job ? <section className="instagram-job-status" aria-live="polite">
      {status ? <p>{status}</p> : null}
      {locked ? <div
        className={`instagram-progress-track${state.job.percent === null ? " is-indeterminate" : ""}`}
        role="progressbar"
        aria-label={copy.downloadProgressLabel}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={state.job.percent ?? undefined}
      ><span style={state.job.percent === null ? undefined : { width: `${state.job.percent}%` }} /></div> : null}
      {locked ? <button type="button" className="instagram-cancel" disabled={state.job.cancelRequested} onClick={onCancel}>
        {copy.downloadCancel}
      </button> : null}
    </section> : null}

    {!locked ? <button type="button" className="instagram-download-start" disabled={!canStart} onClick={onStart}>
      <Icon name="download" size={18} />{state.result ? copy.downloadRetry : copy.downloadStart}
    </button> : null}
    </div>
  </aside>;
}
