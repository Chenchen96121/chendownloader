import type { DownloadErrorCode } from "../../shared/contracts/download";
import { catalogs, type Catalog } from "../../shared/i18n/catalog";
import type { Locale } from "../../shared/i18n/locale";
import { Icon } from "../../shared/ui/Icon";
import {
  isDownloadActive,
  isRetryableDownloadError,
  type DownloadUiPhase,
  type DownloadUiState,
} from "./downloadState";

const errorActions: Record<DownloadErrorCode, keyof Catalog> = {
  invalid_request: "downloadActionAnalyzeAgain",
  analysis_unavailable: "downloadActionAnalyzeAgain",
  destination_unavailable: "downloadActionChooseFolder",
  format_unavailable: "downloadActionAdjustSelection",
  download_in_progress: "downloadActionWait",
  tool_missing: "downloadActionCheckTools",
  tool_integrity_failed: "downloadActionCheckTools",
  authentication_required: "downloadActionPublicMedia",
  access_denied: "downloadActionPublicMedia",
  content_unavailable: "downloadActionTryAnother",
  content_changed: "downloadActionAnalyzeAgain",
  rate_limited: "downloadActionRetry",
  extractor_outdated: "downloadActionUpdateTools",
  network_failed: "downloadActionRetry",
  download_timeout: "downloadActionRetry",
  process_failed: "downloadActionRetry",
  output_invalid: "downloadActionRetry",
  image_content_invalid: "downloadActionAnalyzeAgain",
  image_dimensions_mismatch: "downloadActionAnalyzeAgain",
  image_size_mismatch: "downloadActionAnalyzeAgain",
  image_probe_failed: "downloadActionCheckTools",
  disk_full: "downloadActionChooseFolder",
  publication_failed: "downloadActionChooseFolder",
  unknown: "downloadActionRetry",
};

const statusKeys: Record<Exclude<DownloadUiPhase, "idle" | "completed" | "failed" | "canceled">, keyof Catalog> = {
  starting: "downloadStarting",
  preparing: "downloadPreparing",
  downloading: "downloadDownloading",
  processing: "downloadProcessing",
  verifying: "downloadVerifying",
};

function formatBytes(bytes: number, locale: Locale): string {
  if (bytes === 0) return "0 B";
  const scale = bytes >= 1_000_000_000 ? 3 : bytes >= 1_000_000 ? 2 : bytes >= 1000 ? 1 : 0;
  const units = ["B", "KB", "MB", "GB"] as const;
  const amount = scale === 0
    ? String(bytes)
    : new Intl.NumberFormat(locale, { minimumFractionDigits: 2, maximumFractionDigits: 2 })
      .format(bytes / 1000 ** scale);
  return `${amount} ${units[scale]}`;
}

function statusMessage(state: DownloadUiState, canStart: boolean, copy: Catalog): string {
  if (state.destinationPending) return copy.downloadChoosingFolder;
  if (state.cancelRequested) return copy.downloadCanceling;
  if (state.phase === "idle") return canStart ? copy.downloadReady : copy.downloadNeedsSelection;
  if (state.phase === "completed") return state.displayName ? copy.downloadCompleted : copy.downloadFinalizing;
  if (state.phase === "failed") return state.errorCode ? copy.downloadFailed : copy.downloadFinalizing;
  if (state.phase === "canceled") return copy.downloadCanceled;
  return copy[statusKeys[state.phase]];
}

export interface DownloadControlsProps {
  namespace?: string;
  locale: Locale;
  state: DownloadUiState;
  canStart: boolean;
  onChooseDestination: () => void;
  onStart: () => void;
  onCancel: () => void;
  onRetry: () => void;
  onOpenLocation: () => void;
}

export function DownloadControls({
  namespace,
  locale,
  state,
  canStart,
  onChooseDestination,
  onStart,
  onCancel,
  onRetry,
  onOpenLocation,
}: DownloadControlsProps) {
  const copy = catalogs[locale];
  const headingId = namespace ? `${namespace}-download-heading` : "download-heading";
  const active = isDownloadActive(state);
  const progressActive = state.phase === "preparing"
    || state.phase === "downloading"
    || state.phase === "processing"
    || state.phase === "verifying";
  const showRetry = state.phase === "canceled"
    || (state.phase === "failed" && isRetryableDownloadError(state.errorCode));
  const progressText = state.downloadedBytes === null
    ? null
    : state.totalBytes === null
      ? formatBytes(state.downloadedBytes, locale)
      : `${formatBytes(state.downloadedBytes, locale)} / ${formatBytes(state.totalBytes, locale)}`;

  return (
    <section
      className="download-controls"
      data-phase={state.phase}
      aria-labelledby={headingId}
      aria-busy={state.destinationPending || active}
    >
      <h3 id={headingId}>{copy.downloadHeading}</h3>
      <div className="download-destination">
        <Icon name="folder" size={18} />
        <div>
          <span className="download-field-label">{copy.downloadDestinationLabel}</span>
          <strong title={state.destination?.displayName}>{state.destination?.displayName ?? copy.downloadNoDestination}</strong>
        </div>
        <button type="button" className="settings-shortcut" disabled={state.destinationPending || active} onClick={onChooseDestination}>{copy.openSettings}</button>
      </div>

      <button className="download-start" type="button" disabled={!canStart} onClick={onStart}>
        <Icon name="download" size={18} />{copy.downloadStart}
      </button>

      <p className="download-status" role="status">{statusMessage(state, canStart, copy)}</p>

      {progressActive ? (
        <div className="download-progress">
          <div
            className="download-progress-track"
            role="progressbar"
            aria-label={copy.downloadProgressLabel}
            aria-valuemin={0}
            aria-valuemax={100}
            {...(state.percent === null ? {} : { "aria-valuenow": state.percent })}
          >
            {state.percent === null ? null : <span style={{ width: `${state.percent}%` }} />}
          </div>
          {progressText || state.percent !== null ? (
            <p className="download-progress-copy">
              {progressText ? <span>{copy.downloadTransferred} {progressText}</span> : null}
              {state.percent !== null ? <strong>{Math.floor(state.percent)}%</strong> : null}
            </p>
          ) : null}
        </div>
      ) : null}

      {state.errorCode ? (
        <p className="download-error">
          {copy[`downloadError.${state.errorCode}`]} <span>{copy[errorActions[state.errorCode]]}</span>
        </p>
      ) : null}

      {state.phase === "completed" && state.displayName ? (
        <p className="download-completed-name">
          <span>{copy.downloadCompletedFileLabel}</span><strong title={state.displayName}>{state.displayName}</strong>
        </p>
      ) : null}

      <div className="download-actions">
        {active && state.jobId ? (
          <button type="button" disabled={state.cancelRequested} onClick={onCancel}><Icon name="close" size={16} />{copy.downloadCancel}</button>
        ) : null}
        {showRetry ? <button type="button" onClick={onRetry}>{copy.downloadRetry}</button> : null}
        {state.phase === "completed" && state.displayName ? (
          <button type="button" onClick={onOpenLocation}><Icon name="folder" size={16} />{copy.downloadOpenFolder}</button>
        ) : null}
      </div>
    </section>
  );
}
