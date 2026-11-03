import { describe, expect, it, vi } from "vitest";
import type { XAnalysisPreview, XPreviewPayload } from "../../shared/contracts/x";
import type { XIpc } from "../../shared/ipc/x";
import {
  createXWorkspaceController,
  createXProgressRelay,
  type XWorkspaceAccess,
} from "./useXWorkspace";
import {
  buildXDownloadIntent,
  createXWorkspaceState,
  xWorkspaceReducer,
  type XWorkspaceAction,
} from "./xState";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

const capabilities = {
  postBatch: true, images: true, videos: true, compatibility: true,
  clip: false, manualQuality: false, audioOnly: false, videoOnly: false,
} as const;
function analysis(analysisId: string, shortcode: string): XAnalysisPreview {
  return {
    analysisId, postId: `${analysisId}00`, title: shortcode,
    sourcePageUrl: `https://x.com/i/web/status/${analysisId}00`, capabilities,
    items: [{ itemId: analysisId, ordinal: 1, kind: "image", container: "jpg", previewAvailable: true, availability: "ready" }],
  };
}

function unavailableVideo(analysisId: string, shortcode: string): XAnalysisPreview {
  return {
    ...analysis(analysisId, shortcode),
    items: [{
      itemId: analysisId, ordinal: 1, kind: "video", container: "mp4",
      previewAvailable: true, availability: "unavailable", unavailableReason: "analysis_unavailable",
    }],
  };
}

function harness(ipc: XIpc) {
  let state = createXWorkspaceState();
  const access: XWorkspaceAccess = {
    getState: () => state,
    apply: (action: XWorkspaceAction) => { state = xWorkspaceReducer(state, action); },
  };
  const controller = createXWorkspaceController(access, ipc);
  return { controller, state: () => state, relay: createXProgressRelay(access, controller) };
}

function fakeIpc(overrides: Partial<XIpc>): XIpc {
  return {
    analyze: vi.fn(), getPreview: vi.fn(), openSource: vi.fn(), startDownload: vi.fn(),
    cancelDownload: vi.fn(), getDownloadResult: vi.fn(), openCompletedLocation: vi.fn(),
    listenProgress: vi.fn(async () => () => undefined), ...overrides,
  };
}

