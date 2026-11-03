import type {
  InstagramAnalysisPreview,
  InstagramDownloadIntent,
  InstagramDownloadProgress,
  InstagramDownloadResult,
  InstagramJobState,
  InstagramPreviewPayload,
} from "../../shared/contracts/instagram";
import type { AnalysisErrorCode, DownloadErrorCode } from "../../shared/contracts/errors";

export type InstagramAnalysisStatus = "idle" | "analyzing" | "ready" | "error";
export type InstagramJobViewState = "starting" | InstagramJobState;

export interface InstagramPreviewState {
  status: "loading" | "ready" | "error";
  requestId: number;
  preview?: InstagramPreviewPayload;
  errorCode?: AnalysisErrorCode;
}

export interface InstagramJobView {
  jobId: string | null;
  generation: number;
  state: InstagramJobViewState;
  itemIndex: number;
  itemCount: number;
  downloadedBytes: number | null;
  totalBytes: number | null;
  percent: number | null;
  cancelRequested: boolean;
}

export type InstagramWorkspaceError =
  | { scope: "analysis"; code: AnalysisErrorCode }
  | { scope: "preview"; code: AnalysisErrorCode }
  | { scope: "source"; code: AnalysisErrorCode }
  | { scope: "download"; code: DownloadErrorCode }
  | { scope: "cancel"; code: DownloadErrorCode }
  | { scope: "location"; code: DownloadErrorCode };

export interface InstagramWorkspaceState {
  url: string;
  analysisStatus: InstagramAnalysisStatus;
  analysis: InstagramAnalysisPreview | null;
  selectedItemIds: string[];
  compatibilityMode: boolean;
  previews: Record<string, InstagramPreviewState>;
  requestGeneration: number;
  job: InstagramJobView | null;
  result: InstagramDownloadResult | null;
  error: InstagramWorkspaceError | null;
}

export type InstagramWorkspaceAction =
  | { type: "urlChanged"; url: string }
  | { type: "analysisStarted"; generation: number }
  | { type: "analysisSucceeded"; generation: number; analysis: InstagramAnalysisPreview }
  | {
      type: "analysisFailed";
      generation: number;
      errorCode: AnalysisErrorCode;
    }
  | { type: "selectionToggled"; itemId: string }
  | { type: "selectionAll" }
  | { type: "selectionCleared" }
  | { type: "compatibilityChanged"; value: boolean }
  | {
      type: "previewRequested";
      analysisId: string;
      itemId: string;
      requestId: number;
    }
  | {
      type: "previewLoaded";
      analysisId: string;
      itemId: string;
      requestId: number;
      preview: InstagramPreviewPayload;
    }
  | {
      type: "previewFailed";
      analysisId: string;
      itemId: string;
      requestId: number;
      errorCode: AnalysisErrorCode;
    }
  | {
      type: "downloadStartRequested";
      generation: number;
    }
  | { type: "downloadStarted"; generation: number; jobId: string; itemCount: number }
  | { type: "downloadDetached"; jobId: string }
  | {
      type: "downloadStartFailed";
      generation: number;
      errorCode: DownloadErrorCode;
    }
  | { type: "downloadProgress"; progress: InstagramDownloadProgress }
  | {
      type: "downloadFinished";
      result: InstagramDownloadResult;
    }
  | {
      type: "downloadResultFailed";
      jobId: string;
      errorCode: DownloadErrorCode;
    }
  | { type: "downloadListenerFailed"; errorCode: DownloadErrorCode }
  | { type: "cancelRequested"; jobId: string }
  | {
      type: "cancelFailed";
      jobId: string;
      errorCode: DownloadErrorCode;
    }
  | { type: "sourceFailed"; errorCode: AnalysisErrorCode }
  | { type: "locationFailed"; errorCode: DownloadErrorCode }
  | { type: "reset" };

const activeJobStates = new Set<InstagramJobViewState>([
  "starting",
  "queued",
  "downloading",
  "processing",
  "verifying",
  "publishing",
]);

const terminalJobStates = new Set<InstagramJobState>([
  "completed",
  "partial",
  "failed",
  "canceled",
]);

const itemStageRank: Partial<Record<InstagramJobState, number>> = {
  queued: 0,
  downloading: 1,
  processing: 2,
  verifying: 3,
  publishing: 4,
};

export function createInstagramWorkspaceState(): InstagramWorkspaceState {
  return {
    url: "",
    analysisStatus: "idle",
    analysis: null,
    selectedItemIds: [],
    compatibilityMode: false,
    previews: {},
    requestGeneration: 0,
    job: null,
    result: null,
    error: null,
  };
}

export function isInstagramWorkspaceLocked(state: InstagramWorkspaceState): boolean {
  return state.job !== null && activeJobStates.has(state.job.state);
}

function readyItemIds(analysis: InstagramAnalysisPreview): string[] {
  return analysis.items.filter((item) => item.availability === "ready").map((item) => item.itemId);
}

