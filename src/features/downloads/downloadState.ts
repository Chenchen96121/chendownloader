import type {
  DestinationApproval,
  DownloadErrorCode,
  DownloadProgress,
} from "../../shared/contracts/download";
import type { DownloadResult } from "../../shared/ipc/downloads";

export type DownloadUiPhase =
  | "idle"
  | "starting"
  | "preparing"
  | "downloading"
  | "processing"
  | "verifying"
  | "completed"
  | "failed"
  | "canceled";

export interface DownloadUiState {
  analysisId: string | null;
  destination: DestinationApproval | null;
  destinationPending: boolean;
  phase: DownloadUiPhase;
  jobId: string | null;
  downloadedBytes: number | null;
  totalBytes: number | null;
  percent: number | null;
  errorCode: DownloadErrorCode | null;
  displayName: string | null;
  cancelRequested: boolean;
  startGeneration: number;
}

const activePhases = new Set<DownloadUiPhase>([
  "starting", "preparing", "downloading", "processing", "verifying",
]);
const terminalPhases = new Set<DownloadUiPhase>(["completed", "failed", "canceled"]);
const phaseOrder: Readonly<Record<"preparing" | "downloading" | "processing" | "verifying", number>> = {
  preparing: 0,
  downloading: 1,
  processing: 2,
  verifying: 3,
};
const retryableErrors = new Set<DownloadErrorCode>([
  "rate_limited", "network_failed", "download_timeout", "process_failed", "output_invalid", "unknown",
]);

export function createDownloadUiState(analysisId: string | null = null): DownloadUiState {
  return {
    analysisId,
    destination: null,
    destinationPending: false,
    phase: "idle",
    jobId: null,
    downloadedBytes: null,
    totalBytes: null,
    percent: null,
    errorCode: null,
    displayName: null,
    cancelRequested: false,
    startGeneration: 0,
  };
}

function resetLifecycle(state: DownloadUiState): DownloadUiState {
  return {
    ...state,
    phase: "idle",
    jobId: null,
    downloadedBytes: null,
    totalBytes: null,
    percent: null,
    errorCode: null,
    displayName: null,
    cancelRequested: false,
    destinationPending: false,
    startGeneration: state.startGeneration + 1,
  };
}

export function detachSubmittedDownload(state: DownloadUiState, generation: number): DownloadUiState {
  return state.startGeneration === generation && state.jobId !== null ? resetLifecycle(state) : state;
}

export function synchronizeDownloadAnalysis(
  state: DownloadUiState,
  analysisId: string | null,
): DownloadUiState {
  if (state.analysisId === analysisId) return state;
  return {
    ...createDownloadUiState(analysisId),
    startGeneration: state.startGeneration + 1,
  };
}

export function invalidateDownloadSelection(state: DownloadUiState): DownloadUiState {
  if (isDownloadActive(state)) return state;
  return resetLifecycle(state);
}

export function selectDownloadDestination(
  state: DownloadUiState,
  destination: DestinationApproval | null,
): DownloadUiState {
  if (!destination || isDownloadActive(state)) return state;
  const reset = resetLifecycle(state);
  return { ...reset, destination };
}

export function beginDestinationSelection(state: DownloadUiState): DownloadUiState {
  if (!state.analysisId || state.destinationPending || isDownloadActive(state)) return state;
  return { ...state, destinationPending: true, errorCode: null };
}

// Default-setting approval refresh differs from an explicit folder-picker action.
export function refreshSavedDownloadDestination(
  state: DownloadUiState,
  destination: DestinationApproval,
  settingChanged: boolean,
): DownloadUiState {
  if (isDownloadActive(state)) return state;
  if (settingChanged && state.phase === "failed"
    && (state.errorCode === "destination_unavailable" || state.errorCode === "disk_full" || state.errorCode === "publication_failed")) {
    return selectDownloadDestination(state, destination);
  }
  return { ...state, destination };
}

export function finishDestinationSelection(
  state: DownloadUiState,
  analysisId: string,
  destination: DestinationApproval | null,
): DownloadUiState {
  if (!state.destinationPending || state.analysisId !== analysisId) return state;
  const settled = { ...state, destinationPending: false };
  return destination ? selectDownloadDestination(settled, destination) : settled;
}

