import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { getToolProvisioningStatus, repairManagedTools } from "./toolProvisioning";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("tool provisioning IPC", () => {
  beforeEach(() => vi.mocked(invoke).mockReset());

  it("uses fixed argument-free command names", async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce({ state: "preparing" })
      .mockResolvedValueOnce({ state: "ready" });

    await expect(getToolProvisioningStatus()).resolves.toEqual({ state: "preparing" });
    await expect(repairManagedTools()).resolves.toEqual({ state: "ready" });
    expect(invoke).toHaveBeenNthCalledWith(1, "get_tool_provisioning_status");
    expect(invoke).toHaveBeenNthCalledWith(2, "repair_managed_tools");
    expect(vi.mocked(invoke).mock.calls.flat()).not.toContainEqual(expect.stringMatching(/path|url|manifest/i));
  });

  it("rejects malformed native responses", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({ state: "ready", path: "C:\\private" });
    await expect(getToolProvisioningStatus()).rejects.toThrow("Invalid tool provisioning status");
  });
});
