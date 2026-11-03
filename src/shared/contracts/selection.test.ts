import { describe, expect, it } from "vitest";
import { isSelectionIntent, isSelectionPreview, type SelectionIntent } from "./selection";

const video = { id: "v1", container: "mp4", kind: "video_only", videoCodec: "avc1" };
const audio = { id: "a1", container: "m4a", kind: "audio_only", audioCodec: "aac" };
const combined = { id: "c1", container: "mp4", kind: "combined", videoCodec: "avc1", audioCodec: "aac" };
const media = { platform: "youtube", mediaId: "sample", title: "样本", durationSeconds: 60, formats: [video, audio, combined] };
const intent: SelectionIntent = { mode: "best_available" };
const plan = { compatibilityMode: false, containerSelection: "automatic" as const, videoProcessing: "copy" as const, outputVideoCodec: "h264", outputAudioCodec: "aac", audioProcessing: "copy", mode: "best_available", sourceFormatIds: ["v1", "a1"], videoFormatId: "v1", audioFormatId: "a1", operation: "merge", outputContainer: "mp4", warnings: ["metadata_incomplete", "container_unverified", "output_size_unknown"] };
const preview = { media, rankedVideoFormatIds: ["v1", "c1"], rankedAudioFormatIds: ["a1", "c1"], outcome: { status: "selected", plan } };
const checkPlan = (change: object, request = intent) => isSelectionPreview({ ...preview, outcome: { status: "selected", plan: { ...plan, ...change } } }, "youtube", request);

describe("fidelity authorization", () => {
  const native = { ...plan, compatibilityMode: false, containerSelection: "automatic" as const, videoProcessing: "copy" as const, outputVideoCodec: "h264" };
  const check = (selection: object, request: SelectionIntent = intent, source = media) => isSelectionPreview({ ...preview, media: source, outcome: { status: "selected", plan: selection } }, "youtube", request);
  it("accepts real booleans only and defaults omitted compatibility to off", () => {
    expect(isSelectionIntent({ mode: "best_available", compatibilityMode: false })).toBe(true);
    expect(isSelectionIntent({ mode: "best_available", compatibilityMode: true })).toBe(true);
    for (const compatibilityMode of ["true", 1, null, undefined]) expect(isSelectionIntent({ mode: "best_available", compatibilityMode })).toBe(false);
    expect(check(native)).toBe(true);
    expect(check({ ...native, audioProcessing: "transcode" })).toBe(false);
    expect(check({ ...native, videoProcessing: "transcode" })).toBe(false);
  });
  it("copies original Opus to MP4 without changing the selected source", () => {
    const source = { ...media, formats: [video, { ...audio, audioCodec: "opus" }, combined] };
    expect(check({ ...native, outputAudioCodec: "opus" }, intent, source)).toBe(true);
    expect(check({ ...native, audioProcessing: "transcode" }, intent, source)).toBe(false);
  });
  it("maps a distinct combined audio source without counting unused video bytes", () => {
    const source = { ...media, formats: [video, audio, { ...combined, audioCodec: "flac" }] };
    expect(check({ ...native, audioFormatId: "c1", sourceFormatIds: ["v1", "c1"], outputAudioCodec: "flac" }, intent, source)).toBe(true);
  });
  it.each([["opus", "webm"], ["vorbis", "webm"], ["alac", "m4a"], ["flac", "flac"], ["mp3", "mp3"]])("preserves %s in audio-only %s", (codec, container) => {
    const source = { ...media, formats: [video, { ...audio, audioCodec: codec }, combined] };
    const selection = { compatibilityMode: false, containerSelection: "automatic" as const, mode: "audio_only", sourceFormatIds: ["a1"], audioFormatId: "a1", operation: "keep", outputContainer: container, outputAudioCodec: codec, audioProcessing: "copy", warnings: plan.warnings };
    expect(check(selection, { mode: "audio_only" }, source)).toBe(true);
  });
  it("accepts an explicitly authorized conversion and rejects mismatched consent", () => {
    const selection = { ...native, compatibilityMode: true, containerSelection: "compatibility", videoProcessing: "transcode", warnings: [...plan.warnings, "compatibility_may_reencode"] };
    expect(check(selection, { mode: "best_available", compatibilityMode: true })).toBe(true);
    expect(check(selection)).toBe(false);
  });
  it("estimates a whole audio source but not unused roles in a combined source", () => {
    const source = { ...media, formats: [video, { ...audio, fileSize: { bytes: 100, isApproximate: false } }, combined] };
    const selection = { compatibilityMode: false, containerSelection: "automatic", mode: "audio_only", sourceFormatIds: ["a1"], audioFormatId: "a1", operation: "keep", outputContainer: "m4a", outputAudioCodec: "aac", audioProcessing: "copy", estimatedSizeBytes: 100, warnings: ["metadata_incomplete", "container_unverified"] };
    expect(check(selection, { mode: "audio_only" }, source)).toBe(true);
    expect(check({ ...selection, estimatedSizeBytes: 99 }, { mode: "audio_only" }, source)).toBe(false);
  });
});

