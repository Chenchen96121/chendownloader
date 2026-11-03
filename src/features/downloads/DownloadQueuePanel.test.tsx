import { expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { DownloadQueuePanelView } from "./DownloadQueuePanel";
import type { DownloadQueueSnapshot } from "../../shared/contracts/downloadQueue";
const snapshot: DownloadQueueSnapshot = { limit: 2, tasks: [
  { jobId: "1", platform: "youtube", title: "Video A", state: "downloading", percent: 41, errorCode: null, displayName: null },
  { jobId: "2", platform: "instagram", title: "Post B", state: "queued", percent: null, errorCode: null, displayName: null },
  { jobId: "3", platform: "instagram", title: "Post C", state: "completed", percent: 100, errorCode: null, displayName: "Post C" },
  { jobId: "4", platform: "x", title: "X post D", state: "partial", percent: null, errorCode: null, displayName: "X post" },
  { jobId: "5", platform: "bilibili", title: "Bili P2", state: "downloading", percent: 10, errorCode: null, displayName: null },
  { jobId: "6", platform: "bilibili", title: "Bili done", state: "completed", percent: 100, errorCode: null, displayName: "Bili.mp4" },
] };
it.each(["zh-CN", "en-US"] as const)("renders separate global jobs and per-job actions in %s", (locale) => {
  const html = renderToStaticMarkup(<DownloadQueuePanelView locale={locale} snapshot={snapshot} failed={false} error={null} pending={new Set(["2"])} expanded onToggle={() => {}} onCancel={() => {}} onOpen={() => {}} />);
  for (const title of ["Video A", "Post B", "Post C", "X post D", "Bili P2", "Bili done"]) expect(html).toContain(title);
  expect(html).toContain('value="41"');
  expect(html.match(/class="queue-task-cancel"/g)).toHaveLength(3);
  expect(html.match(/class="queue-task-open"/g)).toHaveLength(3);
  expect(html).toContain('class="queue-task-platform">Bilibili</span>');
  expect(html).toContain('class="queue-task-platform">X</span>');
  expect(html).toContain(locale === "zh-CN" ? "排队中" : "Queued");
  expect(html).not.toMatch(/jobId|C:\\|signed_url/);
});
