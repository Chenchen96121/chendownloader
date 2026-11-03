import {isLiveAnalysisPreview,type LiveAnalysisPreview} from "./liveAnalysis";
import type {SelectionIntent} from "./selection";

export interface BilibiliAnalysisPreview extends LiveAnalysisPreview {part:number}
export interface BilibiliCoverPayload {mimeType:string;dataBase64:string}
function exact(value:unknown,fields:readonly string[]):value is Record<string,unknown> {
  return typeof value==="object" && value!==null && !Array.isArray(value)
    && [Object.prototype,null].includes(Object.getPrototypeOf(value))
    && Object.keys(value).length===fields.length && fields.every(key=>Object.hasOwn(value,key));
}
export function isBilibiliAnalysisPreview(value:unknown,intent:SelectionIntent):value is BilibiliAnalysisPreview {
  if(!exact(value,["analysisId","part","media","rankedVideoFormatIds","rankedAudioFormatIds","outcome"])
    || typeof value.part!=="number" || !Number.isInteger(value.part) || value.part<1 || value.part>10000)return false;
  const {part,...base}=value;
  return isLiveAnalysisPreview(base,intent) && base.media.platform==="bilibili"
    && new RegExp(`^BV[A-Za-z0-9]{10}_p${part}$`).test(base.media.mediaId);
}
export function isBilibiliCoverPayload(value:unknown):value is BilibiliCoverPayload {
  if(!exact(value,["mimeType","dataBase64"]) || typeof value.dataBase64!=="string" || value.dataBase64.length===0
    || value.dataBase64.length>11_184_812 || value.dataBase64.length%4!==0
    || !/^[A-Za-z0-9+/]+={0,2}$/.test(value.dataBase64))return false;
  const padding=value.dataBase64.endsWith("==")?2:value.dataBase64.endsWith("=")?1:0;
  if(value.dataBase64.length/4*3-padding>8*1024*1024)return false;
  try {
    const header=atob(value.dataBase64.slice(0,32));
    if(value.mimeType==="image/png")return header.startsWith("\x89PNG\r\n\x1a\n");
    if(value.mimeType==="image/jpeg")return header.startsWith("\xff\xd8\xff");
    return value.mimeType==="image/webp" && header.startsWith("RIFF") && header.slice(8,12)==="WEBP";
  }catch{return false;}
}
