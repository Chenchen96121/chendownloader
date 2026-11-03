import { describe, expect, it } from "vitest";
import { isToolInventory } from "./managedTools";

const validInventory = {
  target: "x86_64-pc-windows-msvc",
  tools: [
    { name: "yt-dlp", state: "missing" },
    { name: "deno", state: "ready", version: "2.5.0" },
    { name: "gallery-dl", state: "ready", version: "1.32.15" },
    { name: "ffmpeg", state: "ready", version: "8.0.1" },
    { name: "ffprobe", state: "integrity_failed" },
  ],
};

describe("isToolInventory", () => {
  it("accepts the ordered native tool inventory contract", () => {
    expect(isToolInventory(validInventory)).toBe(true);
  });

  it("rejects an inventory without a target", () => {
    expect(isToolInventory({ ...validInventory, target: "" })).toBe(false);
  });

  it("rejects an inventory with fewer than the required tools", () => {
    expect(isToolInventory({
      ...validInventory,
      tools: validInventory.tools.slice(0, 4),
    })).toBe(false);
  });

  it("rejects duplicate tools", () => {
    expect(isToolInventory({
      ...validInventory,
      tools: [validInventory.tools[0], validInventory.tools[0], validInventory.tools[2], validInventory.tools[3], validInventory.tools[4]],
    })).toBe(false);
  });

  it("rejects tools outside their canonical order", () => {
    expect(isToolInventory({
      ...validInventory,
      tools: [validInventory.tools[1], validInventory.tools[0], validInventory.tools[2], validInventory.tools[3], validInventory.tools[4]],
    })).toBe(false);
  });

  it("rejects unknown tools", () => {
    expect(isToolInventory({
      ...validInventory,
      tools: [{ name: "unknown", state: "missing" }, validInventory.tools[1], validInventory.tools[2], validInventory.tools[3], validInventory.tools[4]],
    })).toBe(false);
  });

  it("rejects ready tools without a version", () => {
    expect(isToolInventory({
      ...validInventory,
      tools: [validInventory.tools[0], validInventory.tools[1], validInventory.tools[2], { name: "ffmpeg", state: "ready" }, validInventory.tools[4]],
    })).toBe(false);
  });

  it("rejects non-ready tools with a version", () => {
    expect(isToolInventory({
      ...validInventory,
      tools: [{ name: "yt-dlp", state: "missing", version: "8.0.1" }, validInventory.tools[1], validInventory.tools[2], validInventory.tools[3], validInventory.tools[4]],
    })).toBe(false);
  });
});
