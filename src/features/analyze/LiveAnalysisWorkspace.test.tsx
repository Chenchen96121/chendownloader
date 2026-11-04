import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { App } from "../../app/App";
import type { LiveAnalysisPreview } from "../../shared/contracts/liveAnalysis";
import { catalogs } from "../../shared/i18n/catalog";
import { LiveAnalysisWorkspaceView } from "./LiveAnalysisWorkspace";
import { createLiveAnalysisState } from "./liveAnalysisState";
import { sample } from "./selectionTestFixtures";
import { createDownloadUiState } from "../downloads/downloadState";

const liveSample: LiveAnalysisPreview = { analysisId: "7", ...sample };

it("does not describe invalid extractor metadata as inaccessible public content", () => {
  const state={...createLiveAnalysisState(),status:"error" as const,error:"invalid_metadata" as const};
  const html=renderToStaticMarkup(<LiveAnalysisWorkspaceView locale="zh-CN" state={state} onChange={()=>{}} onAnalyze={()=>{}} onPreview={()=>{}} />);
  expect(html).toContain(catalogs["zh-CN"].analysisActionRetry);
  expect(html).not.toContain(catalogs["zh-CN"].analysisActionTryAnother);
});

it.each([["zh-CN", "一键粘贴"], ["en-US", "Paste"]] as const)("renders user-triggered paste inside the URL field in %s", (locale, label) => {
  const state = createLiveAnalysisState();
  const props = {state, locale, onChange: () => {}, onAnalyze: () => {}, onPreview: () => {}};
  const html = renderToStaticMarkup(<LiveAnalysisWorkspaceView {...props} />);
  expect(html).toContain(`type="button" aria-label="${label}"`);
  expect(html).toContain("url-input-wrap");
  const locked = renderToStaticMarkup(<LiveAnalysisWorkspaceView {...props} inputLocked />);
  expect(locked).toMatch(new RegExp(`type="button" aria-label="${label}"[^>]*disabled=""`));
});

