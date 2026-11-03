import {expect,it} from "vitest";
import {isBilibiliAnalysisPreview,isBilibiliCoverPayload} from "./bilibili";

const intent={mode:"best_available" as const};
const preview={analysisId:"7",part:2,media:{platform:"bilibili",mediaId:"BV1xx411c7mD_p2",title:"Synthetic P2",formats:[{id:"c1",container:"mp4",kind:"combined",videoCodec:"avc1",audioCodec:"aac"}]},rankedVideoFormatIds:["c1"],rankedAudioFormatIds:["c1"],outcome:{status:"selected",plan:{compatibilityMode:false,containerSelection:"automatic",videoProcessing:"copy",outputVideoCodec:"h264",mode:"best_available",sourceFormatIds:["c1"],videoFormatId:"c1",audioFormatId:"c1",operation:"keep",outputContainer:"mp4",outputAudioCodec:"aac",audioProcessing:"copy",warnings:["metadata_incomplete","container_unverified","output_size_unknown"]}}};
it("accepts only the exact safe Bilibili envelope correlated with P and selection",()=>{
  expect(isBilibiliAnalysisPreview(preview,intent)).toBe(true);
  for(const change of [{cid:"279787"},{part:1},{part:0},{part:10001},{part:2.5},{analysisId:"0"},{media:{...preview.media,platform:"youtube"}},{media:{...preview.media,mediaId:"BV1xx411c7mD_p02"}},{sourceUrl:"https://private.bilivideo.com/secret"}]) expect(isBilibiliAnalysisPreview({...preview,...change},intent)).toBe(false);
  expect(isBilibiliAnalysisPreview(preview,{mode:"manual",videoFormatId:"c1"})).toBe(false);
});
it("limits covers to bounded image payloads with no URL or SVG authority",()=>{
  const png="iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jL1sAAAAASUVORK5CYII=";
  expect(isBilibiliCoverPayload({mimeType:"image/png",dataBase64:png})).toBe(true);
  for(const bad of [{mimeType:"image/svg+xml",dataBase64:png},{mimeType:"image/avif",dataBase64:png},{mimeType:"image/png",dataBase64:""},{mimeType:"image/png",dataBase64:"data:image/png;base64,"+png},{mimeType:"image/png",dataBase64:"!notbase64"},{mimeType:"image/png",dataBase64:png,url:"https://evil.test"},{mimeType:"image/png",dataBase64:"A".repeat(11184816)}]) expect(isBilibiliCoverPayload(bad)).toBe(false);
});
