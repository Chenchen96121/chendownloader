import type { XAnalysisPreview, XDownloadResult } from "../../shared/contracts/x";
import { createXWorkspaceState, xWorkspaceReducer } from "./xState";
export const mixedXPost: XAnalysisPreview = {
  analysisId: "1", postId: "12345678901234567890", title: "A mixed X post", creator: "creator",
  sourcePageUrl: "https://x.com/i/web/status/12345678901234567890",
  capabilities: { postBatch:true, images:true, videos:true, compatibility:true, clip:false, manualQuality:false, audioOnly:false, videoOnly:false },
  items: [
    { itemId:"11", ordinal:1, kind:"image", width:1600, height:900, container:"jpg", fileSize:{bytes:2_000_000,isApproximate:false}, previewAvailable:true, availability:"ready" },
    { itemId:"12", ordinal:2, kind:"video", width:1920, height:1080, container:"mp4", fps:60, videoCodec:"h264", audioCodec:"aac", dynamicRange:"SDR", videoBitrateKbps:8200, previewAvailable:false, availability:"ready" },
    { itemId:"13", ordinal:3, kind:"animated_video", width:1280, height:720, container:"mp4", fps:30, videoCodec:"h264", previewAvailable:true, availability:"ready" },
    { itemId:"14", ordinal:4, kind:"video", container:"unknown", previewAvailable:true, availability:"unavailable", unavailableReason:"access_denied" },
  ],
};
export function readyXState() {
  const initial = xWorkspaceReducer(createXWorkspaceState(), {type:"analysisStarted", generation:1});
  return xWorkspaceReducer(initial, {type:"analysisSucceeded", generation:1, analysis:mixedXPost});
}
export const selectedXResult: XDownloadResult = {
  jobId:"8", state:"completed", postItemCount:4, unavailableCount:1, contentChanged:false,
  completedCount:1, failedCount:0, folderDisplayName:"creator [12345678901234567890]",
  items:[{itemId:"13", ordinal:3, kind:"animated_video", state:"completed", displayName:"03.mp4"}],
};