describe("live URL analysis workspace", () => {
  it("places an opt-in compatibility checkbox below the four modes and locks it during a task", () => {
    const state={...createLiveAnalysisState(),data:liveSample};
    const props={state,locale:"zh-CN" as const,onChange:()=>{},onAnalyze:()=>{},onPreview:()=>{}};
    const html=renderToStaticMarkup(<LiveAnalysisWorkspaceView {...props}/>);
    expect(html).toContain('id="live-compatibility"');
    expect(html).toContain("正常下载出现无法播放或无法解码等情况可选择");
    expect(html.indexOf('id="live-compatibility"')).toBeGreaterThan(html.indexOf('value="audio_only"'));
    expect(html).not.toMatch(/id="live-compatibility"[^>]*checked/);
    const locked=renderToStaticMarkup(<LiveAnalysisWorkspaceView {...props} inputLocked/>);
    expect(locked).toMatch(/id="live-compatibility"[^>]*disabled/);
  });
  it("locks source options while a repeated analysis is pending", () => {
    const state = { ...createLiveAnalysisState(), data: liveSample, mode: "manual" as const, status: "analyzing" as const, videoFormatId: "hidden-v", clipEnabled: true, clipStart: "0:00:10", clipEnd: "0:00:30" };
    const html = renderToStaticMarkup(<LiveAnalysisWorkspaceView state={state} locale="zh-CN" onChange={() => {}} onAnalyze={() => {}} onPreview={() => {}} />);
    expect(html).toMatch(/<input[^>]*disabled=""[^>]*name="live-selection-mode"/);
    expect(html).toContain('<select id="live-manual-video" disabled=""');
    expect(html).toContain('id="live-clip-start" type="text" inputMode="text" autoComplete="off" spellCheck="false" maxLength="14" disabled=""');
  });
  it.each(["manual", "video_only"] as const)("renders concise native resolution choices and a synchronized timeline in %s", (mode) => {
    const state = { ...createLiveAnalysisState(), data: liveSample, mode, videoFormatId: "hidden-v", outputContainer: "mp4" as const, clipEnabled: true, clipStart: "0:00:10", clipEnd: "0:00:30" };
    const plans = [{ videoFormatId: "hidden-v", outputContainer: "mp4" as const, outcome: {status: "selected" as const, plan: {compatibilityMode: false, containerSelection: "explicit" as const, videoProcessing: "copy" as const, outputVideoCodec: "h264", mode, videoFormatId: "hidden-v", sourceFormatIds: ["hidden-v"], operation: "keep" as const, outputContainer: "mp4" as const, warnings: []}} }];
    const html = renderToStaticMarkup(<LiveAnalysisWorkspaceView state={state} locale="zh-CN" resolutionPlans={plans} onChange={() => {}} onAnalyze={() => {}} onPreview={() => {}} />);
    expect(html).toContain('<option value="[&quot;hidden-v&quot;,&quot;mp4&quot;]" selected="">1920 × 1080 · 大小未知 · MP4</option>');
    expect(html).toContain('aria-label="片段开始位置"');
    expect(html).toContain('aria-valuetext="0:00:10"');
    expect(html).not.toMatch(/avc1\.64001f|mp4a\.40/);
  });
  it("starts behind the Chinese local-component gate with language settings available", () => {
    const html = renderToStaticMarkup(<App />);
    expect(html).toContain("chendownloader");
    expect(html).toContain("正在准备本地组件…");
    expect(html).toContain('aria-label="选择界面语言"');
    expect(html).not.toContain('id="media-url"');
    expect(html).not.toMatch(/本地样本预览|样本平台|sample-platform|preview_media_selection_fixture/);
  });

  it("locks URL submission while analyzing and uses form submission for Enter", () => {
    const state = { ...createLiveAnalysisState(), url: "https://youtube.com/watch?v=x", status: "analyzing" as const };
    const html = renderToStaticMarkup(<LiveAnalysisWorkspaceView state={state} locale="zh-CN" onChange={() => {}} onAnalyze={() => {}} onPreview={() => {}} />);
    expect(html).toContain("<form");
    expect(html).toContain('type="submit" disabled=""');
    expect(html).toContain('id="media-url" type="url"');
    expect(html).toContain('disabled=""');
    expect(html).toContain("正在解析媒体");
  });

  it("uses distinct success copy for analysis and selection preview", () => {
    const analyzed = { ...createLiveAnalysisState(), data: liveSample, outcome: liveSample.outcome, status: "analyzed" as const };
    const selected = { ...analyzed, status: "ready" as const };
    const props = { locale: "zh-CN" as const, onChange: () => {}, onAnalyze: () => {}, onPreview: () => {} };
    expect(renderToStaticMarkup(<LiveAnalysisWorkspaceView state={analyzed} {...props} />)).toContain("媒体已解析，可以选择格式");
    expect(renderToStaticMarkup(<LiveAnalysisWorkspaceView state={selected} {...props} />)).toContain("选择方案预览已更新");
  });

  it.each(["zh-CN", "en-US"] as const)("shows concise safe media details and four modes in %s", (locale) => {
    const state = { ...createLiveAnalysisState(), url: "https://youtube.com/watch?v=x", data: liveSample, outcome: liveSample.outcome, status: "ready" as const };
    const html = renderToStaticMarkup(<LiveAnalysisWorkspaceView state={state} locale={locale} onChange={() => {}} onAnalyze={() => {}} onPreview={() => {}} />);
    expect(html).toContain("&lt;img src=x onerror=alert(1)&gt;");
    expect(html).not.toContain("<img");
    expect(html).toContain("1920 × 1080");
    expect(html).toContain("59.94 FPS");
    expect(html).not.toContain("H.264"); expect(html).not.toContain("AAC");
    for (const mode of ["best_available", "video_only", "audio_only", "manual"] as const) {
      expect(html).toContain(catalogs[locale][`mode.${mode}`]);
    }
    expect(html).not.toMatch(/hidden-|PRIVATE CREATOR|48000|analysisId|yt-dlp|ffmpeg|C:\\|PRIVATE native/);
  });

  it("automatically pairs audio without expert controls and keeps the clip caveat", () => {
    const state = { ...createLiveAnalysisState(), url: "https://youtube.com/watch?v=x", data: liveSample, outcome: null, status: "idle" as const, mode: "manual" as const, videoFormatId: "hidden-v", clipEnabled: true, clipStart: "0:00:10", clipEnd: "0:00:30" };
    const html = renderToStaticMarkup(<LiveAnalysisWorkspaceView state={state} locale="zh-CN" onChange={() => {}} onAnalyze={() => {}} onPreview={() => {}} />);
    expect(html).not.toContain('id="live-manual-audio"');
    expect(html).toContain('id="live-manual-video"');
    expect(html).toContain("起止位置可能略有偏差");
    expect(html).not.toContain("hidden-");
    const combined = renderToStaticMarkup(<LiveAnalysisWorkspaceView state={{ ...state, videoFormatId: "hidden-c" }} locale="zh-CN" onChange={() => {}} onAnalyze={() => {}} onPreview={() => {}} />);
    expect(combined).not.toContain('id="live-manual-audio"');
    expect(combined).not.toContain("此格式已包含音频");
  });

  it.each([
    ["zh-CN", "片段下载可能需要先获取完整媒体。"],
    ["en-US", "A clip may require downloading the full media first."],
  ] as const)("shows the concise full-media clip notice only for a valid enabled clip in %s", (locale, notice) => {
    const state = {
      ...createLiveAnalysisState(),
      url: "https://youtube.com/watch?v=x",
      data: liveSample,
      outcome: liveSample.outcome,
      status: "ready" as const,
      clipEnabled: true,
      clipStart: "0:00:10",
      clipEnd: "0:00:30",
    };
    const props = { locale, onChange: () => {}, onAnalyze: () => {}, onPreview: () => {} };

    expect(renderToStaticMarkup(<LiveAnalysisWorkspaceView state={state} {...props} />)).toContain(notice);
    expect(renderToStaticMarkup(<LiveAnalysisWorkspaceView state={{ ...state, clipEnabled: false }} {...props} />)).not.toContain(notice);
    expect(renderToStaticMarkup(<LiveAnalysisWorkspaceView state={{ ...state, mode: "audio_only" as const }} {...props} />)).not.toContain(notice);
    expect(renderToStaticMarkup(<LiveAnalysisWorkspaceView state={{ ...state, clipStart: "bad" }} {...props} />)).not.toContain(notice);
    expect(renderToStaticMarkup(<LiveAnalysisWorkspaceView state={{ ...state, clipStart: "0:00:40", clipEnd: "0:00:30" }} {...props} />)).not.toContain(notice);
  });

  it("switches clip notice locale without restarting analysis, destination, or download state", () => {
    const state = {
      ...createLiveAnalysisState(),
      url: "https://youtube.com/watch?v=x",
      data: liveSample,
      outcome: liveSample.outcome,
      status: "ready" as const,
      clipEnabled: true,
      clipStart: "0:00:10",
      clipEnd: "0:00:30",
    };
    const onChange = vi.fn();
    const onAnalyze = vi.fn();
    const onPreview = vi.fn();
    const onChooseDestination = vi.fn();
    const onStart = vi.fn();
    const onCancel = vi.fn();
    const onRetry = vi.fn();
    const onOpenLocation = vi.fn();
    const download = {
      state: {
        ...createDownloadUiState("7"),
        destination: { destinationId: "4", displayName: "Downloads" },
        phase: "completed" as const,
        jobId: "8",
        displayName: "clip.mp4",
      },
      canStart: false,
      onChooseDestination,
      onStart,
      onCancel,
      onRetry,
      onOpenLocation,
    };

    const chinese = renderToStaticMarkup(<LiveAnalysisWorkspaceView state={state} locale="zh-CN" download={download} onChange={onChange} onAnalyze={onAnalyze} onPreview={onPreview} />);
    const english = renderToStaticMarkup(<LiveAnalysisWorkspaceView state={state} locale="en-US" download={download} onChange={onChange} onAnalyze={onAnalyze} onPreview={onPreview} />);

    expect(chinese).toContain("片段下载可能需要先获取完整媒体。");
    expect(english).toContain("A clip may require downloading the full media first.");
    for (const html of [chinese, english]) {
      expect(html).toContain("Downloads");
      expect(html).toContain("clip.mp4");
    }
    for (const callback of [onChange, onAnalyze, onPreview, onChooseDestination, onStart, onCancel, onRetry, onOpenLocation]) {
      expect(callback).not.toHaveBeenCalled();
    }
  });

  it.each([
    ["invalid_url", "请输入有效的媒体链接"],
    ["tool_missing", "请先完成本地工具准备"],
    ["authentication_required", "平台要求本次请求登录"],
    ["rate_limited", "稍后手动重试"],
    ["extractor_outdated", "请更新本地解析组件"],
  ] as const)("localizes stable %s errors with an action", (error, message) => {
    const state = { ...createLiveAnalysisState(), url: "https://youtube.com/watch?v=x", status: "error" as const, error };
    const html = renderToStaticMarkup(<LiveAnalysisWorkspaceView state={state} locale="zh-CN" onChange={() => {}} onAnalyze={() => {}} onPreview={() => {}} />);
    expect(html).toContain(message);
    expect(html).not.toContain(error);
  });

  it("changing locale only changes copy and never invokes an operation", () => {
    const onAnalyze = vi.fn();
    const onPreview = vi.fn();
    const state = { ...createLiveAnalysisState(), url: "https://youtube.com/watch?v=x", data: liveSample, outcome: liveSample.outcome, status: "ready" as const };
    renderToStaticMarkup(<LiveAnalysisWorkspaceView state={state} locale="zh-CN" onChange={() => {}} onAnalyze={onAnalyze} onPreview={onPreview} />);
    renderToStaticMarkup(<LiveAnalysisWorkspaceView state={state} locale="en-US" onChange={() => {}} onAnalyze={onAnalyze} onPreview={onPreview} />);
    expect(onAnalyze).not.toHaveBeenCalled();
    expect(onPreview).not.toHaveBeenCalled();
  });

  it("locks analysis inputs during a download and embeds path-free download controls", () => {
    const state = { ...createLiveAnalysisState(), url: "https://youtube.com/watch?v=x", data: liveSample, outcome: liveSample.outcome, status: "ready" as const };
    const download = {
      state: {
        ...createDownloadUiState("7"),
        destination: { destinationId: "4", displayName: "Downloads" },
        phase: "verifying" as const,
        jobId: "8",
      },
      canStart: false,
      onChooseDestination: vi.fn(),
      onStart: vi.fn(),
      onCancel: vi.fn(),
      onRetry: vi.fn(),
      onOpenLocation: vi.fn(),
    };
    const html = renderToStaticMarkup(<LiveAnalysisWorkspaceView
      state={state}
      locale="zh-CN"
      inputLocked
      download={download}
      onChange={() => {}}
      onAnalyze={() => {}}
      onPreview={() => {}}
    />);
    expect(html).toContain("正在验证文件");
    expect(html).toContain("Downloads");
    expect(html).toContain('id="media-url" type="url"');
    expect(html.match(/disabled=""/g)?.length).toBeGreaterThan(4);
    expect(html).not.toMatch(/destinationId|jobId|analysisId|C:\\/);
  });
});