it("accepts only a video target container and rejects former manual audio overrides", () => {
  expect(isSelectionIntent({mode: "manual", videoFormatId: "v1", outputContainer: "webm"})).toBe(true);
  expect(isSelectionIntent({mode: "manual", videoFormatId: "v1", audioFormatId: "a1"})).toBe(false);
  expect(isSelectionIntent({mode: "best_available", outputContainer: "mp4"})).toBe(false);
});

describe("isSelectionIntent", () => {
  it("allows a selected video-only resolution but never an audio reference", () => {
    expect(isSelectionIntent({ mode: "video_only", videoFormatId: "v1" })).toBe(true);
    expect(isSelectionIntent({ mode: "video_only", videoFormatId: "v1", audioFormatId: "a1" })).toBe(false);
  });
  it.each(["best_available", "video_only", "audio_only", "manual"])("accepts closed mode %s", (mode) => {
    expect(isSelectionIntent({ mode })).toBe(true);
  });
  it("accepts manual IDs and an exact bounded clip", () => {
    expect(isSelectionIntent({ mode: "manual", videoFormatId: "v1", outputContainer: "mp4", clip: { startMs: 0, endMs: 31_536_000_000 } })).toBe(true);
  });
  it.each([
    null, [], {}, { mode: "best" }, { mode: "manual", extra: true },
    { mode: "manual", videoFormatId: "https://x" }, { mode: "manual", audioFormatId: "" },
    { mode: "manual", videoFormatId: "a".repeat(65) }, { mode: "manual", videoFormatId: "v1\n" },
    { mode: "manual", audioFormatId: undefined }, { mode: "manual", videoFormatId: null },
    { mode: "best_available", videoFormatId: "v1" }, { mode: "audio_only", audioFormatId: "a1" },
    { mode: "audio_only", clip: { startMs: 0, endMs: 1000 } },
  ])("rejects invalid request %j", (request) => expect(isSelectionIntent(request)).toBe(false));
  it.each([
    null, {}, { startMs: 0 }, { startMs: -1, endMs: 10 }, { startMs: 10, endMs: 10 },
    { startMs: 11, endMs: 10 }, { startMs: 0.5, endMs: 10 }, { startMs: 0, endMs: 10.5 },
    { startMs: 0, endMs: 31_536_000_001 }, { startMs: 0, endMs: Infinity },
    { startMs: NaN, endMs: 10 }, { startMs: 0, endMs: Number.MAX_SAFE_INTEGER + 1 },
    { startMs: 0, endMs: 10, raw: true },
  ])("rejects invalid clip %j", (clip) => expect(isSelectionIntent({ mode: "best_available", clip })).toBe(false));
});

