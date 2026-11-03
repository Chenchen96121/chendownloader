import { expect, it, vi } from "vitest";
import type { SelectionOutcome } from "../../shared/contracts/selection";
import { sample, samplePlan } from "./selectionTestFixtures";
import { resolutionOptions, requestResolutionPlans } from "./resolutionOptions";
import {getResolutionOptions} from "../../shared/ipc/resolutionOptions";
vi.mock("../../shared/ipc/resolutionOptions",()=>({getResolutionOptions:vi.fn()}));

it("late_resolution_response_cannot_replace_source",async()=>{
  const pending=()=>{let resolve!: (value: import("../../shared/contracts/resolutionOptions").VideoResolutionPlan[])=>void;return {promise:new Promise<import("../../shared/contracts/resolutionOptions").VideoResolutionPlan[]>(yes=>{resolve=yes;}),resolve:(value:import("../../shared/contracts/resolutionOptions").VideoResolutionPlan[])=>resolve(value)};};
  const old=pending();const current=pending();const generation={current:0};const applied: {compatibilityMode:boolean;plans:unknown[]}[]=[];
  vi.mocked(getResolutionOptions).mockReturnValueOnce(old.promise).mockReturnValueOnce(current.promise);
  const data={analysisId:"1",...sample};
  const before=requestResolutionPlans(data,"manual",true,generation,result=>applied.push(result));
  const after=requestResolutionPlans(data,"manual",false,generation,result=>applied.push(result));
  const selected={videoFormatId:"hidden-v",outputContainer:"webm" as const,outcome:{status:"selected" as const,plan:{...samplePlan,mode:"manual" as const,outputContainer:"webm" as const}}};
  current.resolve([selected]);await after;
  old.resolve([{videoFormatId:"hidden-c",outputContainer:"mp4",outcome:{status:"selected",plan:{...samplePlan,videoFormatId:"hidden-c",compatibilityMode:true,containerSelection:"compatibility",videoProcessing:"transcode"}}}]);await before;
  expect(applied).toHaveLength(1);expect(applied[0]).toMatchObject({compatibilityMode:false,plans:[selected]});
});

it("keeps two containers for one video source as distinct selectable choices", () => {
  const rows = (["mp4", "webm"] as const).map((outputContainer) => ({
    videoFormatId: "hidden-v", outputContainer,
    outcome: { status: "selected" as const, plan: { compatibilityMode: false, containerSelection: "automatic" as const, videoProcessing: "copy" as const, outputVideoCodec: "h264",
      mode: "manual" as const, operation: "merge" as const, sourceFormatIds: ["hidden-v", "hidden-a"],
      videoFormatId: "hidden-v", audioFormatId: "hidden-a", outputContainer, warnings: [],
    } },
  }));
  const choices = resolutionOptions(sample, "manual", "zh-CN", rows);
  expect(choices.map((choice) => choice.label)).toEqual(["1920 × 1080 · 大小未知 · MP4", "1920 × 1080 · 大小未知 · WEBM"]);
  expect(new Set(choices.map((choice) => choice.value)).size).toBe(2);
});
it("does not group landscape and portrait sources with the same short edge", () => {
  const data={...sample,media:{...sample.media,formats:[...sample.media.formats,{...sample.media.formats[0]!,id:"portrait",width:1080,height:1920}]}};
  const plans=["hidden-v","portrait"].map(id=>({videoFormatId:id,outputContainer:"mp4" as const,outcome:{status:"selected" as const,plan:{...samplePlan,mode:"manual" as const,videoFormatId:id,sourceFormatIds:[id,"hidden-a"]}}}));
  expect(resolutionOptions(data,"manual","zh-CN",plans).map(r=>r.label)).toEqual(["1920 × 1080 · 大小未知 · MP4","1080 × 1920 · 大小未知 · MP4"]);
});

