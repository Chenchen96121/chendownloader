import { afterEach, describe, expect, it, vi } from "vitest";
import { catalogs } from "./catalog";
import { LOCALE_STORAGE_KEY, readLocale, writeLocale, type Locale } from "./locale";

afterEach(() => { vi.unstubAllGlobals(); });

describe("locale storage", () => {
  it.each([null, "", "en", "zh", "EN-US", "en-US ", "fr-FR", '{"locale":"en-US"}'])("defaults to Chinese for %j", (stored) => {
    const readKeys: string[] = [];
    expect(readLocale({ getItem: (key) => { readKeys.push(key); return stored; } })).toBe("zh-CN");
    expect(readKeys).toEqual(["local-media-downloader.locale"]);
  });
  it.each(["zh-CN", "en-US"] as const)("reads and writes only the exact key and closed value %s", (locale) => {
    const entries = new Map<string, string>();
    const storage = { getItem: (key: string) => entries.get(key) ?? null, setItem: (key: string, value: string) => { entries.set(key, value); } };
    writeLocale(locale, storage);
    expect([...entries]).toEqual([["local-media-downloader.locale", locale]]);
    expect(readLocale(storage)).toBe(locale);
    expect(LOCALE_STORAGE_KEY).toBe("local-media-downloader.locale");
  });
  it("defaults without accessing browser language", () => {
    let languageReads = 0;
    vi.stubGlobal("localStorage", { getItem: () => null });
    vi.stubGlobal("navigator", { get language() { languageReads += 1; return "en-US"; } });
    expect(readLocale()).toBe("zh-CN");
    expect(languageReads).toBe(0);
  });
  it("tolerates missing browser storage", () => {
    vi.stubGlobal("localStorage", undefined);
    expect(readLocale()).toBe("zh-CN");
    expect(() => writeLocale("en-US")).not.toThrow();
  });
  it("uses default browser storage without introducing other keys", () => {
    const writes: [string, string][] = [];
    vi.stubGlobal("localStorage", { getItem: () => "en-US", setItem: (key: string, value: string) => { writes.push([key, value]); } });
    expect(readLocale()).toBe("en-US");
    writeLocale("zh-CN");
    expect(writes).toEqual([["local-media-downloader.locale", "zh-CN"]]);
  });
  it("tolerates denied reads and writes", () => {
    expect(readLocale({ getItem: () => { throw new Error("Denied"); } })).toBe("zh-CN");
    expect(() => writeLocale("en-US", { setItem: () => { throw new Error("Quota"); } })).not.toThrow();
  });
  it("never persists runtime-invalid locale values", () => {
    const writes: unknown[] = [];
    writeLocale("fr-FR" as Locale, { setItem: (...args) => { writes.push(args); } });
    expect(writes).toEqual([]);
  });
});

