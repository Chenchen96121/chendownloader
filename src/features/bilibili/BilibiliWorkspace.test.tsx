import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import { catalogs } from "../../shared/i18n/catalog";
import { LiveAnalysisWorkspaceView } from "../analyze/LiveAnalysisWorkspace";
import { createLiveAnalysisState } from "../analyze/liveAnalysisState";
import { createDownloadUiState } from "../downloads/downloadState";
import { BilibiliWorkspaceView } from "./BilibiliWorkspace";
import { createBilibiliState } from "./bilibiliState";
import { biliSample } from "./bilibiliTestFixtures";

const callbacks = { onChange: () => {}, onAnalyze: () => {}, onOpenSource: () => {}, onRetryCover: () => {} };
const state = { ...createBilibiliState(), data: biliSample, outcome: biliSample.outcome, status: "ready" as const };
it("renders share text in a field without the browser's URL-only submission constraint", () => {
  const share = "【QWER '少年漫画' Official Special Clip】 https://www.bilibili.com/video/BV13Bab6QEir/?share_source=copy_web";
  const html = renderToStaticMarkup(<BilibiliWorkspaceView state={{ ...createBilibiliState(), url: share }} locale="zh-CN" {...callbacks} />);
  const input = html.match(/<input\b[^>]*\bid="bilibili-media-url"[^>]*>/)?.[0];
  expect(input).toBeDefined();
  expect(input).toContain('type="text"');
  expect(input).toContain('inputMode="url"');
  expect(input).toContain("Official Special Clip");
});
it.each(["zh-CN", "en-US"] as const)("renders safe independent Bili controls and current P in %s", locale => {
  const html = renderToStaticMarkup(<BilibiliWorkspaceView state={state} locale={locale} {...callbacks} />);
  expect(html).toContain('id="bilibili-media-url"');
  expect(html).toContain('name="bilibili-selection-mode"');
  expect(html).toContain("1920 × 1080");
  expect(html).toContain(locale === "zh-CN" ? "第 2 P" : "Part 2");
  for (const mode of ["best_available", "manual", "video_only", "audio_only"] as const) expect(html).toContain(catalogs[locale][`mode.${mode}`]);
  expect(html.indexOf('id="bilibili-compatibility"')).toBeGreaterThan(html.indexOf('value="audio_only"'));
  expect(html).not.toMatch(/id="bilibili-compatibility"[^>]*checked/);
  expect(html).toContain("Bilibili &lt;P2&gt;");
  expect(html).not.toMatch(/<iframe|bilibili-manual-audio|279787|bilivideo\.com|PRIVATE CREATOR/);
});
it.each(["manual", "video_only"] as const)("shows only concise resolution, honest size and feasible container choices in %s", mode => {
  const plans = [{ videoFormatId: "hidden-v", outputContainer: "mp4" as const, outcome: { status: "selected" as const, plan: { ...biliSample.outcome.status === "selected" ? biliSample.outcome.plan : {}, mode, outputContainer: "mp4" as const } } }];
  const html = renderToStaticMarkup(<BilibiliWorkspaceView state={{ ...state, mode, videoFormatId: "hidden-v", outputContainer: "mp4", clipEnabled: true, clipStart: "0:00:10", clipEnd: "0:00:30" }} locale="zh-CN" resolutionPlans={plans as never} {...callbacks} />);
  expect(html).toContain('id="bilibili-manual-video"');
  expect(html).toContain("1920 × 1080 · 大小未知 · MP4");
  expect(html).not.toContain("WEBM</option>");
  expect(html).toContain('id="bilibili-clip-start"'); expect(html).toContain('aria-valuetext="0:00:10"');
  expect(html).toContain('aria-label="片段结束位置"');
});
it("keeps cover errors separate from selection and uses a local safe image payload without embeds", () => {
  const failed = renderToStaticMarkup(<BilibiliWorkspaceView state={state} locale="zh-CN" cover={{ analysisId: "17", status: "error", payload: null }} {...callbacks} />);
  expect(failed).toContain("重试封面"); expect(failed).toContain("1920 × 1080"); expect(failed).not.toContain("H.264"); expect(failed).not.toContain("发生未知的解析错误");
  const ready = renderToStaticMarkup(<BilibiliWorkspaceView state={state} locale="en-US" cover={{ analysisId: "17", status: "ready", payload: { mimeType: "image/png", dataBase64: "iVBORw0KGgo=" } }} {...callbacks} />);
  expect(ready).toContain('src="data:image/png;base64,iVBORw0KGgo="');
  expect(ready).toContain("Open original page"); expect(ready).not.toContain("<iframe");
});
it("namespaces all mounted Bili IDs and radio names independently from YouTube", () => {
  const download = { state: createDownloadUiState("17"), canStart: false, onChooseDestination: () => {}, onStart: () => {}, onCancel: () => {}, onRetry: () => {}, onOpenLocation: () => {} };
  const html = renderToStaticMarkup(<><LiveAnalysisWorkspaceView state={{ ...createLiveAnalysisState(), data: biliSample }} locale="zh-CN" onChange={() => {}} onAnalyze={() => {}} onPreview={() => {}} download={download} /><BilibiliWorkspaceView state={state} locale="zh-CN" download={download} {...callbacks} /></>);
  const ids = [...html.matchAll(/\sid="([^"]+)"/g)].map(row => row[1]);
  expect(new Set(ids).size).toBe(ids.length);
  expect(html).toContain('aria-labelledby="bilibili-download-heading"');
  const audio = renderToStaticMarkup(<BilibiliWorkspaceView state={{ ...state, mode: "audio_only", outcome: null }} locale="zh-CN" {...callbacks} />);
  expect(audio).not.toContain("<select"); expect(audio).toContain("仅音频模式不支持片段");
});
