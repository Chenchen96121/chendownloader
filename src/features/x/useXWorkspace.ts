import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type {
  XAnalysisPreview,
  XDownloadProgress,
  XDownloadResult,
  XPreviewPayload,
} from "../../shared/contracts/x";
import type { AnalysisErrorCode, DownloadErrorCode } from "../../shared/contracts/errors";
import {
  XAnalysisIpcError,
  XDownloadIpcError,
  xIpc,
  type XIpc,
  type StartedXDownload,
} from "../../shared/ipc/x";
import {
  buildXDownloadIntent,
  createXWorkspaceState,
  xWorkspaceReducer,
  isXWorkspaceLocked,
  type XWorkspaceAction,
  type XWorkspaceState,
} from "./xState";

export interface XWorkspaceAccess {
  getState(): XWorkspaceState;
  apply(action: XWorkspaceAction): void;
}

function analysisErrorCode(error: unknown): AnalysisErrorCode {
  return error instanceof XAnalysisIpcError ? error.code : "analysis_unavailable";
}

function downloadErrorCode(error: unknown): DownloadErrorCode {
  return error instanceof XDownloadIpcError ? error.code : "unknown";
}

export interface XWorkspaceController {
  changeUrl(url: string): void;
  analyze(): Promise<XAnalysisPreview | null>;
  toggleSelection(itemId: string): void;
  selectAll(): void;
  clearSelection(): void;
  setCompatibilityMode(enabled: boolean): void;
  loadPreview(itemId: string): Promise<XPreviewPayload | null>;
  startDownload(destinationId: string): Promise<StartedXDownload | null>;
  detachDownload(jobId: string): void;
  receiveProgress(progress: XDownloadProgress): void;
  loadResult(jobId: string): Promise<XDownloadResult | null>;
  cancelDownload(): Promise<boolean>;
  openSource(): Promise<boolean>;
  openCompletedLocation(): Promise<boolean>;
  reset(): void;
}

export function createXWorkspaceController(
  access: XWorkspaceAccess,
  ipc: XIpc,
): XWorkspaceController {
  let previewRequestId = 0;

  return {
    detachDownload(jobId) { access.apply({ type: "downloadDetached", jobId }); },
    changeUrl(url) {
      access.apply({ type: "urlChanged", url });
    },

    async analyze() {
      const before = access.getState();
      if (isXWorkspaceLocked(before)) {
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
      const intent = buildXDownloadIntent(state);
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
        !isXWorkspaceLocked(current)
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

export function createXProgressRelay(access: XWorkspaceAccess, controller: XWorkspaceController) {
  const buffered = new Map<string, XDownloadProgress>();
  const loadedResults = new Set<string>();
  function deliver(progress: XDownloadProgress) {
    controller.receiveProgress(progress);
    if (access.getState().job?.state !== progress.state || !terminalStates.has(progress.state) || loadedResults.has(progress.jobId)) return;
    loadedResults.add(progress.jobId);
    if (loadedResults.size > 100) loadedResults.delete(loadedResults.values().next().value!);
    void controller.loadResult(progress.jobId);
  }
  return {
    receive(progress: XDownloadProgress) {
      const job = access.getState().job;
      if (job?.jobId === progress.jobId) { deliver(progress); return; }
      if (job?.state !== "starting" || job.jobId !== null) return;
      const previous = buffered.get(progress.jobId);
      if (previous && terminalStates.has(previous.state)) return;
      buffered.set(progress.jobId, progress);
      // Only the latest state is needed before the native start acknowledgement.
      if (buffered.size > 132) buffered.delete(buffered.keys().next().value!);
    },
    finishSubmission(jobId: string) {
      const progress = buffered.get(jobId);
      buffered.clear();
      if (progress && access.getState().job?.jobId === jobId) deliver(progress);
      controller.detachDownload(jobId);
    },
    resetBuffer() { buffered.clear(); },
    clear() { buffered.clear(); loadedResults.clear(); },
  };
}

export type XWorkspaceModel = Omit<XWorkspaceController, "detachDownload" | "receiveProgress" | "loadResult"> & {
  state: XWorkspaceState;
  eventsReady: boolean;
};

export function useXWorkspace(ipc: XIpc = xIpc): XWorkspaceModel {
  const [state, setState] = useState(createXWorkspaceState);
  const [eventsReady, setEventsReady] = useState(false);
  const stateRef = useRef(state);

  const apply = useCallback((action: XWorkspaceAction) => {
    const next = xWorkspaceReducer(stateRef.current, action);
    if (next !== stateRef.current) {
      stateRef.current = next;
      setState(next);
    }
  }, []);

  const access = useMemo<XWorkspaceAccess>(
    () => ({ getState: () => stateRef.current, apply }),
    [apply],
  );
  const controller = useMemo(
    () => createXWorkspaceController(access, ipc),
    [access, ipc],
  );

  const relay = useMemo(() => createXProgressRelay(access, controller), [access, controller]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void ipc
      .listenProgress((progress) => {
        if (!disposed) {
          relay.receive(progress);
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
      relay.clear();
    };
  }, [apply, ipc, relay]);

  const startDownload = useCallback(
    async (destinationId: string) => {
      relay.resetBuffer();
      const started = await controller.startDownload(destinationId);
      if (started === null) {
        relay.resetBuffer();
        return null;
      }
      relay.finishSubmission(started.jobId);
      return started;
    },
    [controller, relay],
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
