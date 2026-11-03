import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { catalogs } from "../../shared/i18n/catalog";
import { DownloadControls } from "./DownloadControls";
import { createDownloadUiState } from "./downloadState";

const callbacks = {
  onChooseDestination: vi.fn(),
  onStart: vi.fn(),
  onCancel: vi.fn(),
  onRetry: vi.fn(),
  onOpenLocation: vi.fn(),
};

describe("download controls", () => {
  it("can namespace a second mounted platform's accessible download heading", () => {
    const html = renderToStaticMarkup(<DownloadControls locale="zh-CN" namespace="bilibili" state={createDownloadUiState("17")} canStart={false} {...callbacks} />);
    expect(html).toContain('id="bilibili-download-heading"');
    expect(html).toContain('aria-labelledby="bilibili-download-heading"');
    expect(html).not.toContain('id="download-heading"');
  });
  it.each(["zh-CN", "en-US"] as const)("renders localized destination and start controls in %s", (locale) => {
    const html = renderToStaticMarkup(<DownloadControls
      locale={locale}
      state={{ ...createDownloadUiState("7"), destination: { destinationId: "4", displayName: "Downloads" } }}
      canStart
      {...callbacks}
    />);
    expect(html).toContain(catalogs[locale].downloadHeading);
    expect(html).toContain("Downloads");
    expect(html).not.toContain(catalogs[locale].downloadChangeFolder);
    expect(html).not.toContain(catalogs[locale].downloadChooseFolder);
    expect(html).toContain(catalogs[locale].downloadStart);
    expect(html).not.toMatch(/destinationId|analysisId|https?:\/\/|C:\\|\/Users\//);
  });

  it("locks start while active and renders an accessible concise progress meter", () => {
    const state = {
      ...createDownloadUiState("7"),
      destination: { destinationId: "4", displayName: "下载" },
      phase: "downloading" as const,
      jobId: "8",
      downloadedBytes: 2_500_000,
      totalBytes: 10_000_000,
      percent: 25,
    };
    const html = renderToStaticMarkup(<DownloadControls locale="zh-CN" state={state} canStart={false} {...callbacks} />);
    expect(html).toContain('role="progressbar"');
    expect(html).toContain('aria-valuenow="25"');
    expect(html).toContain("2.50 MB / 10.00 MB");
    expect(html).toContain("25%");
    expect(html).toContain('type="button" disabled=""');
    expect(html).toContain(catalogs["zh-CN"].downloadCancel);
    expect(html).not.toMatch(/bytesPerSecond|etaSeconds|jobId|PRIVATE|https?:\/\//);
  });

  it("supports indeterminate processing, cancellation, safe completion, and open folder", () => {
    const processing = renderToStaticMarkup(<DownloadControls locale="en-US" state={{
      ...createDownloadUiState("7"), phase: "processing", jobId: "8",
    }} canStart={false} {...callbacks} />);
    expect(processing).toContain('role="progressbar"');
    expect(processing).not.toContain("aria-valuenow");
    expect(processing).toContain(catalogs["en-US"].downloadProcessing);

    const canceled = renderToStaticMarkup(<DownloadControls locale="en-US" state={{
      ...createDownloadUiState("7"), phase: "canceled", jobId: "8",
    }} canStart={false} {...callbacks} />);
    expect(canceled).toContain(catalogs["en-US"].downloadCanceled);
    expect(canceled).toContain(catalogs["en-US"].downloadRetry);

    const completed = renderToStaticMarkup(<DownloadControls locale="zh-CN" state={{
      ...createDownloadUiState("7"), phase: "completed", jobId: "8", displayName: "&lt;安全&gt;.mp4",
    }} canStart={false} {...callbacks} />);
    expect(completed).toContain("&amp;lt;安全&amp;gt;.mp4");
    expect(completed).toContain(catalogs["zh-CN"].downloadOpenFolder);
    expect(completed).not.toContain("<安全>");
  });

  it("shows retry only for retryable failures and never invokes callbacks during locale rendering", () => {
    const network = { ...createDownloadUiState("7"), phase: "failed" as const, errorCode: "network_failed" as const };
    const denied = { ...network, errorCode: "authentication_required" as const };
    const retryable = renderToStaticMarkup(<DownloadControls locale="zh-CN" state={network} canStart={false} {...callbacks} />);
    const final = renderToStaticMarkup(<DownloadControls locale="en-US" state={denied} canStart={false} {...callbacks} />);
    expect(retryable).toContain(catalogs["zh-CN"].downloadRetry);
    expect(final).not.toContain(catalogs["en-US"].downloadRetry);
    for (const callback of Object.values(callbacks)) expect(callback).not.toHaveBeenCalled();
  });
});
