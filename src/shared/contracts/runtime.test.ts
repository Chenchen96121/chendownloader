import { describe, expect, it } from "vitest";
import { isRuntimeStatus } from "./runtime";

describe("isRuntimeStatus", () => {
  it("accepts the native runtime contract", () => {
    expect(isRuntimeStatus({
      appVersion: "0.1.0",
      localOnly: true,
      managedToolsConfigured: false,
    })).toBe(true);
  });

  it("rejects a contract that is not explicitly local-only", () => {
    expect(isRuntimeStatus({
      appVersion: "0.1.0",
      localOnly: false,
      managedToolsConfigured: false,
    })).toBe(false);
  });
});