export function failDestinationSelection(
  state: DownloadUiState,
  analysisId: string,
  errorCode: DownloadErrorCode,
): DownloadUiState {
  if (!state.destinationPending || state.analysisId !== analysisId) return state;
  return { ...state, destinationPending: false, errorCode };
}

export function canStartDownload(state: DownloadUiState, selectionReady: boolean): boolean {
  return state.phase === "idle"
    && !state.destinationPending
    && state.analysisId !== null
    && state.destination !== null
    && selectionReady;
}

export function isDownloadActive(state: DownloadUiState): boolean {
  return activePhases.has(state.phase);
}

export function beginDownloadStart(state: DownloadUiState): {
  state: DownloadUiState;
  generation: number;
} {
  const generation = state.startGeneration + 1;
  return {
    generation,
    state: {
      ...state,
      phase: "starting",
      jobId: null,
      downloadedBytes: null,
      totalBytes: null,
      percent: null,
      errorCode: null,
      displayName: null,
      cancelRequested: false,
      startGeneration: generation,
    },
  };
}

export function finishDownloadStart(
  state: DownloadUiState,
  generation: number,
  jobId: string,
): DownloadUiState {
  if (state.phase !== "starting" || state.startGeneration !== generation) return state;
  return { ...state, phase: "preparing", jobId };
}

export function failDownloadStart(
  state: DownloadUiState,
  generation: number,
  errorCode: DownloadErrorCode,
): DownloadUiState {
  if (state.phase !== "starting" || state.startGeneration !== generation) return state;
  return { ...state, phase: "failed", jobId: null, errorCode };
}

function maxNullable(current: number | null, incoming: number | null): number | null {
  if (current === null) return incoming;
  if (incoming === null) return current;
  return Math.max(current, incoming);
}

export function applyDownloadProgress(
  state: DownloadUiState,
  progress: DownloadProgress,
): DownloadUiState {
  if (state.jobId !== progress.jobId || terminalPhases.has(state.phase)) return state;
  if (progress.state === "completed" || progress.state === "failed" || progress.state === "canceled") {
    return {
      ...state,
      phase: progress.state,
      cancelRequested: false,
      errorCode: null,
      displayName: null,
    };
  }

  const currentOrder = state.phase === "starting" ? -1 : phaseOrder[state.phase as keyof typeof phaseOrder];
  const nextOrder = phaseOrder[progress.state];
  if (currentOrder !== undefined && nextOrder < currentOrder) return state;
  return {
    ...state,
    phase: progress.state,
    downloadedBytes: maxNullable(state.downloadedBytes, progress.downloadedBytes),
    totalBytes: maxNullable(state.totalBytes, progress.totalBytes),
    percent: maxNullable(state.percent, progress.percent),
    errorCode: null,
  };
}

export function requestDownloadCancel(state: DownloadUiState, jobId: string): DownloadUiState {
  if (!isDownloadActive(state) || state.phase === "starting" || state.jobId !== jobId) return state;
  if (state.cancelRequested) return state;
  return { ...state, cancelRequested: true, errorCode: null };
}

export function finishDownloadCancelRequest(
  state: DownloadUiState,
  jobId: string,
  errorCode: DownloadErrorCode,
): DownloadUiState {
  if (state.jobId !== jobId || !isDownloadActive(state)) return state;
  return { ...state, cancelRequested: false, errorCode };
}

export function applyDownloadResult(
  state: DownloadUiState,
  result: DownloadResult,
): DownloadUiState {
  if (state.jobId !== result.jobId || state.phase !== result.state) return state;
  if (state.errorCode === result.errorCode && state.displayName === result.displayName) return state;
  return {
    ...state,
    errorCode: result.errorCode,
    displayName: result.displayName,
    cancelRequested: false,
  };
}

export function recordDownloadActionError(
  state: DownloadUiState,
  errorCode: DownloadErrorCode,
): DownloadUiState {
  return state.errorCode === errorCode ? state : { ...state, errorCode };
}

export function retryDownload(state: DownloadUiState): DownloadUiState {
  return terminalPhases.has(state.phase) ? resetLifecycle(state) : state;
}

export function isRetryableDownloadError(errorCode: DownloadErrorCode | null): boolean {
  return errorCode !== null && retryableErrors.has(errorCode);
}
