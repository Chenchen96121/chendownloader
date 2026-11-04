import type { TikTokAnalysisPreview, TikTokDownloadResult } from "../../shared/contracts/tiktok";
import { createTikTokWorkspaceState, tiktokWorkspaceReducer } from "./tiktokState";
export const mixedTikTokPost: TikTokAnalysisPreview = {
  analysisId: "1", postId: "12345678901234567890", title: "A mixed TikTok post", creator: "creator",
  sourcePageUrl: "https://www.tiktok.com/@example/video/12345678901234567890",
  capabilities: { postBatch:true, images:true, videos:true, compatibility:true, clip:false, manualQuality:false, audioOnly:false, videoOnly:false },
  items: [
    { itemId:"11", ordinal:1, kind:"image", width:1600, height:900, container:"jpg", fileSize:{bytes:2_000_000,isApproximate:false}, previewAvailable:true, availability:"ready" },
    { itemId:"12", ordinal:2, kind:"video", width:1920, height:1080, container:"mp4", fps:60, videoCodec:"h264", audioCodec:"aac", dynamicRange:"SDR", videoBitrateKbps:8200, previewAvailable:false, availability:"ready" },
    { itemId:"13", ordinal:3, kind:"video", width:1280, height:720, container:"mp4", fps:30, videoCodec:"h264", previewAvailable:true, availability:"ready" },
    { itemId:"14", ordinal:4, kind:"video", container:"unknown", previewAvailable:true, availability:"unavailable", unavailableReason:"access_denied" },
  ],
};
export function readyTikTokState() {
  const initial = tiktokWorkspaceReducer(createTikTokWorkspaceState(), {type:"analysisStarted", generation:1});
  return tiktokWorkspaceReducer(initial, {type:"analysisSucceeded", generation:1, analysis:mixedTikTokPost});
}
export const selectedTikTokResult: TikTokDownloadResult = {
  jobId:"8", state:"completed", postItemCount:4, unavailableCount:1, contentChanged:false,
  completedCount:1, failedCount:0, folderDisplayName:"creator [12345678901234567890]",
  items:[{itemId:"13", ordinal:3, kind:"video", state:"completed", displayName:"03.mp4"}],
};
