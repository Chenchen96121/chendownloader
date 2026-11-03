import { describe, expect, it } from "vitest";
import {
  isToolProvisioningErrorCode,
  isToolProvisioningStatus,
  toolProvisioningErrorCodes,
  toolProvisioningStates,
} from "./toolProvisioning";

describe("tool provisioning contract", () => {
  it("accepts only the four closed states and six closed command errors", () => {
    expect(toolProvisioningStates).toEqual(["preparing", "ready", "repairing", "failed"]);
    expect(toolProvisioningErrorCodes).toEqual([
      "package_missing",
      "package_invalid",
      "tool_probe_failed",
      "activation_failed",
      "busy",
      "download_in_progress",
    ]);
    for (const code of toolProvisioningErrorCodes) expect(isToolProvisioningErrorCode(code)).toBe(true);
    expect(isToolProvisioningErrorCode("network_failed")).toBe(false);
  });

  it("accepts exact possible status shapes and rejects unknown or impossible data", () => {
    for (const state of ["preparing", "ready", "repairing"] as const) {
      expect(isToolProvisioningStatus({ state })).toBe(true);
    }
    for (const errorCode of toolProvisioningErrorCodes.slice(0, 4)) {
      expect(isToolProvisioningStatus({ state: "failed", errorCode })).toBe(true);
    }
    for (const value of [
      null,
      { state: "ready", errorCode: "package_missing" },
      { state: "failed" },
      { state: "failed", errorCode: "busy" },
      { state: "unknown" },
      { state: "preparing", extra: true },
      { state: 1 },
      [],
    ]) {
      expect(isToolProvisioningStatus(value)).toBe(false);
    }
  });
});
