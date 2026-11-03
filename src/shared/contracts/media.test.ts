import { describe, expect, it } from "vitest";
import { isMediaInfo } from "./media";

// Hand-defined projected DTO: expectations do not use the production validator.
const video = { id: "v1", container: "mp4", kind: "video_only", videoCodec: "avc1.640028" };
const audio = { id: "a1", container: "m4a", kind: "audio_only", audioCodec: "mp4a.40.2" };
const combined = { id: "c1", container: "webm", kind: "combined", videoCodec: "vp9", audioCodec: "opus" };
const sample = { platform: "youtube", mediaId: "synthetic-01", title: "Sample 海 🌊", formats: [video, audio, combined] };

describe("isMediaInfo", () => {
  it.each([
    { videoBitrateKbps: 0.001, audioBitrateKbps: 0.001, audioSampleRateHz: 1, audioChannels: 1 },
    { videoBitrateKbps: 1_000_000, audioBitrateKbps: 100_000, audioSampleRateHz: 768_000, audioChannels: 64 },
  ])("accepts bounded stream metadata %j", (metrics) => {
    expect(isMediaInfo({ ...sample, formats: [{ ...combined, ...metrics }] })).toBe(true);
  });
  it.each([
    ["videoBitrateKbps", 1_000_000], ["audioBitrateKbps", 100_000],
    ["audioSampleRateHz", 768_000], ["audioChannels", 64],
  ] as const)("rejects invalid %s metrics", (key, max) => {
    for (const value of [0, -1, max + 1, Infinity, NaN, "1", null, undefined]) {
      expect(isMediaInfo({ ...sample, formats: [{ ...combined, [key]: value }] }), `${key}=${value}`).toBe(false);
    }
  });
  it.each(["audioSampleRateHz", "audioChannels"])("rejects fractional integer metric %s", (key) => {
    expect(isMediaInfo({ ...sample, formats: [{ ...combined, [key]: 1.5 }] })).toBe(false);
  });
  it("rejects metadata on the wrong stream role", () => {
    expect(isMediaInfo({ ...sample, formats: [{ ...audio, videoBitrateKbps: 1 }] })).toBe(false);
    for (const key of ["audioBitrateKbps", "audioSampleRateHz", "audioChannels"]) {
      expect(isMediaInfo({ ...sample, formats: [{ ...video, [key]: 1 }] })).toBe(false);
    }
  });
  it("accepts Unicode titles, all stream kinds and omitted unknown values", () => {
    expect(isMediaInfo(sample)).toBe(true);
  });
  it.each(["youtube", "bilibili", "x", "instagram"])("accepts the closed platform %s", (platform) => {
    expect(isMediaInfo({ ...sample, platform })).toBe(true);
  });
  it.each([true, false])("accepts size approximation flag %s", (isApproximate) => {
    expect(isMediaInfo({ ...sample, creator: "Sample creator", durationSeconds: 0,
      formats: [{ ...video, width: 16384, height: 1, fps: 29.97, dynamicRange: "HDR10+", fileSize: { bytes: 9007199254740991, isApproximate } }] })).toBe(true);
  });
  it("counts Unicode scalars rather than UTF-16 units", () => {
    expect(isMediaInfo({ ...sample, title: "🌊".repeat(512), creator: "🌊".repeat(256) })).toBe(true);
    expect(isMediaInfo({ ...sample, title: "🌊".repeat(513) })).toBe(false);
  });
  it("mirrors Rust White_Space trimming without treating BOM as whitespace", () => {
    expect(isMediaInfo({ ...sample, title: "\uFEFFSample\uFEFF" })).toBe(true);
    expect(isMediaInfo({ ...sample, title: "\u0085Sample" })).toBe(false);
    expect(isMediaInfo({ ...sample, title: "Sample\u2007" })).toBe(false);
  });
  it("accepts exactly 512 distinct formats", () => {
    expect(isMediaInfo({ ...sample, formats: Array.from({ length: 512 }, (_, i) => ({ ...video, id: `v${i}` })) })).toBe(true);
  });
  it.each([
    ["unknown platform", { platform: "twitter" }], ["empty media id", { mediaId: "" }],
    ["unsafe media id", { mediaId: "https://sample" }], ["long media id", { mediaId: "x".repeat(129) }],
    ["empty title", { title: "" }], ["leading whitespace", { title: " Sample" }],
    ["trailing whitespace", { title: "Sample\u3000" }], ["control text", { title: "Sample\nTitle" }],
    ["DEL text", { title: "Sample\u007f" }], ["C1 text", { title: "Sample\u009f" }],
    ["lone high surrogate", { title: "\ud800" }], ["lone low surrogate", { title: "\udfff" }],
    ["long creator", { creator: "x".repeat(257) }], ["empty creator", { creator: "" }],
    ["untrimmed creator", { creator: " Sample" }], ["null creator", { creator: null }],
    ["undefined optional field", { creator: undefined }], ["negative duration", { durationSeconds: -1 }],
    ["too long duration", { durationSeconds: 31536001 }], ["nonfinite duration", { durationSeconds: Infinity }],
    ["NaN duration", { durationSeconds: NaN }], ["string duration", { durationSeconds: "125" }],
    ["empty formats", { formats: [] }], ["oversized formats", { formats: Array(513).fill(video) }],
    ["duplicate ids", { formats: [video, video] }], ["raw media URL", { url: "discarded-sentinel" }],
    ["thumbnail URL", { thumbnail: "discarded-sentinel" }], ["raw headers", { headers: {} }],
  ])("rejects %s", (_name, change) => {
    expect(isMediaInfo({ ...sample, ...change })).toBe(false);
  });
  it.each([
    ["unknown kind", { kind: "video" }], ["combined without audio", { kind: "combined" }],
    ["video with audio codec", { audioCodec: "opus" }], ["literal none codec", { videoCodec: "none" }],
    ["unsafe codec", { videoCodec: "codec/path" }], ["empty codec", { videoCodec: "" }],
    ["long codec", { videoCodec: "x".repeat(65) }], ["unsafe id", { id: "v 1" }],
    ["long id", { id: "x".repeat(65) }], ["empty id", { id: "" }],
    ["uppercase container", { container: "MP4" }], ["long container", { container: "x".repeat(17) }],
    ["unsafe container", { container: "m-p4" }], ["empty container", { container: "" }],
    ["zero width", { width: 0 }], ["negative height", { height: -1 }], ["large width", { width: 16385 }],
    ["fractional height", { height: 1.5 }], ["nonfinite width", { width: Infinity }],
    ["zero fps", { fps: 0 }], ["negative fps", { fps: -1 }], ["large fps", { fps: 241 }],
    ["nonfinite fps", { fps: NaN }], ["unknown range", { dynamicRange: "HDR99" }],
    ["raw row URL", { url: "discarded-sentinel" }], ["raw filename", { filename: "discarded-sentinel" }],
    ["zero bytes", { fileSize: { bytes: 0, isApproximate: false } }],
    ["negative bytes", { fileSize: { bytes: -1, isApproximate: false } }],
    ["unsafe bytes", { fileSize: { bytes: 9007199254740992, isApproximate: false } }],
    ["fractional bytes", { fileSize: { bytes: 1.5, isApproximate: false } }],
    ["nonfinite bytes", { fileSize: { bytes: Infinity, isApproximate: false } }],
    ["missing approximation", { fileSize: { bytes: 1 } }],
    ["wrong approximation", { fileSize: { bytes: 1, isApproximate: "false" } }],
    ["extra size field", { fileSize: { bytes: 1, isApproximate: false, raw: "sentinel" } }],
  ])("rejects format %s", (_name, change) => {
    expect(isMediaInfo({ ...sample, formats: [{ ...video, ...change }] })).toBe(false);
  });
  it.each([{ width: 1 }, { height: 1 }, { fps: 1 }, { dynamicRange: "SDR" }, { videoCodec: "vp9" }])("rejects video attributes on audio-only rows %j", (change) => {
    expect(isMediaInfo({ ...sample, formats: [{ ...audio, ...change }] })).toBe(false);
  });
  it.each([null, undefined, [], 1, "sample", {}, { ...sample, formats: [null] }])("rejects incomplete/non-object payload %j", (value) => {
    expect(isMediaInfo(value)).toBe(false);
  });
  it.each(["SDR", "HDR", "HDR10", "HDR10+", "HDR12", "HLG", "DV"])("accepts range %s", (dynamicRange) => {
    expect(isMediaInfo({ ...sample, durationSeconds: 31536000, formats: [{ ...video, dynamicRange, fps: 240 }] })).toBe(true);
  });
  it.each(["\n", "\r", "\u2028", "\u2029"])("rejects trailing line terminator %j in every token", (ending) => {
    expect(isMediaInfo({ ...sample, mediaId: `sample${ending}` })).toBe(false);
    expect(isMediaInfo({ ...sample, formats: [{ ...video, id: `v1${ending}` }] })).toBe(false);
    expect(isMediaInfo({ ...sample, formats: [{ ...video, container: `mp4${ending}` }] })).toBe(false);
    expect(isMediaInfo({ ...sample, formats: [{ ...video, videoCodec: `avc1${ending}` }] })).toBe(false);
    expect(isMediaInfo({ ...sample, formats: [{ ...audio, audioCodec: `aac${ending}` }] })).toBe(false);
  });
});
