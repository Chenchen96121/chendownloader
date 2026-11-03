import { describe, expect, it, vi } from "vitest";
import type { InstagramAnalysisPreview, InstagramPreviewPayload } from "../../shared/contracts/instagram";
import type { InstagramIpc } from "../../shared/ipc/instagram";
import {
  createInstagramWorkspaceController,
  type InstagramWorkspaceAccess,
} from "./useInstagramWorkspace";
import {
  buildInstagramDownloadIntent,
  createInstagramWorkspaceState,
  instagramWorkspaceReducer,
  type InstagramWorkspaceAction,
} from "./instagramState";

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
function analysis(analysisId: string, shortcode: string): InstagramAnalysisPreview {
  return {
    analysisId, sourceKind: "post", shortcode, title: shortcode,
    sourcePageUrl: `https://www.instagram.com/p/${shortcode}/`, capabilities,
    items: [{ itemId: analysisId, ordinal: 1, kind: "image", container: "jpg", previewAvailable: true, availability: "ready" }],
  };
}

function unavailableVideo(analysisId: string, shortcode: string): InstagramAnalysisPreview {
  return {
    ...analysis(analysisId, shortcode),
    items: [{
      itemId: analysisId, ordinal: 1, kind: "video", container: "mp4",
      previewAvailable: true, availability: "unavailable", unavailableReason: "analysis_unavailable",
    }],
  };
}

function harness(ipc: InstagramIpc) {
  let state = createInstagramWorkspaceState();
  const access: InstagramWorkspaceAccess = {
    getState: () => state,
    apply: (action: InstagramWorkspaceAction) => { state = instagramWorkspaceReducer(state, action); },
  };
  return { controller: createInstagramWorkspaceController(access, ipc), state: () => state };
}

function fakeIpc(overrides: Partial<InstagramIpc>): InstagramIpc {
  return {
    analyze: vi.fn(), getPreview: vi.fn(), openSource: vi.fn(), startDownload: vi.fn(),
    cancelDownload: vi.fn(), getDownloadResult: vi.fn(), openCompletedLocation: vi.fn(),
    listenProgress: vi.fn(async () => () => undefined), ...overrides,
  };
}

describe("Instagram workspace async controller", () => {
  it("loads an unavailable video's cover without enabling its selection or download", async () => {
    const ipc = fakeIpc({
      analyze: vi.fn().mockResolvedValue(unavailableVideo("1", "Post_1")),
      getPreview: vi.fn().mockResolvedValue({ mimeType: "image/jpeg", dataBase64: "AAAA" }),
    });
    const { controller, state } = harness(ipc);
    controller.changeUrl("https://www.instagram.com/p/Post_1/");
    await controller.analyze();
    await controller.loadPreview("1");
    expect(state().previews["1"]).toMatchObject({
      status: "ready", preview: { mimeType: "image/jpeg", dataBase64: "AAAA" },
    });
    controller.toggleSelection("1");
    controller.selectAll();
    expect(state().selectedItemIds).toEqual([]);
    expect(buildInstagramDownloadIntent(state())).toBeNull();
    expect(await controller.startDownload("3")).toBeNull();
    expect(state().job).toBeNull();
  });

  it("detaches a submitted native job so another post can be analyzed without canceling it", async () => {
    const ipc = fakeIpc({ analyze: vi.fn().mockResolvedValueOnce(analysis("1", "Post_1")).mockResolvedValueOnce(analysis("2", "Post_2")), startDownload: vi.fn().mockResolvedValue({ jobId: "7" }) });
    const { controller, state } = harness(ipc);
    controller.changeUrl("https://www.instagram.com/p/Post_1/"); await controller.analyze();
    await controller.startDownload("3"); controller.detachDownload("7");
    controller.changeUrl("https://www.instagram.com/p/Post_2/"); await controller.analyze();
    expect(state().analysis?.shortcode).toBe("Post_2"); expect(state().job).toBeNull();
    expect(ipc.cancelDownload).not.toHaveBeenCalled();
  });
  it("prevents an older analysis response from replacing a newer URL", async () => {
    const first = deferred<InstagramAnalysisPreview>();
    const second = deferred<InstagramAnalysisPreview>();
    const ipc = fakeIpc({ analyze: vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise) });
    const { controller, state } = harness(ipc);
    controller.changeUrl("https://www.instagram.com/p/Old_1/");
    const oldRequest = controller.analyze();
    controller.changeUrl("https://www.instagram.com/p/New_2/");
    const newRequest = controller.analyze();
    second.resolve(analysis("2", "New_2"));
    await newRequest;
    first.resolve(analysis("1", "Old_1"));
    await oldRequest;
    expect(state().analysis?.shortcode).toBe("New_2");
  });

  it("ignores an older cover response for the same unavailable video", async () => {
    const first = deferred<InstagramPreviewPayload>();
    const second = deferred<InstagramPreviewPayload>();
    const ipc = fakeIpc({
      analyze: vi.fn().mockResolvedValue(unavailableVideo("1", "Post_1")),
      getPreview: vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise),
    });
    const { controller, state } = harness(ipc);
    controller.changeUrl("https://www.instagram.com/p/Post_1/");
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
    const oldPreview = deferred<InstagramPreviewPayload>();
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
    controller.changeUrl("https://www.instagram.com/p/Post_1/");
    await controller.analyze();
    const pending = controller.loadPreview("1");
    controller.changeUrl("https://www.instagram.com/p/Post_2/");
    await controller.analyze();
    oldPreview.resolve({ mimeType: "image/jpeg", dataBase64: "AAAA" });
    await pending;
    expect(state().analysis?.shortcode).toBe("Post_2");
    expect(state().previews).toEqual({});
  });

  it("does not let a late start revive a reset workspace", async () => {
    const start = deferred<{ jobId: string }>();
    const ipc = fakeIpc({
      analyze: vi.fn().mockResolvedValue(analysis("1", "Post_1")),
      startDownload: vi.fn().mockReturnValue(start.promise),
    });
    const { controller, state } = harness(ipc);
    controller.changeUrl("https://www.instagram.com/p/Post_1/");
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
    controller.changeUrl("https://www.instagram.com/p/Post_1/");
    await controller.analyze();
    const olderStart = controller.startDownload("3");
    controller.reset();
    controller.changeUrl("https://www.instagram.com/p/Post_2/");
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
    controller.changeUrl("https://www.instagram.com/p/Post_1/");
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
