import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { App } from "../../app/App";
import { catalogs } from "../../shared/i18n/catalog";
import { SelectionDetails, SelectionSummary } from "./SelectionDetails";
import { SelectionPreviewView } from "./SelectionPreview";
import { createSelectionState } from "./selectionState";
import { sample } from "./selectionTestFixtures";

describe("concise bilingual selection rendering", () => {
  it("starts in Chinese with a nontechnical local-component gate", () => {
    const html = renderToStaticMarkup(<App />);
    expect(html).toContain("chendownloader");
    expect(html).toContain("正在准备本地组件");
    expect(html).toContain("重新校验并修复");
    expect(html).not.toContain('id="media-url"');
    expect(html).not.toContain('id="sample-platform"');
    expect(html).toContain("YouTube 视频解析");
    expect(html).not.toContain("正在检查本地引擎");
    expect(html).not.toMatch(/Phase|target|x86_64|App version|下载文件|<details/);
  });
  it.each(["zh-CN", "en-US"] as const)("renders safe title and common format information in %s", (locale) => {
    const html = renderToStaticMarkup(<SelectionDetails data={sample} outcome={sample.outcome} mode="best_available" locale={locale} />);
    expect(html).toContain("&lt;img src=x onerror=alert(1)&gt;");
    expect(html).not.toContain("<img");
    expect(html).toContain("1920 × 1080");
    expect(html).not.toMatch(/H\.264|AAC/);
    expect(html).not.toMatch(/hidden-|PRIVATE CREATOR|avc1.64001f|mp4a.40.2|59.94|HDR10|7654|48000|192 kb|FPS|声道|采样率|<details/);
    expect(html).not.toContain(catalogs[locale]["operation.merge"]);
    expect(html).not.toContain(catalogs[locale]["warning.container_unverified"]);
    expect(html).not.toContain(catalogs[locale].estimatedSizeLabel);
  });
  it("labels the output estimate separately from input sizes", () => {
    const outcome = { status: "selected" as const, plan: { compatibilityMode: false, containerSelection: "automatic" as const, videoProcessing: "copy" as const, outputVideoCodec: "h264",  mode: "best_available" as const, sourceFormatIds: ["hidden-c"], videoFormatId: "hidden-c", audioFormatId: "hidden-c", operation: "keep" as const, outputContainer: "mp4" as const, estimatedSizeBytes: 2097152, warnings: ["container_unverified" as const] } };
    const html = renderToStaticMarkup(<SelectionSummary data={sample} outcome={outcome} locale="zh-CN" />);
    expect(html).toContain("约 2.10 MB");
    expect(html).toContain("MP4");
    expect(html).not.toContain("保留原格式");
  });
  it("shows selected source properties without an exhaustive format list", () => {
    const html = renderToStaticMarkup(<SelectionSummary data={sample} outcome={sample.outcome} locale="zh-CN" />);
    expect(html).toContain("MP4"); expect(html).toContain("大小未知");
    expect(html).not.toMatch(/selected-sources|format-list|hidden-|avc1\.64001f|mp4a\.40/);
  });
  it.each(["format_unavailable", "invalid_selection", "invalid_clip", "clip_duration_unknown"] as const)("localizes unavailable %s", (code) => {
    const html = renderToStaticMarkup(<SelectionSummary data={sample} outcome={{ status: "unavailable", code }} locale="en-US" />);
    expect(html).toContain(catalogs["en-US"][`unavailable.${code}`]);
    expect(html).not.toContain(code);
  });
  it("explains unavailable video-only formats explicitly", () => {
    const html = renderToStaticMarkup(<SelectionSummary data={sample} outcome={{ status: "unavailable", code: "format_unavailable" }} locale="zh-CN" />);
    expect(html).toContain("当前模式没有可用格式，请调整选择。");
  });
  it.each(["metadata_incomplete", "container_unverified", "output_size_unknown", "clip_boundary_may_vary", "clip_transfer_may_exceed_range"] as const)("localizes warning %s", (warning) => {
    if (sample.outcome.status !== "selected") throw new Error("Expected selected fixture");
    const outcome = { ...sample.outcome, plan: { ...sample.outcome.plan, warnings: [warning] } };
    const html = renderToStaticMarkup(<SelectionDetails data={sample} outcome={outcome} mode="best_available" locale="en-US" />);
    expect(html).not.toContain(catalogs["en-US"][`warning.${warning}`]);
    expect(html).not.toContain(warning);
  });
  it.each(["idle", "loading", "ready", "error"] as const)("keeps provenance and accessible status during %s", (status) => {
    const state = { ...createSelectionState(), status, data: sample, outcome: status === "ready" ? sample.outcome : null, error: status === "error" ? "preview" as const : null };
    const html = renderToStaticMarkup(<SelectionPreviewView state={state} locale="zh-CN" onChange={() => {}} onPreview={() => {}} />);
    expect(html).toContain("本地样本预览，尚未连接真实平台");
    expect(html.match(/role="status"/g)).toHaveLength(1);
    expect(html).toContain('type="radio"');
    expect(html).toContain("最高质量");
    expect(html).toContain("仅视频");
    expect(html).toContain("仅音频");
    expect(html).toContain("手动选择");
  });
  it.each(["manual", "video_only"] as const)("uses resolution choices without expert audio fields in fixture %s", (mode) => {
    const state = { ...createSelectionState(), data: sample, mode, videoFormatId: "hidden-v" };
    const html = renderToStaticMarkup(<SelectionPreviewView state={state} locale="en-US" onChange={() => {}} onPreview={() => {}} />);
    expect(html).toContain('id="manual-video"');
    expect(html).not.toContain('id="manual-audio"');
    expect(html).not.toMatch(/H\.264|AAC|mp4a.40.2/);
    const combined = renderToStaticMarkup(<SelectionPreviewView state={{ ...state, videoFormatId: "hidden-c" }} locale="zh-CN" onChange={() => {}} onPreview={() => {}} />);
    expect(combined).not.toContain('id="manual-audio"');
    expect(combined).not.toContain("此格式已包含音频");
  });
  it("renders labeled strict clip inputs and a localized duration summary", () => {
    const state = { ...createSelectionState(), data: sample, clipEnabled: true, clipStart: "0:00:10", clipEnd: "0:00:30" };
    const html = renderToStaticMarkup(<SelectionPreviewView state={state} locale="zh-CN" onChange={() => {}} onPreview={() => {}} />);
    expect(html).toContain('for="clip-start"');
    expect(html).toContain('for="clip-end"');
    expect(html).toContain('maxLength="14"');
    expect(html).toContain("00:10–00:30 · 20 秒");
    expect(html).toContain("起止位置可能略有偏差");
  });
});
