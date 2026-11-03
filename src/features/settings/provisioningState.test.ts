import { describe, expect, it } from "vitest";
import {
  beginProvisioningRead,
  beginRepair,
  createProvisioningState,
  failRepair,
  receiveProvisioningStatus,
  shouldPollProvisioning,
  shouldShowWorkspace,
} from "./provisioningState";

describe("provisioning state", () => {
  it("issues one read at a time and polls only bounded transitional states", () => {
    const initial = createProvisioningState();
    const reading = beginProvisioningRead(initial);
    expect(reading).not.toBe(initial);
    expect(beginProvisioningRead(reading)).toBe(reading);
    const preparing = receiveProvisioningStatus(reading, { state: "preparing" });
    expect(shouldPollProvisioning(preparing)).toBe(true);
    expect(shouldPollProvisioning({ ...preparing, readCount: 240 })).toBe(false);
    expect(shouldPollProvisioning(receiveProvisioningStatus(reading, { state: "ready" }))).toBe(false);
    expect(shouldPollProvisioning(receiveProvisioningStatus(reading, { state: "failed", errorCode: "package_invalid" }))).toBe(false);
  });

  it("prevents duplicate repair and automatically exposes the workspace at ready", () => {
    const ready = receiveProvisioningStatus(createProvisioningState(), { state: "ready" });
    expect(shouldShowWorkspace(ready)).toBe(true);
    const repairing = beginRepair(ready);
    expect(repairing.status).toEqual({ state: "repairing" });
    expect(beginRepair(repairing)).toBe(repairing);
    expect(shouldShowWorkspace(repairing)).toBe(false);
    expect(shouldShowWorkspace(receiveProvisioningStatus(repairing, { state: "ready" }))).toBe(true);
  });

  it("keeps locale outside operational identity and closes a failed repair", () => {
    const ready = receiveProvisioningStatus(createProvisioningState(), { state: "ready" });
    const repairing = beginRepair(ready);
    const failed = failRepair(repairing, "tool_probe_failed");
    expect(failed.repairPending).toBe(false);
    expect(failed.repairError).toBe("tool_probe_failed");
    expect(Object.keys(failed)).not.toContain("locale");
  });
});
