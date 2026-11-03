import { describe, expect, it } from "vitest";
import type { MediaFormat } from "../../shared/contracts/media";
import { formatClipRange, formatCodec, formatDuration, formatFileSize, formatFormatLabel, formatResolution } from "./formatPresentation";

const video: MediaFormat = { id: "hidden-v1", container: "mp4", kind: "video_only", videoCodec: "avc1.64001f" };

describe("concise localized format presentation", () => {
  it.each([[125, "2:05"], [0, "0:00"], [undefined, "时长未知"], [3661, "1:01:01"], [125.9, "2:05"]])("formats duration %s", (value, expected) => {
    expect(formatDuration(value, "zh-CN")).toBe(expected);
  });
  it("localizes unknown duration", () => expect(formatDuration(undefined, "en-US")).toBe("Duration unknown"));
  it.each([
    [512, false, "zh-CN", "512 B"], [2097152, false, "zh-CN", "2.10 MB"],
    [2097152, true, "zh-CN", "约 2.10 MB"], [2097152, true, "en-US", "Approx. 2.10 MB"],
    [1000, false, "en-US", "1.00 KB"], [1000000000, false, "en-US", "1.00 GB"],
  ] as const)("uses decimal size %s", (bytes, isApproximate, locale, expected) => {
    expect(formatFileSize({ bytes, isApproximate }, locale)).toBe(expected);
  });
  it("localizes unknown size", () => {
    expect(formatFileSize(undefined, "zh-CN")).toBe("大小未知");
    expect(formatFileSize(undefined, "en-US")).toBe("Size unknown");
  });
  it("keeps resolution concise even when professional metadata is available", () => {
    expect(formatResolution({ ...video, width: 1920, height: 1080, fps: 29.97, dynamicRange: "HDR10" }, "zh-CN")).toBe("1920 × 1080");
    expect(formatResolution({ ...video, height: 1080, fps: 60 }, "zh-CN")).toBe("分辨率未知");
    expect(formatResolution(video, "en-US")).toBe("Resolution unknown");
  });
  it.each([
    ["AV01.0.12M.08", "AV1"], ["vp09.00.10.08", "VP9"], ["hvc1.1.6", "HEVC"],
    ["avc1.64001f", "H.264"], ["mp4a.40.2", "AAC"], ["mp4a.40.5", "AAC"], ["mp4a.40.29", "AAC"],
    ["alac", "ALAC"], ["OPUS", "Opus"], ["vorbis", "Vorbis"], ["mp3", "MP3"], ["flac", "FLAC"],
    ["avc1evil", "未知"], ["mp4a.69", "未知"], ["opus.extra", "未知"], [undefined, "未知"], ["constructor", "未知"], ["__proto__", "未知"],
  ])("projects codec %s without raw parameters", (token, expected) => {
    expect(formatCodec(token, "zh-CN")).toBe(expected);
  });
  it("localizes unsupported codecs", () => expect(formatCodec("private-token", "en-US")).toBe("Unknown"));
  it("builds manual option labels without IDs or full codecs", () => {
    expect(formatFormatLabel({ ...video, width: 1920, height: 1080, fileSize: { bytes: 2097152, isApproximate: true } }, "zh-CN"))
      .toBe("1920 × 1080 · 约 2.10 MB · MP4");
    expect(formatFormatLabel({ id: "hidden-a1", container: "m4a", kind: "audio_only", audioCodec: "mp4a.40.2" }, "en-US"))
      .toBe("Size unknown · M4A");
  });
  it("shows full nonstandard dimensions rather than a short-edge quality label", () => {
    expect(formatFormatLabel({...video,width:3146,height:2160},"zh-CN")).toBe("3146 × 2160 · 大小未知 · MP4");
  });
  it("shows clip precision and elapsed time without inventing an output size", () => {
    expect(formatClipRange({ startMs: 10000, endMs: 30000 }, "zh-CN")).toBe("00:10–00:30 · 20 秒");
    expect(formatClipRange({ startMs: 10025, endMs: 30075 }, "en-US")).toBe("00:10.025–00:30.075 · 20.05 s");
  });
});