describe("X workspace async controller", () => {
  it("replays an early terminal once, detaches immediately and preserves the submitted directory and selection", async () => {
    const start = deferred<{jobId:string}>();
    const ipc = fakeIpc({analyze:vi.fn().mockResolvedValue(analysis("1","Post_1")),startDownload:vi.fn().mockReturnValue(start.promise),getDownloadResult:vi.fn().mockResolvedValue({jobId:"7",state:"completed",postItemCount:1,unavailableCount:0,contentChanged:false,completedCount:1,failedCount:0,folderDisplayName:"Post",items:[{itemId:"1",ordinal:1,kind:"image",state:"completed",displayName:"01.jpg"}]})});
    const {controller,state,relay}=harness(ipc);
    controller.changeUrl("https://x.com/i/web/status/100"); await controller.analyze();
    const pending=controller.startDownload("3");
    const terminal={jobId:"7",state:"completed" as const,itemIndex:1,itemCount:1,downloadedBytes:null,totalBytes:null,percent:null};
    relay.receive(terminal); relay.receive(terminal);
    start.resolve({jobId:"7"}); await pending;
    relay.finishSubmission("7");
    expect(state().job).toBeNull();
    relay.receive(terminal);
    expect(ipc.getDownloadResult).toHaveBeenCalledExactlyOnceWith("7");
    expect(ipc.startDownload).toHaveBeenCalledExactlyOnceWith("1","3",{itemIds:["1"],compatibilityMode:false});
    controller.changeUrl("https://x.com/i/web/status/200");
    expect(ipc.cancelDownload).not.toHaveBeenCalled();
  });

  it("deduplicates current terminal events and ignores foreign jobs and stale progress", async () => {
    const ipc=fakeIpc({analyze:vi.fn().mockResolvedValue(analysis("1","Post_1")),startDownload:vi.fn().mockResolvedValue({jobId:"7"}),getDownloadResult:vi.fn()});
    const {controller,state,relay}=harness(ipc);
    controller.changeUrl("https://x.com/i/web/status/100"); await controller.analyze(); await controller.startDownload("3");
    const terminal={jobId:"7",state:"completed" as const,itemIndex:1,itemCount:1,downloadedBytes:null,totalBytes:null,percent:null};
    relay.receive({...terminal,jobId:"8"});
    expect(state().job?.state).toBe("queued");
    relay.receive(terminal); relay.receive(terminal);
    relay.receive({...terminal,state:"downloading"});
    expect(state().job?.state).toBe("completed");
    expect(ipc.getDownloadResult).toHaveBeenCalledExactlyOnceWith("7");
  });
  it("loads an unavailable video's cover without enabling its selection or download", async () => {
    const ipc = fakeIpc({
      analyze: vi.fn().mockResolvedValue(unavailableVideo("1", "Post_1")),
      getPreview: vi.fn().mockResolvedValue({ mimeType: "image/jpeg", dataBase64: "AAAA" }),
    });
    const { controller, state } = harness(ipc);
    controller.changeUrl("https://x.com/i/web/status/500");
    await controller.analyze();
    await controller.loadPreview("1");
    expect(state().previews["1"]).toMatchObject({
      status: "ready", preview: { mimeType: "image/jpeg", dataBase64: "AAAA" },
    });
    controller.toggleSelection("1");
    controller.selectAll();
    expect(state().selectedItemIds).toEqual([]);
    expect(buildXDownloadIntent(state())).toBeNull();
    expect(await controller.startDownload("3")).toBeNull();
    expect(state().job).toBeNull();
  });

  it("detaches a submitted native job so another post can be analyzed without canceling it", async () => {
    const ipc = fakeIpc({ analyze: vi.fn().mockResolvedValueOnce(analysis("1", "Post_1")).mockResolvedValueOnce(analysis("2", "Post_2")), startDownload: vi.fn().mockResolvedValue({ jobId: "7" }) });
    const { controller, state } = harness(ipc);
    controller.changeUrl("https://x.com/i/web/status/500"); await controller.analyze();
    await controller.startDownload("3"); controller.detachDownload("7");
    controller.changeUrl("https://x.com/i/web/status/501"); await controller.analyze();
    expect(state().analysis?.title).toBe("Post_2"); expect(state().job).toBeNull();
    expect(ipc.cancelDownload).not.toHaveBeenCalled();
  });
  it("prevents an older analysis response from replacing a newer URL", async () => {
    const first = deferred<XAnalysisPreview>();
    const second = deferred<XAnalysisPreview>();
    const ipc = fakeIpc({ analyze: vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise) });
    const { controller, state } = harness(ipc);
    controller.changeUrl("https://x.com/i/web/status/502");
    const oldRequest = controller.analyze();
    controller.changeUrl("https://x.com/i/web/status/503");
    const newRequest = controller.analyze();
    second.resolve(analysis("2", "New_2"));
    await newRequest;
    first.resolve(analysis("1", "Old_1"));
    await oldRequest;
    expect(state().analysis?.title).toBe("New_2");
  });

  it("ignores an older cover response for the same unavailable video", async () => {
    const first = deferred<XPreviewPayload>();
    const second = deferred<XPreviewPayload>();
    const ipc = fakeIpc({
      analyze: vi.fn().mockResolvedValue(unavailableVideo("1", "Post_1")),
      getPreview: vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise),
    });
    const { controller, state } = harness(ipc);
    controller.changeUrl("https://x.com/i/web/status/500");
    await controller.analyze();
    const oldRequest = controller.loadPreview("1");
    const newRequest = controller.loadPreview("1");
    second.resolve({ mimeType: "image/png", dataBase64: "BBBB" });
    await newRequest;
    first.resolve({ mimeType: "image/jpeg", dataBase64: "AAAA" });
    await oldRequest;
    expect(state().previews["1"]).toMatchObject({ status: "ready", preview: { mimeType: "image/png", dataBase64: "BBBB" } });
  });

  it("drops a preview response from a previous analysis even when item IDs match", async () => {
    const oldPreview = deferred<XPreviewPayload>();
    const ipc = fakeIpc({
      analyze: vi.fn()
        .mockResolvedValueOnce(unavailableVideo("1", "Post_1"))
        .mockResolvedValueOnce({
          ...unavailableVideo("2", "Post_2"),
          items: unavailableVideo("1", "Post_2").items,
        }),
      getPreview: vi.fn().mockReturnValue(oldPreview.promise),
    });
    const { controller, state } = harness(ipc);
    controller.changeUrl("https://x.com/i/web/status/500");
    await controller.analyze();
    const pending = controller.loadPreview("1");
    controller.changeUrl("https://x.com/i/web/status/501");
    await controller.analyze();
    oldPreview.resolve({ mimeType: "image/jpeg", dataBase64: "AAAA" });
    await pending;
    expect(state().analysis?.title).toBe("Post_2");
    expect(state().previews).toEqual({});
  });

  it("does not let a late start revive a reset workspace", async () => {
    const start = deferred<{ jobId: string }>();
    const ipc = fakeIpc({
      analyze: vi.fn().mockResolvedValue(analysis("1", "Post_1")),
      startDownload: vi.fn().mockReturnValue(start.promise),
    });
    const { controller, state } = harness(ipc);
    controller.changeUrl("https://x.com/i/web/status/500");
    await controller.analyze();
    const pending = controller.startDownload("3");
    controller.reset();
    start.resolve({ jobId: "7" });
    await pending;
    expect(state()).toMatchObject({ analysis: null, job: null, result: null });
  });

  it("keeps the newer download when two start responses resolve in reverse order", async () => {
    const first = deferred<{ jobId: string }>();
    const second = deferred<{ jobId: string }>();
    const ipc = fakeIpc({
      analyze: vi.fn()
        .mockResolvedValueOnce(analysis("1", "Post_1"))
        .mockResolvedValueOnce(analysis("2", "Post_2")),
      startDownload: vi.fn()
        .mockReturnValueOnce(first.promise)
        .mockReturnValueOnce(second.promise),
    });
    const { controller, state } = harness(ipc);
    controller.changeUrl("https://x.com/i/web/status/500");
    await controller.analyze();
    const olderStart = controller.startDownload("3");
    controller.reset();
    controller.changeUrl("https://x.com/i/web/status/501");
    await controller.analyze();
    const newerStart = controller.startDownload("4");
    second.resolve({ jobId: "8" });
    await newerStart;
    first.resolve({ jobId: "7" });
    await olderStart;
    expect(state().analysis?.analysisId).toBe("2");
    expect(state().job).toMatchObject({ jobId: "8", state: "queued", itemCount: 1 });
  });

  it("keeps cancellation correlated and drops events from another job", async () => {
    const ipc = fakeIpc({
      analyze: vi.fn().mockResolvedValue(analysis("1", "Post_1")),
      startDownload: vi.fn().mockResolvedValue({ jobId: "7" }),
      cancelDownload: vi.fn().mockResolvedValue(undefined),
    });
    const { controller, state } = harness(ipc);
    controller.changeUrl("https://x.com/i/web/status/500");
    await controller.analyze();
    await controller.startDownload("3");
    controller.receiveProgress({
      jobId: "8", state: "completed", itemIndex: 1, itemCount: 1,
      downloadedBytes: null, totalBytes: null, percent: null,
    });
    expect(state().job?.state).toBe("queued");
    await controller.cancelDownload();
    expect(state().job?.cancelRequested).toBe(true);
    await controller.cancelDownload();
    expect(ipc.cancelDownload).toHaveBeenCalledOnce();
  });
});