describe("isSelectionPreview", () => {
  it("accepts video-only plans and rejects audio or changed selected resolution", () => {
    const value = { ...preview, outcome: { status: "selected", plan: { compatibilityMode: false, containerSelection: "automatic" as const, videoProcessing: "copy" as const, outputVideoCodec: "h264",
      mode: "video_only", sourceFormatIds: ["v1"], videoFormatId: "v1",
      operation: "keep", outputContainer: "mp4", warnings: plan.warnings,
    } } };
    const request = { mode: "video_only", videoFormatId: "v1" } as SelectionIntent;
    expect(isSelectionPreview(value, "youtube", request)).toBe(true);
    expect(isSelectionPreview({ ...value, outcome: { status: "selected", plan: { ...value.outcome.plan, audioFormatId: "a1" } } }, "youtube", request)).toBe(false);
    expect(isSelectionPreview(value, "youtube", { ...request, videoFormatId: "c1" })).toBe(false);
  });
  it("accepts stripping audio from a combined source without reporting its source size as output size", () => {
    const value = { ...preview, outcome: { status: "selected", plan: { compatibilityMode: false, containerSelection: "automatic" as const, videoProcessing: "copy" as const, outputVideoCodec: "h264",
      mode: "video_only", sourceFormatIds: ["c1"], videoFormatId: "c1",
      operation: "extract_video", outputContainer: "mp4", warnings: plan.warnings,
    } } };
    expect(isSelectionPreview(value, "youtube", { mode: "video_only" } as SelectionIntent)).toBe(true);
    expect(isSelectionPreview({ ...value, outcome: { status: "selected", plan: { ...value.outcome.plan, estimatedSizeBytes: 100 } } }, "youtube", { mode: "video_only" } as SelectionIntent)).toBe(false);
  });
  it("accepts a literal correlated merge without recomputing ranking", () => {
    expect(isSelectionPreview(preview, "youtube", intent)).toBe(true);
    expect(isSelectionPreview({ ...preview, rankedVideoFormatIds: ["c1", "v1"], rankedAudioFormatIds: ["c1", "a1"] }, "youtube", intent)).toBe(true);
  });
  it("accepts combined keep with one deduplicated source", () => {
    expect(checkPlan({ sourceFormatIds: ["c1"], videoFormatId: "c1", audioFormatId: "c1", operation: "keep" })).toBe(true);
  });
  it.each(["combined", "audio_only"] as const)("requires the original container for %s keep", (kind) => {
    const kept = kind === "combined"
      ? { compatibilityMode: false, containerSelection: "automatic" as const, videoProcessing: "copy" as const, outputVideoCodec: "h264",  mode: "best_available", sourceFormatIds: ["c1"], videoFormatId: "c1", audioFormatId: "c1", operation: "keep", outputAudioCodec: "aac", audioProcessing: "copy", warnings: plan.warnings }
      : { compatibilityMode: false, containerSelection: "automatic" as const,  mode: "audio_only", sourceFormatIds: ["a1"], audioFormatId: "a1", operation: "keep", outputAudioCodec: "aac", audioProcessing: "copy", warnings: plan.warnings };
    const request: SelectionIntent = { mode: kind === "combined" ? "best_available" : "audio_only" };
    const check = (fields: object) => isSelectionPreview({ ...preview, outcome: { status: "selected", plan: { ...kept, ...fields } } }, "youtube", request);
    expect(check({ outputContainer: kind === "combined" ? "mp4" : "m4a" })).toBe(true);
    expect(check({ outputContainer: "webm" })).toBe(false);
    expect(check({})).toBe(false);
  });
  it("rejects a sparse ranked array instead of treating its hole as a valid ID", () => {
    const value = { ...preview, media: { ...media, formats: [combined] }, rankedVideoFormatIds: Array(1), rankedAudioFormatIds: ["c1"], outcome: { status: "unavailable", code: "format_unavailable" } };
    expect(isSelectionPreview(value, "youtube", intent)).toBe(false);
  });
  it.each(["manual"] as const)("correlates mode %s", (mode) => {
    const request: SelectionIntent = { mode, videoFormatId: "v1", outputContainer: "mp4" };
    expect(checkPlan({ mode, containerSelection: "explicit" }, request)).toBe(true);
  });
  it.each([["a1", "keep", "m4a"], ["c1", "extract_audio", "m4a"]])("accepts audio-only %s/%s", (id, operation, outputContainer) => {
    expect(isSelectionPreview({ ...preview, outcome: { status: "selected", plan: { compatibilityMode: false, containerSelection: "automatic" as const,  mode: "audio_only", sourceFormatIds: [id], audioFormatId: id, operation, outputContainer, outputAudioCodec: "aac", audioProcessing: "copy", warnings: plan.warnings } } }, "youtube", { mode: "audio_only" })).toBe(true);
  });
  it("requires the Rust-derived container for a recognized merge pair", () => {
    const { outputContainer: _container, ...withoutContainer } = plan;
    expect(isSelectionPreview({ ...preview, outcome: { status: "selected", plan: withoutContainer } }, "youtube", intent)).toBe(false);
    expect(checkPlan({ outputContainer: "mkv" })).toBe(false);
    expect(checkPlan({ outputContainer: "exe" })).toBe(false);
    expect(checkPlan({ outputContainer: "mp4" })).toBe(true);
  });
  it("copies native Opus to MP4 or an explicitly requested WebM", () => {
    const sourceMedia = { ...media, formats: [{ ...video, videoCodec: "vp9" }, { ...audio, audioCodec: "opus", container: "webm" }, combined] };
    const check = (fields: object, request = intent) => isSelectionPreview({ ...preview, media: sourceMedia, outcome: { status: "selected", plan: { ...plan, outputVideoCodec: "vp9", outputAudioCodec: "opus", ...fields } } }, "youtube", request);
    expect(check({})).toBe(true);
    expect(check({ outputContainer: "mkv" })).toBe(false);
    expect(check({ outputContainer: "webm", containerSelection: "explicit", mode: "manual" }, { mode: "manual", videoFormatId: "v1", outputContainer: "webm" })).toBe(true);
    expect(check({ outputContainer: "mp4", outputAudioCodec: "aac", audioProcessing: "transcode" })).toBe(false);
    const unknownMedia = { ...media, formats: [{ ...video, videoCodec: "futurevideo" }, audio, combined] };
    expect(isSelectionPreview({ ...preview, media: unknownMedia }, "youtube", intent)).toBe(false);
  });
  it.each([
    ["aac", "aac", "m4a"], ["mp4a.40.2", "aac", "m4a"], ["alac", "alac", "m4a"], ["opus", "opus", "webm"],
    ["vorbis", "vorbis", "webm"], ["mp3", "mp3", "mp3"], ["flac", "flac", "flac"],
  ])("keeps original %s in audio extraction", (audioCodec, outputAudioCodec, outputContainer) => {
    const source = { ...combined, audioCodec };
    const selected = { compatibilityMode: false, containerSelection: "automatic" as const, mode: "audio_only", sourceFormatIds: ["c1"], audioFormatId: "c1", operation: "extract_audio", outputContainer, outputAudioCodec, audioProcessing: "copy", warnings: plan.warnings };
    const check = (fields: object) => isSelectionPreview({ ...preview, media: { ...media, formats: [video, audio, source] }, outcome: { status: "selected", plan: { ...selected, ...fields } } }, "youtube", { mode: "audio_only" });
    expect(check({})).toBe(true);
    expect(check({ outputContainer: "opus" })).toBe(false);
    expect(check({ audioProcessing: "transcode" })).toBe(false);
    expect(check({ estimatedSizeBytes: 100 })).toBe(false);
  });
  it("rejects extraction from an unknown audio codec", () => {
    const source = { ...combined, audioCodec: "futureaudio" };
    const selected = { compatibilityMode: false, containerSelection: "automatic" as const,  mode: "audio_only", sourceFormatIds: ["c1"], audioFormatId: "c1", operation: "extract_audio", outputContainer: "m4a", outputAudioCodec: "aac", audioProcessing: "transcode", warnings: plan.warnings };
    expect(isSelectionPreview({ ...preview, media: { ...media, formats: [video, audio, source] }, outcome: { status: "selected", plan: selected } }, "youtube", { mode: "audio_only" })).toBe(false);
  });
  it.each([
    ["keep", 100, undefined, 100, true], ["keep", 100, undefined, 1, false],
    ["keep", 100, undefined, undefined, false], ["keep", undefined, undefined, 1, false],
    ["keep", undefined, undefined, undefined, true], ["keep", 1, undefined, 1, true],
    ["keep", 9_007_199_254_740_991, undefined, 9_007_199_254_740_991, true],
    ["merge", 100, 50, 150, true], ["merge", 100, 50, 100, false],
    ["merge", 100, 50, undefined, false], ["merge", undefined, 50, 50, false],
    ["merge", 100, undefined, 100, false], ["merge", undefined, undefined, 1, false],
    ["merge", undefined, 50, undefined, true], ["merge", 100, undefined, undefined, true],
    ["merge", 9_007_199_254_740_990, 1, 9_007_199_254_740_991, true],
    ["merge", 9_007_199_254_740_991, 1, undefined, true],
    ["merge", 9_007_199_254_740_991, 1, 9_007_199_254_740_991, false],
  ] as const)("correlates %s source bytes %s/%s with estimate %s (valid=%s)", (operation, videoBytes, audioBytes, estimate, expected) => {
    const sized = (bytes: number | undefined) => bytes === undefined ? {} : { fileSize: { bytes, isApproximate: true } };
    const formats = operation === "keep"
      ? [{ ...video, ...sized(999) }, { ...audio, ...sized(999) }, { ...combined, ...sized(videoBytes) }]
      : [{ ...video, ...sized(videoBytes) }, { ...audio, ...sized(audioBytes) }, { ...combined, ...sized(999) }];
    const selected = {
      ...plan,
      ...(operation === "keep" ? { operation, videoFormatId: "c1", audioFormatId: "c1", sourceFormatIds: ["c1"] } : {}),
      ...(estimate === undefined ? {} : { estimatedSizeBytes: estimate }),
      warnings: estimate === undefined ? plan.warnings : ["metadata_incomplete", "container_unverified"],
    };
    expect(isSelectionPreview({ ...preview, media: { ...media, formats }, outcome: { status: "selected", plan: selected } }, "youtube", intent)).toBe(expected);
  });
  it.each(["keep", "merge", "extract_audio"] as const)("omits estimates for %s when clipping or extracting a partial source", (operation) => {
    const clip = { startMs: 0, endMs: 1000 };
    const audioOnly = operation === "extract_audio";
    const request: SelectionIntent = audioOnly ? { mode: "audio_only" } : { ...intent, clip };
    const selection = operation === "merge" ? { ...plan, clip }
      : operation === "keep" ? { ...plan, operation, videoFormatId: "c1", audioFormatId: "c1", sourceFormatIds: ["c1"], clip }
        : { compatibilityMode: false, containerSelection: "automatic" as const, mode: "audio_only", operation, audioFormatId: "c1", sourceFormatIds: ["c1"], outputContainer: "m4a", outputAudioCodec: "aac", audioProcessing: "copy" };
    const formats = [video, audio, combined].map((format) => ({ ...format, fileSize: { bytes: 100, isApproximate: false } }));
    const warnings = [...plan.warnings, ...(audioOnly ? [] : ["clip_boundary_may_vary", "clip_transfer_may_exceed_range"])];
    const check = (fields: object) => isSelectionPreview({ ...preview, media: { ...media, formats }, outcome: { status: "selected", plan: { ...selection, warnings, ...fields } } }, "youtube", request);
    expect(check({})).toBe(true);
    expect(check({ estimatedSizeBytes: 100, warnings: warnings.filter((warning) => warning !== "output_size_unknown") })).toBe(false);
  });
  it("correlates unavailable codes with request mode and clip validation order", () => {
    const unavailable = (code: string, request: SelectionIntent, returnedMedia = media) => isSelectionPreview(
      { ...preview, media: returnedMedia, outcome: { status: "unavailable", code } }, "youtube", request,
    );
    expect(unavailable("format_unavailable", intent)).toBe(true);
    expect(unavailable("invalid_selection", intent)).toBe(false);
    expect(unavailable("invalid_clip", intent)).toBe(false);
    expect(unavailable("clip_duration_unknown", intent)).toBe(false);
    expect(unavailable("invalid_selection", { mode: "manual" })).toBe(true);
    expect(unavailable("format_unavailable", { mode: "manual" })).toBe(false);

    const clip = { startMs: 0, endMs: 1000 };
    expect(unavailable("clip_duration_unknown", { ...intent, clip }, { ...media, durationSeconds: 0 })).toBe(true);
    expect(unavailable("invalid_clip", { ...intent, clip }, { ...media, durationSeconds: 0 })).toBe(false);
    expect(unavailable("invalid_clip", { ...intent, clip }, { ...media, durationSeconds: 0.999 })).toBe(true);
    expect(unavailable("clip_duration_unknown", { ...intent, clip }, { ...media, durationSeconds: 0.999 })).toBe(false);
    expect(unavailable("format_unavailable", { ...intent, clip })).toBe(true);
    expect(unavailable("invalid_clip", { ...intent, clip })).toBe(false);
  });
  describe.each(["combined", "split"] as const)("metadata completeness for %s selection", (kind) => {
    const videoMetrics = { width: 1920, height: 1080, fps: 30, dynamicRange: "SDR" };
    const audioMetrics = { audioChannels: 2, audioBitrateKbps: 128, audioSampleRateHz: 48_000 };
    const formats = kind === "combined"
      ? [video, audio, { ...combined, ...videoMetrics, ...audioMetrics }]
      : [{ ...video, ...videoMetrics }, { ...audio, ...audioMetrics }, combined];
    const selected = kind === "combined"
      ? { ...plan, operation: "keep", videoFormatId: "c1", audioFormatId: "c1", sourceFormatIds: ["c1"] }
      : plan;
    it.each([
      ["width", "v1"], ["height", "v1"], ["fps", "v1"], ["dynamicRange", "v1"],
      ["audioChannels", "a1"], ["audioBitrateKbps", "a1"], ["audioSampleRateHz", "a1"],
    ])("requires metadata_incomplete when selected %s is missing", (field, source) => {
      const incomplete = formats.map((format) => format.id === (kind === "combined" ? "c1" : source)
        ? Object.fromEntries(Object.entries(format).filter(([key]) => key !== field)) : format);
      const check = (warnings: string[]) => isSelectionPreview({ ...preview, media: { ...media, formats: incomplete }, outcome: { status: "selected", plan: { ...selected, warnings } } }, "youtube", intent);
      expect(check(["container_unverified", "output_size_unknown"])).toBe(false);
      expect(check(plan.warnings)).toBe(true);
    });
    it("does not require the warning for incomplete unselected sources", () => {
      expect(isSelectionPreview({ ...preview, media: { ...media, formats }, outcome: { status: "selected", plan: { ...selected, warnings: ["container_unverified", "output_size_unknown"] } } }, "youtube", intent)).toBe(true);
    });
  });
  it.each(["audioChannels", "audioBitrateKbps", "audioSampleRateHz"])("requires metadata_incomplete for audio-only selection missing %s", (field) => {
    const completeAudio = { ...audio, audioChannels: 2, audioBitrateKbps: 128, audioSampleRateHz: 48_000 };
    const incompleteAudio = Object.fromEntries(Object.entries(completeAudio).filter(([key]) => key !== field));
    const selected = { compatibilityMode: false, containerSelection: "automatic" as const,  mode: "audio_only", operation: "keep", audioFormatId: "a1", sourceFormatIds: ["a1"], outputContainer: "m4a", outputAudioCodec: "aac", audioProcessing: "copy", warnings: ["container_unverified", "output_size_unknown"] };
    expect(isSelectionPreview({ ...preview, media: { ...media, formats: [video, incompleteAudio, combined] }, outcome: { status: "selected", plan: selected } }, "youtube", { mode: "audio_only" })).toBe(false);
  });
  it("does not require video metadata when extracting audio from a combined source", () => {
    const source = { ...combined, audioChannels: 2, audioBitrateKbps: 128, audioSampleRateHz: 48_000 };
    const selected = { compatibilityMode: false, containerSelection: "automatic" as const,  mode: "audio_only", operation: "extract_audio", audioFormatId: "c1", sourceFormatIds: ["c1"], outputContainer: "m4a", outputAudioCodec: "aac", audioProcessing: "copy", warnings: ["container_unverified", "output_size_unknown"] };
    expect(isSelectionPreview({ ...preview, media: { ...media, formats: [video, audio, source] }, outcome: { status: "selected", plan: selected } }, "youtube", { mode: "audio_only" })).toBe(true);
  });
  it("allows native metadata_incomplete for unknown codecs even with complete metrics", () => {
    const source = { ...combined, videoCodec: "futurevideo", audioCodec: "futureaudio", width: 1920, height: 1080, fps: 30, dynamicRange: "SDR", audioChannels: 2, audioBitrateKbps: 128, audioSampleRateHz: 48_000 };
    const selected = { ...plan, operation: "keep", videoFormatId: "c1", audioFormatId: "c1", sourceFormatIds: ["c1"] };
    expect(isSelectionPreview({ ...preview, media: { ...media, formats: [video, audio, source] }, outcome: { status: "selected", plan: selected } }, "youtube", intent)).toBe(false);
  });
  it("accepts a requested clip within media duration with ordered warnings", () => {
    const clip = { startMs: 0, endMs: 60_000 };
    expect(checkPlan({ clip, warnings: [...plan.warnings, "clip_boundary_may_vary", "clip_transfer_may_exceed_range"] }, { ...intent, clip })).toBe(true);
  });
  it.each([
    ["extra preview field", { ...preview, raw: true }], ["missing media", { ...preview, media: undefined }],
    ["malformed media", { ...preview, media: { ...media, url: "RAW" } }],
    ["platform mismatch", { ...preview, media: { ...media, platform: "x" } }],
    ["missing outcome", { ...preview, outcome: undefined }],
    ["duplicate video rank", { ...preview, rankedVideoFormatIds: ["v1", "v1"] }],
    ["duplicate audio rank", { ...preview, rankedAudioFormatIds: ["a1", "a1"] }],
    ["unsafe rank", { ...preview, rankedVideoFormatIds: ["v1\n"] }],
    ["unknown rank", { ...preview, rankedVideoFormatIds: ["absent"] }],
    ["wrong video rank role", { ...preview, rankedVideoFormatIds: ["a1", "c1"] }],
    ["wrong audio rank role", { ...preview, rankedAudioFormatIds: ["v1", "c1"] }],
    ["omitted eligible rank", { ...preview, rankedAudioFormatIds: ["a1"] }],
    ["missing rank", { ...preview, rankedVideoFormatIds: undefined }],
    ["unavailable with plan", { ...preview, outcome: { status: "unavailable", code: "invalid_clip", plan } }],
    ["selected with code", { ...preview, outcome: { status: "selected", plan, code: "invalid_clip" } }],
    ["unknown code", { ...preview, outcome: { status: "unavailable", code: "raw" } }],
    ["missing code", { ...preview, outcome: { status: "unavailable" } }],
    ["unknown status", { ...preview, outcome: { status: "ready", plan } }],
  ])("rejects %s", (_name, value) => expect(isSelectionPreview(value, "youtube", intent)).toBe(false));
  it.each([
    ["extra field", { raw: true }], ["missing mode", { mode: undefined }], ["mismatched mode", { mode: "video_only" }],
    ["unsafe id", { videoFormatId: "v1\n" }], ["unknown id", { audioFormatId: "absent" }],
    ["duplicate sources", { sourceFormatIds: ["v1", "v1"] }], ["empty sources", { sourceFormatIds: [] }],
    ["too many sources", { sourceFormatIds: ["v1", "a1", "c1"] }], ["unknown source", { sourceFormatIds: ["v1", "absent"] }],
    ["reversed sources", { sourceFormatIds: ["a1", "v1"] }], ["wrong sources", { sourceFormatIds: ["v1", "c1"] }],
    ["missing audio", { audioFormatId: undefined }], ["missing video", { videoFormatId: undefined }],
    ["wrong operation", { operation: "download" }], ["keep split sources", { operation: "keep" }],
    ["merge single source", { sourceFormatIds: ["v1"] }],
    ["merge video pair", { audioFormatId: "v1" }],
    ["uppercase container", { outputContainer: "MP4" }], ["unsafe container", { outputContainer: "mp4\n" }],
    ["long container", { outputContainer: "a".repeat(17) }], ["undefined container", { outputContainer: undefined }],
    ["unsafe size", { estimatedSizeBytes: Number.MAX_SAFE_INTEGER + 1 }], ["fractional size", { estimatedSizeBytes: 1.5 }],
    ["zero size", { estimatedSizeBytes: 0 }], ["nonfinite size", { estimatedSizeBytes: Infinity }],
    ["unknown warning", { warnings: ["container_unverified", "raw"] }],
    ["duplicate warning", { warnings: [...plan.warnings, "container_unverified"] }],
    ["warning order", { warnings: [...plan.warnings].reverse() }], ["missing warnings", { warnings: undefined }],
    ["missing container warning", { warnings: ["output_size_unknown"] }],
    ["missing size warning", { warnings: ["container_unverified"] }],
    ["spurious clip warning", { warnings: [...plan.warnings, "clip_boundary_may_vary"] }],
    ["unrequested clip", { clip: { startMs: 0, endMs: 1000 } }],
  ])("rejects plan %s", (_name, change) => expect(checkPlan(change)).toBe(false));
  it("rejects a plan that substitutes manual IDs or omits requested ones", () => {
    expect(checkPlan({ mode: "manual" }, { mode: "manual", videoFormatId: "c1" })).toBe(false);
    expect(checkPlan({ mode: "manual" }, { mode: "manual", videoFormatId: "v1" })).toBe(true);
  });
  it("requires requested clip equality, valid endpoints and known positive duration", () => {
    const clip = { startMs: 0, endMs: 1000 };
    const warnings = [...plan.warnings, "clip_boundary_may_vary", "clip_transfer_may_exceed_range"];
    expect(checkPlan({}, { ...intent, clip })).toBe(false);
    expect(checkPlan({ clip: { startMs: 1, endMs: 1000 }, warnings }, { ...intent, clip })).toBe(false);
    expect(checkPlan({ clip, warnings: plan.warnings }, { ...intent, clip })).toBe(false);
    expect(checkPlan({ clip, warnings, estimatedSizeBytes: 1 }, { ...intent, clip })).toBe(false);
    for (const durationSeconds of [undefined, 0, 0.9999]) {
      expect(isSelectionPreview({ ...preview, media: { ...media, durationSeconds }, outcome: { status: "selected", plan: { ...plan, clip, warnings } } }, "youtube", { ...intent, clip })).toBe(false);
    }
  });
  it("rejects audio extraction from audio-only and video fields/clip/size in audio mode", () => {
    const audioPlan = { compatibilityMode: false, containerSelection: "automatic" as const,  mode: "audio_only", sourceFormatIds: ["a1"], audioFormatId: "a1", operation: "keep", outputContainer: "m4a", warnings: plan.warnings };
    for (const change of [{ operation: "extract_audio" }, { videoFormatId: "v1" }, { clip: { startMs: 0, endMs: 1 } }, { estimatedSizeBytes: 1 }]) {
      expect(isSelectionPreview({ ...preview, outcome: { status: "selected", plan: { ...audioPlan, ...change } } }, "youtube", { mode: "audio_only" })).toBe(false);
    }
  });
});