function selectedItems(state: InstagramWorkspaceState) {
  if (state.analysis === null) {
    return [];
  }

  const selected = new Set(state.selectedItemIds);
  return state.analysis.items.filter(
    (item) => item.availability === "ready" && selected.has(item.itemId),
  );
}

export function instagramCompatibilityApplies(state: InstagramWorkspaceState): boolean {
  return (
    state.compatibilityMode && selectedItems(state).some((item) => item.kind === "video")
  );
}

export function buildInstagramDownloadIntent(
  state: InstagramWorkspaceState,
): InstagramDownloadIntent | null {
  if (state.analysis === null || isInstagramWorkspaceLocked(state)) {
    return null;
  }

  const selected = selectedItems(state);
  if (selected.length === 0) {
    return null;
  }

  return {
    itemIds: selected.map((item) => item.itemId),
    compatibilityMode: instagramCompatibilityApplies(state),
  };
}

function resetWorkspace(url: string, requestGeneration: number): InstagramWorkspaceState {
  return {
    ...createInstagramWorkspaceState(),
    url,
    requestGeneration,
  };
}

function currentAnalysisHasItem(
  state: InstagramWorkspaceState,
  analysisId: string,
  itemId: string,
) {
  return (
    state.analysis?.analysisId === analysisId &&
    state.analysis.items.some(
      (item) => item.itemId === itemId && item.previewAvailable,
    )
  );
}

function boundedProgressValue(value: number | null): number | null {
  if (value === null || !Number.isFinite(value) || value < 0) {
    return null;
  }
  return value;
}

function applyDownloadProgress(
  state: InstagramWorkspaceState,
  progress: InstagramDownloadProgress,
): InstagramWorkspaceState {
  const job = state.job;
  if (job === null || job.jobId !== progress.jobId || progress.itemCount !== job.itemCount) {
    return state;
  }

  if (progress.itemIndex < job.itemIndex || progress.itemIndex > progress.itemCount) {
    return state;
  }

  if (terminalJobStates.has(job.state as InstagramJobState)) {
    return state;
  }

  if (terminalJobStates.has(progress.state)) {
    return {
      ...state,
      job: {
        ...job,
        state: progress.state,
        itemIndex: progress.itemIndex,
        downloadedBytes: boundedProgressValue(progress.downloadedBytes),
        totalBytes: boundedProgressValue(progress.totalBytes),
        percent: boundedProgressValue(progress.percent),
      },
    };
  }

  const nextRank = itemStageRank[progress.state];
  if (nextRank === undefined) {
    return state;
  }

  const changedItem = progress.itemIndex > job.itemIndex;
  if (!changedItem) {
    const previousRank = itemStageRank[job.state as InstagramJobState] ?? -1;
    if (nextRank < previousRank) {
      return state;
    }
  }

  const downloadedBytes = boundedProgressValue(progress.downloadedBytes);
  const totalBytes = boundedProgressValue(progress.totalBytes);
  const percent = boundedProgressValue(progress.percent);
  if (!changedItem) {
    if (
      downloadedBytes !== null &&
      job.downloadedBytes !== null &&
      downloadedBytes < job.downloadedBytes
    ) {
      return state;
    }
    if (percent !== null && job.percent !== null && percent < job.percent) {
      return state;
    }
  }

  return {
    ...state,
    job: {
      ...job,
      state: progress.state,
      itemIndex: progress.itemIndex,
      downloadedBytes,
      totalBytes,
      percent,
    },
  };
}

