import { describe, expect, it } from "vitest";
import { isTikTokAnalysisPreview, isTikTokDownloadIntent, isTikTokDownloadResult, isTikTokPreviewPayload } from "./tiktok";

const caps = { postBatch:true, images:true, videos:true, compatibility:true, clip:false, manualQuality:false, audioOnly:false, videoOnly:false };
const item = { itemId:"2", ordinal:1, kind:"video", container:"mp4", width:1920, height:1080, previewAvailable:false, availability:"ready" };
const analysis = { analysisId:"1", postId:"12345678901234567890", title:"TikTok 帖子", sourcePageUrl:"https://www.tiktok.com/@example/video/12345678901234567890", capabilities:caps, items:[item] };
const result = { jobId:"1", state:"completed", postItemCount:3, unavailableCount:1, contentChanged:false, completedCount:1, failedCount:0, folderDisplayName:"TikTok post", items:[{ itemId:"2", ordinal:2, kind:"video", state:"completed", displayName:"02.mp4" }] };

describe("TikTok closed media contracts", () => {
  it("accepts canonical TikTok posts and silent animated video without inventing audio", () => {
    expect(isTikTokAnalysisPreview(analysis)).toBe(true);
    expect(isTikTokAnalysisPreview({ ...analysis, postId:12345678901234567890 })).toBe(false);
    expect(isTikTokAnalysisPreview({ ...analysis, sourcePageUrl:analysis.sourcePageUrl+"?token=private" })).toBe(false);
    expect(isTikTokAnalysisPreview({ ...analysis, items:[{...item, rawMediaUrl:"https://video.twimg.com/a.mp4"}] })).toBe(false);
  });
  it("rejects enabling clip, manual quality and audio-only capabilities", () => {
    for (const field of ["clip","manualQuality","audioOnly","videoOnly"]) expect(isTikTokAnalysisPreview({...analysis, capabilities:{...caps,[field]:true}})).toBe(false);
  });
  it("accepts only highest-quality unique item selection and compatibility opt-in", () => {
    expect(isTikTokDownloadIntent({itemIds:["1","2"],compatibilityMode:false})).toBe(true);
    for (const injected of [{clip:{startMs:0,endMs:1000}}, {resolution:1080}, {formatId:"best"}, {url:"https://www.tiktok.com/@example/video/1"}]) expect(isTikTokDownloadIntent({itemIds:["1"],compatibilityMode:false,...injected})).toBe(false);
    expect(isTikTokDownloadIntent({itemIds:["1","1"],compatibilityMode:false})).toBe(false);
    expect(isTikTokDownloadIntent({itemIds:[],compatibilityMode:false})).toBe(false);
    expect(isTikTokDownloadIntent({itemIds:Array.from({length:101},(_,i)=>String(i+1)),compatibilityMode:false})).toBe(false);
  });
  it("preserves whole-post exclusions and rejects inconsistent result counts", () => {
    expect(isTikTokDownloadResult(result)).toBe(true);
    expect(isTikTokDownloadResult({...result, postItemCount:0})).toBe(false);
    expect(isTikTokDownloadResult({...result, unavailableCount:4})).toBe(false);
    expect(isTikTokDownloadResult({...result, completedCount:2})).toBe(false);
    expect(isTikTokDownloadResult({...result, contentChanged:"false"})).toBe(false);
    expect(isTikTokPreviewPayload({mimeType:"image/jpeg",dataBase64:"AQID"})).toBe(true);
    expect(isTikTokPreviewPayload({mimeType:"text/html",dataBase64:"AQID"})).toBe(false);
  });
});
