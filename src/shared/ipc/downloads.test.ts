import { invoke } from "@tauri-apps/api/core";
import { listen, type Event, type UnlistenFn } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SelectionIntent } from "../contracts/selection";
import {
  cancelDownload,
  DownloadCommandError,
  getDownloadResult,
  listenDownloadProgress,
  openCompletedLocation,
  startDownload,
} from "./downloads";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const nativeInvoke = vi.mocked(invoke);
const nativeListen = vi.mocked(listen);
const intent: SelectionIntent = { mode: "best_available" };
const progress = {
  jobId: "7",
  state: "downloading",
  downloadedBytes: 25,
  totalBytes: 100,
  percent: 25,
  speedBytesPerSecond: 5,
  etaSeconds: 15,
  messageKey: "download.downloading",
} as const;

beforeEach(() => {
  nativeInvoke.mockReset();
  nativeListen.mockReset();
});

describe("download commands and events", () => {
  it("starts with only opaque IDs and a strict selection intent", async () => {
    nativeInvoke.mockResolvedValueOnce({ jobId: "7" });
    await expect(startDownload("1", "2", intent)).resolves.toEqual({ jobId: "7" });
    expect(nativeInvoke).toHaveBeenCalledExactlyOnceWith("start_download", {
      analysisId: "1",
      destinationId: "2",
      intent,
    });
  });

  it.each([
    ["", "2", intent],
    ["1", "0", intent],
    ["1", "2", { mode: "raw" }],
  ])("rejects invalid start input before IPC", async (analysisId, destinationId, candidate) => {
    await expect(startDownload(
      analysisId as string,
      destinationId as string,
      candidate as SelectionIntent,
    )).rejects.toBeInstanceOf(DownloadCommandError);
    expect(nativeInvoke).not.toHaveBeenCalled();
  });

  it.each([undefined, null, {}, { jobId: "0" }, { jobId: "7", path: "C:\\private" }])(
    "rejects malformed start response %j",
    async (response) => {
      nativeInvoke.mockResolvedValueOnce(response);
      await expect(startDownload("1", "2", intent)).rejects.toMatchObject({ code: "unknown" });
    },
  );

  it("cancels and opens by job ID without accepting a path", async () => {
    nativeInvoke.mockResolvedValueOnce(null).mockResolvedValueOnce(null);
    await cancelDownload("7");
    await openCompletedLocation("7");
    expect(nativeInvoke.mock.calls).toEqual([
      ["cancel_download", { jobId: "7" }],
      ["open_completed_location", { jobId: "7" }],
    ]);
  });

  it("fetches only a strict path-free terminal summary", async () => {
    const completed = {
      jobId: "7",
      state: "completed",
      errorCode: null,
      displayName: "安全文件名.mp4",
    };
    nativeInvoke.mockResolvedValueOnce(completed);
    await expect(getDownloadResult("7")).resolves.toEqual(completed);
    expect(nativeInvoke).toHaveBeenCalledExactlyOnceWith("get_download_result", { jobId: "7" });

    nativeInvoke.mockResolvedValueOnce({ ...completed, path: "C:\\private" });
    await expect(getDownloadResult("7")).rejects.toMatchObject({ code: "unknown" });
    nativeInvoke.mockResolvedValueOnce({ ...completed, displayName: "C:private.mp4" });
    await expect(getDownloadResult("7")).rejects.toMatchObject({ code: "unknown" });
  });

  it("listens only to the fixed event, validates payloads, and exposes synchronous cleanup", async () => {
    let nativeHandler: ((event: Event<unknown>) => void) | undefined;
    const unlisten = vi.fn<UnlistenFn>();
    nativeListen.mockImplementationOnce(async (eventName, handler) => {
      expect(eventName).toBe("download-progress");
      nativeHandler = handler as (event: Event<unknown>) => void;
      return unlisten;
    });
    const receiver = vi.fn();
    const cleanup = await listenDownloadProgress(receiver);
    nativeHandler?.({ event: "download-progress", id: 1, payload: progress } as Event<unknown>);
    expect(receiver).toHaveBeenCalledExactlyOnceWith(progress);
    cleanup();
    expect(unlisten).toHaveBeenCalledOnce();
  });

  it("rejects malformed or private event fields instead of forwarding them", async () => {
    let nativeHandler: ((event: Event<unknown>) => void) | undefined;
    nativeListen.mockImplementationOnce(async (_eventName, handler) => {
      nativeHandler = handler as (event: Event<unknown>) => void;
      const unlisten: UnlistenFn = () => undefined;
      return unlisten;
    });
    const receiver = vi.fn();
    await listenDownloadProgress(receiver);
    expect(() => nativeHandler?.({
      event: "download-progress",
      id: 1,
      payload: { ...progress, path: "C:\\private" },
    } as Event<unknown>)).toThrow(DownloadCommandError);
    expect(receiver).not.toHaveBeenCalled();
  });

  it("preserves only fixed native error codes and discards raw rejection details", async () => {
    nativeInvoke.mockRejectedValueOnce("download_in_progress");
    await expect(startDownload("1", "2", intent)).rejects.toEqual(
      new DownloadCommandError("download_in_progress"),
    );
    nativeInvoke.mockRejectedValueOnce(new Error("C:\\private signed URL"));
    const error = await cancelDownload("7").catch((value: unknown) => value);
    expect(error).toEqual(new DownloadCommandError("unknown"));
    expect(JSON.stringify(error)).not.toContain("private");
  });
});
