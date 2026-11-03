import { describe, expect, it } from "vitest";
import { parseClipTime, formatClipTime, moveClipBoundary, fullClipRange } from "./clipRange";

describe("timeline ranges", () => {
  it("initializes the full bounded interval, with millisecond precision", () => {
    expect(fullClipRange(31.234)).toEqual({ startMs: 0, endMs: 31234 });
    for (const duration of [undefined, 0, -1, NaN, Infinity, 0.0001, 31536001]) expect(fullClipRange(duration)).toBeUndefined();
    expect(formatClipTime(31234)).toBe("0:00:31.234");
    expect(parseClipTime(formatClipTime(31536000000))).toBe(31536000000);
  });
  it("clamps either boundary without crossing or making a zero-length interval", () => {
    const range = { startMs: 1000, endMs: 5000 };
    expect(moveClipBoundary(10000, range, "start", -100)).toEqual({ startMs: 0, endMs: 5000 });
    expect(moveClipBoundary(10000, range, "start", 9000)).toEqual({ startMs: 4999, endMs: 5000 });
    expect(moveClipBoundary(10000, range, "end", 0)).toEqual({ startMs: 1000, endMs: 1001 });
    expect(moveClipBoundary(10000, range, "end", 20000)).toEqual({ startMs: 1000, endMs: 10000 });
    expect(moveClipBoundary(10000, range, "end", NaN)).toBeUndefined();
    expect(moveClipBoundary(0, range, "end", 100)).toBeUndefined();
    expect(moveClipBoundary(10000, { startMs: 9, endMs: 9 }, "start", 0)).toBeUndefined();
  });
});

describe("parseClipTime", () => {
  it.each([
    ["0:00:00", 0], ["0:00:10", 10_000], ["12:34:56.7", 45_296_700],
    ["0:00:00.01", 10], ["0:00:00.001", 1], ["0000:00:00.999", 999],
    ["1:59:59.999", 7_199_999], ["8760:00:00", 31_536_000_000],
    ["8759:59:59.999", 31_535_999_999],
  ])("parses %s to integer milliseconds", (input, expected) => expect(parseClipTime(input)).toBe(expected));
  it.each([
    "", "0", "1e2", "0:00", "00:00", "0:00:0", "0:0:00", "0:00:00.", "0:00:00.0000",
    "00000:00:00", "0:60:00", "0:00:60", "-1:00:00", "+1:00:00", "1:00:00s", " 1:00:00", "1:00:00 ",
    "1:00:00\n", "1:00:00\r", "1:00:00\t", "1:00:00\0", "1:00:00\u2028", "１:00:00", "1:٠٠:00",
    "8760:00:00.001", "9999:59:59.999", "12345:12:12.123", "0:00:00. 1",
  ])("rejects noncanonical or out-of-range input %j", (input) => expect(parseClipTime(input)).toBeUndefined());
});
