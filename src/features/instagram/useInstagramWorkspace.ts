import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type {
  InstagramAnalysisPreview,
  InstagramDownloadProgress,
  InstagramDownloadResult,
  InstagramPreviewPayload,
} from "../../shared/contracts/instagram";
import type { AnalysisErrorCode, DownloadErrorCode } from "../../shared/contracts/errors";
import {
  InstagramAnalysisIpcError,
  InstagramDownloadIpcError,
  instagramIpc,
  type InstagramIpc,
  type StartedInstagramDownload,
} from "../../shared/ipc/instagram";
import {
  buildInstagramDownloadIntent,
  createInstagramWorkspaceState,
  instagramWorkspaceReducer,
  isInstagramWorkspaceLocked,
  type InstagramWorkspaceAction,
  type InstagramWorkspaceState,
} from "./instagramState";

export interface InstagramWorkspaceAccess {
  getState(): InstagramWorkspaceState;
  apply(action: InstagramWorkspaceAction): void;
}

function analysisErrorCode(error: unknown): AnalysisErrorCode {
  return error instanceof InstagramAnalysisIpcError ? error.code : "analysis_unavailable";
}

function downloadErrorCode(error: unknown): DownloadErrorCode {
  return error instanceof InstagramDownloadIpcError ? error.code : "unknown";
}

export interface InstagramWorkspaceController {
  changeUrl(url: string): void;
  analyze(): Promise<InstagramAnalysisPreview | null>;
  toggleSelection(itemId: string): void;
  selectAll(): void;
  clearSelection(): void;
  setCompatibilityMode(enabled: boolean): void;
  loadPreview(itemId: string): Promise<InstagramPreviewPayload | null>;
  startDownload(destinationId: string): Promise<StartedInstagramDownload | null>;
  detachDownload(jobId: string): void;
  receiveProgress(progress: InstagramDownloadProgress): void;
  loadResult(jobId: string): Promise<InstagramDownloadResult | null>;
  cancelDownload(): Promise<boolean>;
  openSource(): Promise<boolean>;
  openCompletedLocation(): Promise<boolean>;
  reset(): void;
}

export function createInstagramWorkspaceController(
  access: InstagramWorkspaceAccess,
  ipc: InstagramIpc,
): InstagramWorkspaceController {
  let previewRequestId = 0;

  return {
    detachDownload(jobId) { access.apply({ type: "downloadDetached", jobId }); },
    changeUrl(url) {
      access.apply({ type: "urlChanged", url });
    },

    async analyze() {
      const before = access.getState();
      if (isInstagramWorkspaceLocked(before)) {
        return null;
      }
      const url = before.url.trim();
      const generation = before.requestGeneration + 1;
      access.apply({ type: "analysisStarted", generation });

      try {
        const analysis = await ipc.analyze(url);
        access.apply({ type: "analysisSucceeded", generation, analysis });
        return access.getState().analysis === analysis ? analysis : null;
      } catch (error) {
        access.apply({
          type: "analysisFailed",
          generation,
          errorCode: analysisErrorCode(error),
        });
        return null;
      }
    },

    toggleSelection(itemId) {
      access.apply({ type: "selectionToggled", itemId });
    },

    selectAll() {
      access.apply({ type: "selectionAll" });
    },

    clearSelection() {
      access.apply({ type: "selectionCleared" });
    },

    setCompatibilityMode(enabled) {
      access.apply({ type: "compatibilityChanged", value: enabled });
    },

    async loadPreview(itemId) {
      const analysis = access.getState().analysis;
      if (analysis === null) {
        return null;
      }

      const analysisId = analysis.analysisId;
      const requestId = ++previewRequestId;
      access.apply({ type: "previewRequested", analysisId, itemId, requestId });

      try {
        const preview = await ipc.getPreview(analysisId, itemId);
        access.apply({ type: "previewLoaded", analysisId, itemId, requestId, preview });
        return access.getState().previews[itemId]?.preview === preview ? preview : null;
      } catch (error) {
        access.apply({
          type: "previewFailed",
          analysisId,
          itemId,
          requestId,
          errorCode: analysisErrorCode(error),
        });
        return null;
      }
    },

    async startDownload(destinationId) {
      const state = access.getState();
      const intent = buildInstagramDownloadIntent(state);
      if (intent === null) {
        return null;
      }

      const generation = state.requestGeneration + 1;
      access.apply({
        type: "downloadStartRequested",
        generation,
      });

      try {
        const started = await ipc.startDownload(
          state.analysis?.analysisId ?? "",
          destinationId,
          intent,
        );
        access.apply({
          type: "downloadStarted",
          generation,
          jobId: started.jobId,
          itemCount: intent.itemIds.length,
        });
        return access.getState().job?.jobId === started.jobId ? started : null;
      } catch (error) {
        access.apply({
          type: "downloadStartFailed",
          generation,
          errorCode: downloadErrorCode(error),
        });
        return null;
      }
    },

    receiveProgress(progress) {
      access.apply({ type: "downloadProgress", progress });
    },

    async loadResult(jobId) {
      try {
        const result = await ipc.getDownloadResult(jobId);
        access.apply({ type: "downloadFinished", result });
        return access.getState().result === result ? result : null;
      } catch (error) {
        access.apply({
          type: "downloadResultFailed",
          jobId,
          errorCode: downloadErrorCode(error),
        });
        return null;
      }
    },

    async cancelDownload() {
      const current = access.getState();
      const jobId = current.job?.jobId;
      if (
        jobId === null ||
        jobId === undefined ||
        current.job?.cancelRequested === true ||
        !isInstagramWorkspaceLocked(current)
      ) {
        return false;
      }
      access.apply({ type: "cancelRequested", jobId });
      try {
        await ipc.cancelDownload(jobId);
        return true;
      } catch (error) {
        access.apply({ type: "cancelFailed", jobId, errorCode: downloadErrorCode(error) });
        return false;
      }
    },

    async openSource() {
      const analysisId = access.getState().analysis?.analysisId;
      if (analysisId === undefined) {
        return false;
      }
      try {
        await ipc.openSource(analysisId);
        return true;
      } catch (error) {
        access.apply({ type: "sourceFailed", errorCode: analysisErrorCode(error) });
        return false;
      }
    },

    async openCompletedLocation() {
      const jobId = access.getState().result?.jobId;
      if (jobId === undefined) {
        return false;
      }
      try {
        await ipc.openCompletedLocation(jobId);
        return true;
      } catch (error) {
        access.apply({ type: "locationFailed", errorCode: downloadErrorCode(error) });
        return false;
      }
    },

    reset() {
      access.apply({ type: "reset" });
    },
  };
}

