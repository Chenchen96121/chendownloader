import { expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { InstagramTaskResultView } from "./InstagramTaskDetails";
const result = { jobId: "8", state: "partial" as const, completedCount: 1, failedCount: 1, folderDisplayName: "Post", items: [
  { itemId: "1", ordinal: 1, kind: "image" as const, state: "completed" as const, displayName: "01.jpg" },
  { itemId: "2", ordinal: 2, kind: "video" as const, state: "failed" as const, errorCode: "network_failed" as const },
] };
it.each(["zh-CN", "en-US"] as const)("shows retained per-item outcomes and reasons independently of current analysis in %s", locale => {
  const html = renderToStaticMarkup(<InstagramTaskResultView result={result} locale={locale} />);
  expect(html).toContain("01.jpg");
  expect(html).toContain(locale === "zh-CN" ? "1 项成功，1 项失败" : "1 completed, 1 failed");
  expect(html).toContain(locale === "zh-CN" ? "第 2 项" : "Item 2");
  expect(html).toContain(locale === "zh-CN" ? "网络连接失败" : "network connection failed");
  expect(html).not.toMatch(/jobId|itemId|analysisId|C:\\/);
});
