import type { SelectionOutcome, VideoContainer } from "./selection";
export interface VideoResolutionPlan { videoFormatId: string; outputContainer: VideoContainer; outcome: SelectionOutcome }
