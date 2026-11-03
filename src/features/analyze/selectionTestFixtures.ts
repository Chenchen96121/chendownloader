import type { SelectionPreview } from "../../shared/contracts/selection";

export const sample: SelectionPreview = {
  media: {
    platform: "youtube", mediaId: "hidden-media", title: "<img src=x onerror=alert(1)>",
    creator: "PRIVATE CREATOR", durationSeconds: 120,
    formats: [
      { id: "hidden-v", container: "mp4", kind: "video_only", videoCodec: "avc1.64001f", width: 1920, height: 1080, fps: 59.94, dynamicRange: "HDR10", videoBitrateKbps: 7654, fileSize: { bytes: 2000000, isApproximate: false } },
      { id: "hidden-a", container: "m4a", kind: "audio_only", audioCodec: "mp4a.40.2", audioBitrateKbps: 192, audioSampleRateHz: 48000, audioChannels: 2 },
      { id: "hidden-c", container: "mp4", kind: "combined", videoCodec: "avc1.64001f", audioCodec: "mp4a.40.2" },
    ],
  },
  rankedVideoFormatIds: ["hidden-v", "hidden-c"], rankedAudioFormatIds: ["hidden-a", "hidden-c"],
  outcome: { status: "selected", plan: { compatibilityMode: false, containerSelection: "automatic", videoProcessing: "copy", outputVideoCodec: "h264",  mode: "best_available", sourceFormatIds: ["hidden-v", "hidden-a"], videoFormatId: "hidden-v", audioFormatId: "hidden-a", operation: "merge", outputContainer: "mp4", outputAudioCodec: "aac", audioProcessing: "copy", warnings: ["container_unverified", "output_size_unknown"] } },
};
export const samplePlan = sample.outcome.status === "selected" ? sample.outcome.plan : (()=>{throw new Error("Expected selected fixture");})();
