export interface RuntimeStatus {
  appVersion: string;
  localOnly: true;
  managedToolsConfigured: boolean;
}

export function isRuntimeStatus(value: unknown): value is RuntimeStatus {
  if (typeof value !== "object" || value === null) return false;

  const candidate = value as Record<string, unknown>;
  return typeof candidate.appVersion === "string"
    && candidate.localOnly === true
    && typeof candidate.managedToolsConfigured === "boolean";
}
