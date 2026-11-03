import {renderToStaticMarkup} from "react-dom/server";
import {expect,it} from "vitest";
import {SelectionSummary} from "./SelectionDetails";
import {sample,samplePlan} from "./selectionTestFixtures";

it("shows only resolution and fps until source information is expanded", () => {
  const html=renderToStaticMarkup(<SelectionSummary data={sample} outcome={{status:"selected",plan:samplePlan}} locale="zh-CN"/>);
  const source=html.split('class="source-fields"')[1]?.split('</dl>')[0];
  expect(source).toContain("1920 × 1080");
  expect(source).toContain("59.94 FPS");
  expect(source).not.toContain("HDR");
  expect(html).toContain('aria-expanded="false"');
  expect(html).toContain("展开");
});

it.each(["zh-CN","en-US"] as const)("shows selected source roles and target separately in %s",locale=>{
  const data={...sample,media:{...sample.media,formats:sample.media.formats.map(f=>f.id==="hidden-v"?{...f,container:"webm",videoCodec:"av01.0.12M.10"}:f)}};
  const outcome={status:"selected" as const,plan:{...samplePlan,compatibilityMode:true,containerSelection:"compatibility" as const,videoProcessing:"transcode" as const,outputVideoCodec:"h264",estimatedSizeBytes:9000000}};
  const html=renderToStaticMarkup(<SelectionSummary data={data} outcome={outcome} locale={locale}/>);
  for(const text of ["1920 × 1080","59.94 FPS","AAC","MP4","H.264"])expect(html).toContain(text);
  for(const text of ["AV1","HDR10","7,654 kbps","192 kbps","WEBM","M4A"])expect(html).not.toContain(text);
  expect(html).toContain(locale==="zh-CN"?"大小未知":"Size unknown");expect(html).not.toContain("9.00 MB");expect(html).not.toMatch(/hidden-|av01\.0|mp4a\.40/);
});
it("distinguishes no audio, nonapplicable video and unknown source metadata",()=>{
  const videoPlan={...samplePlan};delete videoPlan.audioFormatId;delete videoPlan.outputAudioCodec;delete videoPlan.audioProcessing;
  const video={status:"selected" as const,plan:{...videoPlan,mode:"video_only" as const,operation:"extract_video" as const,sourceFormatIds:["hidden-c"],videoFormatId:"hidden-c"}};
  const html=renderToStaticMarkup(<SelectionSummary data={sample} outcome={video} locale="zh-CN"/>);
  expect(html).toContain("未知");expect(html).not.toContain("30 FPS");
  const audioPlan={...samplePlan};delete audioPlan.videoFormatId;delete audioPlan.videoProcessing;delete audioPlan.outputVideoCodec;
  const audio={status:"selected" as const,plan:{...audioPlan,mode:"audio_only" as const,operation:"extract_audio" as const,sourceFormatIds:["hidden-a"],outputContainer:"m4a" as const}};
  const english=renderToStaticMarkup(<SelectionSummary data={sample} outcome={audio} locale="en-US"/>);
  expect(english).toContain("Not applicable");expect(english).not.toContain("59.94");
});
