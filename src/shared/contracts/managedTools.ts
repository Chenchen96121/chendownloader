export const toolNames = ["yt-dlp", "deno", "gallery-dl", "ffmpeg", "ffprobe"] as const;

export type ToolName = (typeof toolNames)[number];
export type ToolState = "missing" | "ready" | "integrity_failed";

export interface ToolStatus {
  name: ToolName;
  state: ToolState;
  version?: string;
}

export interface ToolInventory {
  target: string;
  tools: ToolStatus[];
}

const toolStates: readonly ToolState[] = ["missing", "ready", "integrity_failed"];

export function isToolInventory(value: unknown): value is ToolInventory {
  if (typeof value !== "object" || value === null) return false;

  const candidate = value as Record<string, unknown>;
  if (typeof candidate.target !== "string" || candidate.target.length === 0 || !Array.isArray(candidate.tools)) {
    return false;
  }

  return candidate.tools.length === toolNames.length && candidate.tools.every((tool, index) => {
    if (typeof tool !== "object" || tool === null) return false;

    const candidateTool = tool as Record<string, unknown>;
    const state = candidateTool.state;
    const version = candidateTool.version;
    return candidateTool.name === toolNames[index]
      && typeof state === "string"
      && toolStates.includes(state as ToolState)
      && (state === "ready"
        ? typeof version === "string" && version.length > 0
        : version === undefined);
  });
}