describe("bilingual catalog", () => {
  const required = [
    "bilibiliUrlPlaceholder", "bilibiliScopeHint", "bilibiliPartLabel", "bilibiliCoverLabel",
    "bilibiliCoverLoading", "bilibiliCoverUnavailable", "bilibiliCoverRetry",
    "xUrlLabel",
    "xUrlPlaceholder",
    "xAnalyze",
    "xEmpty",
    "xMediaHeading",
    "xPostLabel",
    "xCreatorLabel",
    "xItemLabel",
    "xImageLabel",
    "xVideoLabel",
    "xLoadPreview",
    "xPreviewLoading",
    "xPreviewUnavailable",
    "xOpenOriginal",
    "xSelectionRange",
    "xSelectAll",
    "xClearSelection",
    "xSelectedCount",
    "xNoSelection",
    "xCompatibilityVideoHint",
    "xDownloadSummary",
    "xItemProgress",
    "xPartialSummary",
    "xCompletedSummary",
    "xFailedSummary",
    "xCanceledSummary",
    "xFailedItem",
    "xEventsUnavailable",
    "xAnimatedVideoLabel",
    "xDownloadAll",
    "xDownloadAvailable",
    "xDownloadSelected",
    "xUnavailableCount",
    "xOriginalCounts",
    "xSelectedSavedSummary",
    "xWholeCompletedSummary",
    "xContentChangedHint",
    "queueTitle", "queueEmpty", "queueReadFailed", "queueCollapse", "queueExpand", "queuePending", "queueLimit",
    "queueDetails", "queueResultLoading", "queueResultFailed",
    "concurrencyTitle", "concurrencyLabel", "concurrencyLoading", "concurrencyHint", "concurrencyFailed",
    "queueState.queued", "queueState.preparing", "queueState.downloading", "queueState.processing", "queueState.verifying", "queueState.publishing", "queueState.completed", "queueState.partial", "queueState.failed", "queueState.canceled",
    "pasteLink", "pasteError.empty", "pasteError.unsupported_content", "pasteError.too_long", "pasteError.invalid_text", "pasteError.unavailable",
    "clipTimelineStart", "clipTimelineEnd",
    "openSettings",
    "navigationLabel", "route.youtube", "route.instagram", "route.x", "route.bilibili", "comingSoon",
    "instagramUrlLabel", "instagramUrlPlaceholder", "instagramAnalyze", "instagramEmpty", "instagramMediaHeading", "instagramPostLabel", "instagramReelLabel", "instagramCreatorLabel", "instagramItemLabel", "instagramImageLabel", "instagramVideoLabel", "instagramLoadPreview", "instagramPreviewLoading", "instagramPreviewUnavailable", "instagramPlayReel", "instagramOpenOriginal", "instagramSelectionRange", "instagramSelectAll", "instagramClearSelection", "instagramSelectedCount", "instagramNoSelection", "instagramCompatibilityVideoHint", "instagramDownloadSummary", "instagramItemProgress", "instagramPartialSummary", "instagramCompletedSummary", "instagramFailedSummary", "instagramCanceledSummary", "instagramFailedItem", "instagramEventsUnavailable",
    "defaultDestinationTitle", "defaultDestinationHint", "defaultDestinationUnavailable", "defaultDestinationLoading", "highestResolutionLabel", "downloadOptionsLabel", "resolutionLoading", "resolutionFailed",
    "formatUnknown",
    "previewPlayer", "previewPlay", "previewStop", "previewOriginal", "previewFallback", "previewOpenFailed",
    "liveHeading", "liveNotice", "mediaUrlLabel", "mediaUrlPlaceholder", "analyzeMedia", "analysisIdle", "analysisLoading", "analysisReady", "analysisEmpty",
    "analysisActionEditUrl", "analysisActionUseYouTube", "analysisActionTryAnother", "analysisActionRetry", "analysisActionCheckTools", "analysisActionPublicMedia", "analysisActionUpdateTools", "analysisActionAdjustSelection",
    "analysisError.invalid_url", "analysisError.unsupported_platform", "analysisError.unsupported_media", "analysisError.invalid_metadata", "analysisError.format_unavailable", "analysisError.analysis_unavailable", "analysisError.analysis_expired",
    "analysisError.tool_missing", "analysisError.tool_integrity_failed", "analysisError.authentication_required", "analysisError.access_denied", "analysisError.content_unavailable", "analysisError.content_changed", "analysisError.rate_limited",
    "analysisError.extractor_outdated", "analysisError.network_failed", "analysisError.analysis_timeout", "analysisError.process_failed", "analysisError.unknown",
    "emptyPreview", "estimatedSizeLabel", "selectedSourcesLabel", "compatibleUnavailable", "clipCaveat", "clipFullMediaNotice", "secondsUnit",
    "productTitle", "sampleNotice", "settingsTitle", "languageLabel", "languageAriaLabel", "languageChinese", "languageEnglish",
    "sampleHeading", "platformLabel", "previewSample", "previewSelection", "sampleIdle", "sampleLoading", "sampleLoaded", "previewError",
    "selectionLoading", "selectionReady", "selectionIdle", "selectionModesLabel", "mode.best_available", "mode.video_only", "mode.audio_only", "mode.manual", "recommendationNote",
    "manualVideoLabel", "manualAudioLabel", "chooseVideo", "chooseAudio", "noVideoFormats", "noAudioFormats", "combinedAudioIncluded",
    "clipHeading", "clipEnabledLabel", "clipStartLabel", "clipEndLabel", "clipTimeHint", "clipUnavailable", "clipAudioOnlyUnavailable", "clipInvalidInput",
    "mediaLabel", "durationLabel", "durationUnknown", "formatsLabel", "formatLabel", "sizeLabel", "sizeUnknown", "sizeApproximate", "resolutionLabel", "resolutionUnknown", "codecUnknown",
    "selectionResultLabel", "outputFormatLabel", "clipRangeLabel", "fullDurationLabel", "engineReady", "engineNotReady", "engineLoading", "engineUnavailable",
    "componentsPreparing", "componentsReady", "componentsRepairing", "componentsFailed", "localComponentsTitle", "localComponentsDescription",
    "componentsLoading", "componentsUnavailable", "componentReady", "componentMissing", "componentNeedsRepair", "verifyAndRepair", "repairingComponents",
    "repairBusy", "repairDownloadInProgress", "repairFailed",
    "operation.keep", "operation.merge", "operation.extract_audio", "operation.extract_video", "stream.video_only", "stream.audio_only", "stream.combined",
    "warning.metadata_incomplete", "warning.container_unverified", "warning.output_size_unknown", "warning.clip_boundary_may_vary", "warning.clip_transfer_may_exceed_range", "warning.compatibility_may_reencode", "warning.hdr_to_sdr",
    "unavailable.format_unavailable", "unavailable.invalid_selection", "unavailable.invalid_clip", "unavailable.clip_duration_unknown", "unavailable.compatibility_unavailable",
    "downloadHeading", "downloadDestinationLabel", "downloadNoDestination", "downloadChooseFolder", "downloadChangeFolder", "downloadChoosingFolder", "downloadStart", "downloadReady", "downloadNeedsSelection",
    "downloadStarting", "downloadPreparing", "downloadDownloading", "downloadProcessing", "downloadVerifying", "downloadCanceling", "downloadCompleted", "downloadCompletedFileLabel",
    "downloadCanceled", "downloadFailed", "downloadFinalizing", "downloadCancel", "downloadRetry", "downloadOpenFolder", "downloadProgressLabel", "downloadTransferred",
    "downloadActionAnalyzeAgain", "downloadActionAdjustSelection", "downloadActionWait", "downloadActionCheckTools", "downloadActionPublicMedia", "downloadActionTryAnother", "downloadActionRetry", "downloadActionUpdateTools", "downloadActionChooseFolder",
    "downloadError.invalid_request", "downloadError.analysis_unavailable", "downloadError.destination_unavailable", "downloadError.format_unavailable", "downloadError.download_in_progress",
    "downloadError.tool_missing", "downloadError.tool_integrity_failed", "downloadError.authentication_required", "downloadError.access_denied", "downloadError.content_unavailable", "downloadError.content_changed",
    "downloadError.rate_limited", "downloadError.extractor_outdated", "downloadError.network_failed", "downloadError.download_timeout", "downloadError.process_failed", "downloadError.output_invalid",
    "downloadError.disk_full", "downloadError.publication_failed", "downloadError.unknown",
    "downloadError.image_content_invalid", "downloadError.image_dimensions_mismatch", "downloadError.image_size_mismatch", "downloadError.image_probe_failed",
    "compatibilityLabel", "compatibilityHint", "compatibilityTarget", "sourceDetailsLabel", "saveDetailsLabel", "sourceContainerLabel", "saveContainerLabel", "fpsLabel", "videoCodecLabel", "audioCodecLabel", "videoBitrateLabel", "audioBitrateLabel", "notApplicable", "noAudio",
  ];
  it("provides complete matching flat keys for visible, ARIA, status and failure copy", () => {
    expect(Object.keys(catalogs["zh-CN"]).sort()).toEqual([...required].sort());
    expect(Object.keys(catalogs["en-US"]).sort()).toEqual([...required].sort());
    for (const dictionary of Object.values(catalogs)) {
      expect(Object.values(dictionary).every((text) => typeof text === "string" && text.trim().length > 0)).toBe(true);
    }
  });
  it("preserves the product name in both languages and both native language names", () => {
    for (const dictionary of Object.values(catalogs)) {
      expect(dictionary.productTitle).toBe("chendownloader");
      expect(dictionary.languageChinese).toBe("简体中文");
      expect(dictionary.languageEnglish).toBe("English");
    }
  });
});
