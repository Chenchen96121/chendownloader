export const toolProvisioningStates = ["preparing", "ready", "repairing", "failed"] as const;
export const toolProvisioningErrorCodes = [
  "package_missing",
  "package_invalid",
  "tool_probe_failed",
  "activation_failed",
  "busy",
  "download_in_progress",
] as const;

export type ToolProvisioningState = (typeof toolProvisioningStates)[number];
export type ToolProvisioningErrorCode = (typeof toolProvisioningErrorCodes)[number];

export type ToolProvisioningStatus =
  | { state: "preparing" | "ready" | "repairing" }
  | {
    state: "failed";
    errorCode: Exclude<ToolProvisioningErrorCode, "busy" | "download_in_progress">;
  };

const stateSet = new Set<string>(toolProvisioningStates);
const errorSet = new Set<string>(toolProvisioningErrorCodes);
const fatalErrorSet = new Set<string>(toolProvisioningErrorCodes.slice(0, 4));

export function isToolProvisioningErrorCode(value: unknown): value is ToolProvisioningErrorCode {
  return typeof value === "string" && errorSet.has(value);
}

export function isToolProvisioningStatus(value: unknown): value is ToolProvisioningStatus {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const candidate = value as Record<string, unknown>;
  if (typeof candidate.state !== "string" || !stateSet.has(candidate.state)) return false;
  const keys = Object.keys(candidate);
  if (candidate.state === "failed") {
    return keys.length === 2
      && keys.includes("errorCode")
      && typeof candidate.errorCode === "string"
      && fatalErrorSet.has(candidate.errorCode);
  }
  return keys.length === 1 && candidate.errorCode === undefined;
}