export function instagramWorkspaceReducer(
  state: InstagramWorkspaceState,
  action: InstagramWorkspaceAction,
): InstagramWorkspaceState {
  switch (action.type) {
    case "downloadDetached":
      return state.job?.jobId === action.jobId ? { ...state, job: null, result: null, requestGeneration: state.requestGeneration + 1 } : state;
    case "urlChanged":
      if (isInstagramWorkspaceLocked(state) || action.url === state.url) {
        return state;
      }
      return resetWorkspace(action.url, state.requestGeneration + 1);

    case "analysisStarted":
      if (isInstagramWorkspaceLocked(state) || action.generation <= state.requestGeneration) {
        return state;
      }
      return {
        ...resetWorkspace(state.url, action.generation),
        analysisStatus: "analyzing",
      };

    case "analysisSucceeded":
      if (
        action.generation !== state.requestGeneration ||
        state.analysisStatus !== "analyzing"
      ) {
        return state;
      }
      return {
        ...state,
        analysisStatus: "ready",
        analysis: action.analysis,
        selectedItemIds: readyItemIds(action.analysis),
        error: null,
      };

    case "analysisFailed":
      if (
        action.generation !== state.requestGeneration ||
        state.analysisStatus !== "analyzing"
      ) {
        return state;
      }
      return {
        ...state,
        analysisStatus: "error",
        error: { scope: "analysis", code: action.errorCode },
      };

    case "selectionToggled": {
      if (isInstagramWorkspaceLocked(state) || state.analysis === null) {
        return state;
      }
      const available = readyItemIds(state.analysis);
      if (!available.includes(action.itemId)) {
        return state;
      }
      const selected = new Set(state.selectedItemIds);
      if (selected.has(action.itemId)) {
        selected.delete(action.itemId);
      } else {
        selected.add(action.itemId);
      }
      return {
        ...state,
        selectedItemIds: available.filter((itemId) => selected.has(itemId)),
        result: null,
        error: null,
      };
    }

    case "selectionAll":
      if (isInstagramWorkspaceLocked(state) || state.analysis === null) {
        return state;
      }
      return {
        ...state,
        selectedItemIds: readyItemIds(state.analysis),
        result: null,
        error: null,
      };

    case "selectionCleared":
      if (isInstagramWorkspaceLocked(state)) {
        return state;
      }
      return { ...state, selectedItemIds: [], result: null, error: null };

    case "compatibilityChanged":
      if (isInstagramWorkspaceLocked(state)) {
        return state;
      }
      return { ...state, compatibilityMode: action.value, result: null, error: null };

    case "previewRequested": {
      if (
        !currentAnalysisHasItem(state, action.analysisId, action.itemId) ||
        action.requestId <= (state.previews[action.itemId]?.requestId ?? 0)
      ) {
        return state;
      }
      return {
        ...state,
        previews: {
          ...state.previews,
          [action.itemId]: { status: "loading", requestId: action.requestId },
        },
        error: state.error?.scope === "preview" ? null : state.error,
      };
    }

    case "previewLoaded": {
      const current = state.previews[action.itemId];
      if (
        !currentAnalysisHasItem(state, action.analysisId, action.itemId) ||
        current?.status !== "loading" ||
        current.requestId !== action.requestId
      ) {
        return state;
      }
      return {
        ...state,
        previews: {
          ...state.previews,
          [action.itemId]: {
            status: "ready",
            requestId: action.requestId,
            preview: action.preview,
          },
        },
        error: state.error?.scope === "preview" ? null : state.error,
      };
    }

    case "previewFailed": {
      const current = state.previews[action.itemId];
      if (
        !currentAnalysisHasItem(state, action.analysisId, action.itemId) ||
        current?.status !== "loading" ||
        current.requestId !== action.requestId
      ) {
        return state;
      }
      return {
        ...state,
        previews: {
          ...state.previews,
          [action.itemId]: {
            status: "error",
            requestId: action.requestId,
            errorCode: action.errorCode,
          },
        },
        error: state.error && state.error.scope !== "preview"
          ? state.error
          : { scope: "preview", code: action.errorCode },
      };
    }

    case "downloadStartRequested": {
      if (
        action.generation <= state.requestGeneration ||
        buildInstagramDownloadIntent(state) === null
      ) {
        return state;
      }
      return {
        ...state,
        requestGeneration: action.generation,
        job: {
          jobId: null,
          generation: action.generation,
          state: "starting",
          itemIndex: 0,
          itemCount: 0,
          downloadedBytes: null,
          totalBytes: null,
          percent: null,
          cancelRequested: false,
        },
        result: null,
        error: null,
      };
    }

    case "downloadStarted":
      if (
        state.job?.generation !== action.generation ||
        state.job.state !== "starting" ||
        state.requestGeneration !== action.generation
      ) {
        return state;
      }
      return {
        ...state,
        job: {
          ...state.job,
          jobId: action.jobId,
          state: "queued",
          itemCount: action.itemCount,
        },
      };

    case "downloadStartFailed":
      if (
        state.job?.generation !== action.generation ||
        state.job.state !== "starting" ||
        state.requestGeneration !== action.generation
      ) {
        return state;
      }
      return {
        ...state,
        job: null,
        error: { scope: "download", code: action.errorCode },
      };

    case "downloadProgress":
      return applyDownloadProgress(state, action.progress);

    case "downloadFinished":
      if (
        state.job?.jobId !== action.result.jobId ||
        !terminalJobStates.has(state.job.state as InstagramJobState) ||
        action.result.state !== state.job.state
      ) {
        return state;
      }
      return { ...state, result: action.result, error: null };

    case "downloadResultFailed":
      if (state.job?.jobId !== action.jobId) {
        return state;
      }
      return { ...state, error: { scope: "download", code: action.errorCode } };

    case "downloadListenerFailed":
      return { ...state, error: { scope: "download", code: action.errorCode } };

    case "cancelRequested":
      if (state.job?.jobId !== action.jobId || !isInstagramWorkspaceLocked(state)) {
        return state;
      }
      return { ...state, job: { ...state.job, cancelRequested: true }, error: null };

    case "cancelFailed":
      if (state.job?.jobId !== action.jobId) {
        return state;
      }
      return {
        ...state,
        job: { ...state.job, cancelRequested: false },
        error: { scope: "cancel", code: action.errorCode },
      };

    case "sourceFailed":
      return { ...state, error: { scope: "source", code: action.errorCode } };

    case "locationFailed":
      return { ...state, error: { scope: "location", code: action.errorCode } };

    case "reset":
      return resetWorkspace("", state.requestGeneration + 1);
  }
}
