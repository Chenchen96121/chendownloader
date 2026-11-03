import { expect, it } from "vitest";
import { isDownloadQueueSnapshot } from "./downloadQueue";
const task = { jobId: "42", platform: "instagram", title: "帖子视频", state: "queued", percent: null, errorCode: null, displayName: null };
it("accepts distinct bounded cross-platform tasks, not private paths or malformed snapshots", () => {
  const snapshot = { limit: 2, tasks: [task, { ...task, jobId: "43", platform: "youtube", state: "downloading", percent: 35 }, { ...task, jobId: "44", platform: "x" },{...task,jobId:"45",platform:"bilibili"}] };
  expect(isDownloadQueueSnapshot(snapshot)).toBe(true);
  for (const bad of [
    { ...snapshot, limit: 0 }, { ...snapshot, limit: 5 },
    { ...snapshot, tasks: [task, task] }, { ...snapshot, tasks: Array.from({ length: 133 }, (_, n) => ({ ...task, jobId: String(n + 1) })) },
    { ...snapshot, tasks: [{ ...task, jobId: "01" }] }, { ...snapshot, tasks: [{ ...task, percent: NaN }] },
    { ...snapshot, tasks: [{ ...task, url: "https://cdn/private" }] }, { ...snapshot, tasks: [{ ...task, displayName: "C:\\private" }] },
    { ...snapshot, tasks: [{ ...task, state: "anything" }] }, { ...snapshot, tasks: [{ ...task, platform: "unknown" }] },
  ]) expect(isDownloadQueueSnapshot(bad)).toBe(false);
});