it("groups by full dimensions and actual output container, preserving native order", () => {
  const data = { ...sample, media: { ...sample.media, formats: [
    ...sample.media.formats,
    { ...sample.media.formats[0]!, id: "portrait", width: 1080, height: 1920 },
    { ...sample.media.formats[0]!, id: "mp4" },
    { ...sample.media.formats[0]!, id: "720", width: 1280, height: 720 },
  ] }, rankedVideoFormatIds: ["hidden-v", "portrait", "mp4", "720", "hidden-c"] };
  const outcome = (id: string, container: "mp4" | "webm", bytes?: number): SelectionOutcome => ({ status: "selected", plan: { compatibilityMode: false, containerSelection: "explicit", videoProcessing: "copy" as const, outputVideoCodec: "h264", mode: "manual", videoFormatId: id, audioFormatId: "hidden-a", sourceFormatIds: [id, "hidden-a"], operation: "merge", outputContainer: container, ...(bytes === undefined ? {} : { estimatedSizeBytes: bytes }), warnings: [] } });
  const rows = resolutionOptions(data, "manual", "zh-CN", [
    { videoFormatId: "hidden-v", outputContainer: "webm" as const, outcome: outcome("hidden-v", "webm", 3000000) },
    { videoFormatId: "portrait", outputContainer: "webm" as const, outcome: outcome("portrait", "webm") },
    { videoFormatId: "mp4", outputContainer: "mp4" as const, outcome: outcome("mp4", "mp4") },
    { videoFormatId: "720", outputContainer: "webm" as const, outcome: outcome("720", "webm") },
  ]);
  expect(rows.map(({videoFormatId, label}) => ({videoFormatId, label}))).toEqual([
    { videoFormatId: "hidden-v", label: "1920 × 1080 · 约 3.00 MB · WEBM" },
    { videoFormatId: "portrait", label: "1080 × 1920 · 大小未知 · WEBM" },
    { videoFormatId: "mp4", label: "1920 × 1080 · 大小未知 · MP4" },
    { videoFormatId: "720", label: "1280 × 720 · 大小未知 · WEBM" },
  ]);
  expect(rows.map((row) => row.label).join()).not.toMatch(/AV1|H\.264|AAC|hidden-|HDR|fps/);
});
it("does not advertise the original size after extraction or clipping", () => {
  const plan = sample.outcome.status === "selected" ? sample.outcome.plan : undefined;
  const rows = [{ videoFormatId: "hidden-v", outputContainer: "webm" as const, outcome: { status: "selected" as const, plan: { ...plan!, mode: "video_only" as const, outputContainer: "webm" as const, operation: "extract_video" as const, estimatedSizeBytes: 9000000 } } }];
  expect(resolutionOptions(sample, "video_only", "zh-CN", rows)[0]?.label).toBe("1920 × 1080 · 大小未知 · WEBM");
  const merged = [{ videoFormatId: "hidden-v", outputContainer: "webm" as const, outcome: { status: "selected" as const, plan: { ...plan!, mode: "manual" as const, outputContainer: "webm" as const, estimatedSizeBytes: 9000000 } } }];
  expect(resolutionOptions(sample, "manual", "en-US", merged, true)[0]?.label).toBe("1920 × 1080 · Size unknown · WEBM");
});
it("handles absent dimensions and skips unavailable formats", () => {
  const rows = [{ videoFormatId: "hidden-c", outputContainer: "mp4" as const, outcome: { status: "selected" as const, plan: { compatibilityMode: false, containerSelection: "automatic" as const, videoProcessing: "copy" as const, outputVideoCodec: "h264",  mode: "video_only" as const, operation: "extract_video" as const, sourceFormatIds: ["hidden-c"], videoFormatId: "hidden-c", outputContainer: "mp4" as const, warnings: [] } } }, { videoFormatId: "hidden-v", outputContainer: "webm" as const, outcome: { status: "unavailable" as const, code: "invalid_selection" as const } }];
  expect(resolutionOptions(sample, "video_only", "zh-CN", rows).map(({videoFormatId, label}) => ({videoFormatId, label}))).toEqual([{ videoFormatId: "hidden-c", label: "分辨率未知 · 大小未知 · MP4" }]);
});

it("keeps partially known dimensions as separate source choices", () => {
  const { height: _height, ...videoWithoutHeight } = sample.media.formats[0]!;
  const data = { ...sample, media: { ...sample.media, formats: [
    ...sample.media.formats,
    { ...videoWithoutHeight, id: "wide-unknown-height", width: 1920 },
    { ...videoWithoutHeight, id: "narrow-unknown-height", width: 1080 },
  ] } };
  const plans = ["wide-unknown-height", "narrow-unknown-height"].map((id) => ({
    videoFormatId: id,
    outputContainer: "mp4" as const,
    outcome: { status: "selected" as const, plan: {
      ...samplePlan,
      mode: "manual" as const,
      videoFormatId: id,
      sourceFormatIds: [id, "hidden-a"],
      outputContainer: "mp4" as const,
    } },
  }));

  expect(resolutionOptions(data, "manual", "zh-CN", plans).map(({ videoFormatId }) => videoFormatId)).toEqual([
    "wide-unknown-height",
    "narrow-unknown-height",
  ]);
});

it("keeps the explicit selection visible when mode changes collapse equivalent rows", () => {
  const data = { ...sample, media: { ...sample.media, formats: [...sample.media.formats, { ...sample.media.formats[0]!, id: "chosen" }] }, rankedVideoFormatIds: ["hidden-v", "chosen"] };
  const plans = data.rankedVideoFormatIds.map((id) => ({ videoFormatId: id, outputContainer: "mp4" as const, outcome: { status: "selected" as const, plan: { compatibilityMode: false, containerSelection: "automatic" as const, videoProcessing: "copy" as const, outputVideoCodec: "h264",  mode: "video_only" as const, operation: "keep" as const, videoFormatId: id, sourceFormatIds: [id], outputContainer: "mp4" as const, warnings: [] } } }));
  expect(resolutionOptions(data, "video_only", "zh-CN", plans, false, "chosen", "mp4").map(({videoFormatId, label}) => ({videoFormatId, label}))).toEqual([{ videoFormatId: "chosen", label: "1920 × 1080 · 大小未知 · MP4" }]);
});
