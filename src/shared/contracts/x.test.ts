import { describe, expect, it } from "vitest";
import { isXAnalysisPreview, isXDownloadIntent, isXDownloadResult, isXPreviewPayload } from "./x";

const caps = { postBatch:true, images:true, videos:true, compatibility:true, clip:false, manualQuality:false, audioOnly:false, videoOnly:false };
const item = { itemId:"2", ordinal:1, kind:"animated_video", container:"mp4", width:1920, height:1080, previewAvailable:false, availability:"ready" };
const analysis = { analysisId:"1", postId:"12345678901234567890", title:"X 帖子", sourcePageUrl:"https://x.com/i/web/status/12345678901234567890", capabilities:caps, items:[item] };
const result = { jobId:"1", state:"completed", postItemCount:3, unavailableCount:1, contentChanged:false, completedCount:1, failedCount:0, folderDisplayName:"X post", items:[{ itemId:"2", ordinal:2, kind:"video", state:"completed", displayName:"02.mp4" }] };

describe("X closed media contracts", () => {
  it("accepts canonical X posts and silent animated video without inventing audio", () => {
    expect(isXAnalysisPreview(analysis)).toBe(true);
    expect(isXAnalysisPreview({ ...analysis, postId:12345678901234567890 })).toBe(false);
    expect(isXAnalysisPreview({ ...analysis, sourcePageUrl:analysis.sourcePageUrl+"?token=private" })).toBe(false);
    expect(isXAnalysisPreview({ ...analysis, items:[{...item, rawMediaUrl:"https://video.twimg.com/a.mp4"}] })).toBe(false);
  });
  it("rejects enabling clip, manual quality and audio-only capabilities", () => {
    for (const field of ["clip","manualQuality","audioOnly","videoOnly"]) expect(isXAnalysisPreview({...analysis, capabilities:{...caps,[field]:true}})).toBe(false);
  });
  it("accepts only highest-quality unique item selection and compatibility opt-in", () => {
    expect(isXDownloadIntent({itemIds:["1","2"],compatibilityMode:false})).toBe(true);
    for (const injected of [{clip:{startMs:0,endMs:1000}}, {resolution:1080}, {formatId:"best"}, {url:"https://x.com/i/web/status/1"}]) expect(isXDownloadIntent({itemIds:["1"],compatibilityMode:false,...injected})).toBe(false);
    expect(isXDownloadIntent({itemIds:["1","1"],compatibilityMode:false})).toBe(false);
    expect(isXDownloadIntent({itemIds:[],compatibilityMode:false})).toBe(false);
    expect(isXDownloadIntent({itemIds:Array.from({length:33},(_,i)=>String(i+1)),compatibilityMode:false})).toBe(false);
  });
  it("preserves whole-post exclusions and rejects inconsistent result counts", () => {
    expect(isXDownloadResult(result)).toBe(true);
    expect(isXDownloadResult({...result, postItemCount:0})).toBe(false);
    expect(isXDownloadResult({...result, unavailableCount:4})).toBe(false);
    expect(isXDownloadResult({...result, completedCount:2})).toBe(false);
    expect(isXDownloadResult({...result, contentChanged:"false"})).toBe(false);
    expect(isXPreviewPayload({mimeType:"image/jpeg",dataBase64:"AQID"})).toBe(true);
    expect(isXPreviewPayload({mimeType:"text/html",dataBase64:"AQID"})).toBe(false);
  });
});