const terminalStates = new Set(["completed", "partial", "failed", "canceled"]);

export function useInstagramWorkspace(ipc: InstagramIpc = instagramIpc) {
  const [state, setState] = useState(createInstagramWorkspaceState);
  const [eventsReady, setEventsReady] = useState(false);
  const stateRef = useRef(state);
  const bufferedProgressRef = useRef(new Map<string, InstagramDownloadProgress[]>());
  const loadedResultsRef = useRef(new Set<string>());

  const apply = useCallback((action: InstagramWorkspaceAction) => {
    const next = instagramWorkspaceReducer(stateRef.current, action);
    if (next !== stateRef.current) {
      stateRef.current = next;
      setState(next);
    }
  }, []);

  const access = useMemo<InstagramWorkspaceAccess>(
    () => ({ getState: () => stateRef.current, apply }),
    [apply],
  );
  const controller = useMemo(
    () => createInstagramWorkspaceController(access, ipc),
    [access, ipc],
  );

  const loadTerminalResult = useCallback(
    (progress: InstagramDownloadProgress) => {
      if (!terminalStates.has(progress.state) || loadedResultsRef.current.has(progress.jobId)) {
        return;
      }
      loadedResultsRef.current.add(progress.jobId);
      void controller.loadResult(progress.jobId);
    },
    [controller],
  );

  const receiveOrBufferProgress = useCallback(
    (progress: InstagramDownloadProgress) => {
      const job = stateRef.current.job;
      if (job?.jobId === progress.jobId) {
        controller.receiveProgress(progress);
        loadTerminalResult(progress);
        return;
      }
      if (job?.state === "starting" && job.jobId === null) {
        const buffered = bufferedProgressRef.current.get(progress.jobId) ?? [];
        buffered.push(progress);
        bufferedProgressRef.current.set(progress.jobId, buffered);
      }
    },
    [controller, loadTerminalResult],
  );

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void ipc
      .listenProgress((progress) => {
        if (!disposed) {
          receiveOrBufferProgress(progress);
        }
      })
      .then((cleanup) => {
        if (disposed) {
          cleanup();
        } else {
          unlisten = cleanup;
          setEventsReady(true);
        }
      })
      .catch((error: unknown) => {
        if (!disposed) {
          setEventsReady(false);
          apply({ type: "downloadListenerFailed", errorCode: downloadErrorCode(error) });
        }
      });

    return () => {
      disposed = true;
      unlisten?.();
      bufferedProgressRef.current.clear();
      loadedResultsRef.current.clear();
    };
  }, [apply, ipc, receiveOrBufferProgress]);

  const startDownload = useCallback(
    async (destinationId: string) => {
      bufferedProgressRef.current.clear();
      const started = await controller.startDownload(destinationId);
      if (started === null) {
        bufferedProgressRef.current.clear();
        return null;
      }
      const buffered = bufferedProgressRef.current.get(started.jobId) ?? [];
      bufferedProgressRef.current.delete(started.jobId);
      for (const progress of buffered) {
        controller.receiveProgress(progress);
        loadTerminalResult(progress);
      }
      controller.detachDownload(started.jobId);
      return started;
    },
    [controller, loadTerminalResult],
  );

  return {
    state,
    eventsReady,
    changeUrl: controller.changeUrl,
    analyze: controller.analyze,
    toggleSelection: controller.toggleSelection,
    selectAll: controller.selectAll,
    clearSelection: controller.clearSelection,
    setCompatibilityMode: controller.setCompatibilityMode,
    loadPreview: controller.loadPreview,
    startDownload,
    cancelDownload: controller.cancelDownload,
    openSource: controller.openSource,
    openCompletedLocation: controller.openCompletedLocation,
    reset: controller.reset,
  };
}
