import {beforeEach,expect,it,vi} from "vitest";
import {invoke} from "@tauri-apps/api/core";
import {listen} from "@tauri-apps/api/event";
import {bilibiliIpc,BilibiliAnalysisIpcError,BilibiliDownloadIpcError} from "./bilibili";
vi.mock("@tauri-apps/api/core",()=>({invoke:vi.fn()}));
vi.mock("@tauri-apps/api/event",()=>({listen:vi.fn()}));
const intent={mode:"best_available" as const};
const preview={analysisId:"7",part:2,media:{platform:"bilibili",mediaId:"BV1xx411c7mD_p2",title:"P2",formats:[{id:"c1",container:"mp4",kind:"combined",videoCodec:"avc1",audioCodec:"aac"}]},rankedVideoFormatIds:["c1"],rankedAudioFormatIds:["c1"],outcome:{status:"selected",plan:{compatibilityMode:false,containerSelection:"automatic",videoProcessing:"copy",outputVideoCodec:"h264",mode:"best_available",sourceFormatIds:["c1"],videoFormatId:"c1",audioFormatId:"c1",operation:"keep",outputContainer:"mp4",outputAudioCodec:"aac",audioProcessing:"copy",warnings:["metadata_incomplete","container_unverified","output_size_unknown"]}}};
beforeEach(()=>vi.resetAllMocks());
it("sends only closed dedicated Bili analysis and job requests",async()=>{
  vi.mocked(invoke).mockResolvedValueOnce(preview).mockResolvedValueOnce(preview).mockResolvedValueOnce({jobId:"8"}).mockResolvedValue(null);
  await bilibiliIpc.analyze("https://www.bilibili.com/video/BV1xx411c7mD?p=2",intent);
  expect(invoke).toHaveBeenLastCalledWith("analyze_bilibili",{request:{url:"https://www.bilibili.com/video/BV1xx411c7mD?p=2",intent}});
  await bilibiliIpc.preview("7",intent);
  expect(invoke).toHaveBeenLastCalledWith("preview_bilibili_selection",{request:{analysisId:"7",intent}});
  await bilibiliIpc.startDownload("7","8",intent);
  expect(invoke).toHaveBeenLastCalledWith("start_bilibili_download",{request:{analysisId:"7",destinationId:"8",intent}});
  await bilibiliIpc.cancelDownload("8");expect(invoke).toHaveBeenLastCalledWith("cancel_bilibili_download",{request:{jobId:"8"}});
  await bilibiliIpc.openCompletedLocation("8");expect(invoke).toHaveBeenLastCalledWith("open_bilibili_completed_location",{request:{jobId:"8"}});
  await bilibiliIpc.openSource("7");expect(invoke).toHaveBeenLastCalledWith("open_bilibili_source",{request:{analysisId:"7"}});
});
it("rejects mismatched part/platform/IDs, private fields and untrusted errors",async()=>{
  for(const response of [{...preview,part:1},{...preview,cid:"123"},{...preview,media:{...preview.media,platform:"youtube"}},{...preview,analysisId:"9"}]) {
    vi.mocked(invoke).mockResolvedValue(response);await expect(bilibiliIpc.preview("7",intent)).rejects.toBeInstanceOf(BilibiliAnalysisIpcError);
  }
  vi.mocked(invoke).mockResolvedValue({jobId:"9",state:"completed",errorCode:null,displayName:"Video.mp4"});
  await expect(bilibiliIpc.getDownloadResult("8")).rejects.toBeInstanceOf(BilibiliDownloadIpcError);
  vi.mocked(invoke).mockRejectedValue("private signed URL");
  await expect(bilibiliIpc.cover("7")).rejects.toMatchObject({code:"analysis_unavailable"});
  await expect(bilibiliIpc.cancelDownload("8")).rejects.toMatchObject({code:"unknown"});
  vi.mocked(invoke).mockRejectedValue("analysis_expired");await expect(bilibiliIpc.cover("7")).rejects.toMatchObject({code:"analysis_expired"});
});
it("rejects invalid requests before invoking and bounds actual cover payloads",async()=>{
  await expect(bilibiliIpc.startDownload("0","8",intent)).rejects.toMatchObject({code:"invalid_request"});
  await expect(bilibiliIpc.analyze("\n",intent)).rejects.toMatchObject({code:"invalid_url"});
  await expect(bilibiliIpc.preview("7",{...intent,sourceUrl:"https://evil.test"} as never)).rejects.toBeInstanceOf(BilibiliAnalysisIpcError);
  expect(invoke).not.toHaveBeenCalled();
  vi.mocked(invoke).mockResolvedValue({mimeType:"image/svg+xml",dataBase64:"AAAA"});await expect(bilibiliIpc.cover("7")).rejects.toBeInstanceOf(BilibiliAnalysisIpcError);
  expect(invoke).toHaveBeenLastCalledWith("get_bilibili_cover",{request:{analysisId:"7"}});
});
it("validates resolution rows against the same Bili snapshot and intent",async()=>{
  vi.mocked(invoke).mockResolvedValue({analysisId:"7",mode:"manual",compatibilityMode:false,options:[{videoFormatId:"c1",outputContainer:"mp4",outcome:{status:"selected",plan:{...preview.outcome.plan,mode:"manual",containerSelection:"explicit"}}},{videoFormatId:"c1",outputContainer:"webm",outcome:{status:"unavailable",code:"invalid_selection"}}]});
  await expect(bilibiliIpc.resolutionOptions(preview as never,"manual",false)).resolves.toHaveLength(2);
  expect(invoke).toHaveBeenLastCalledWith("get_bilibili_resolution_options",{request:{analysisId:"7",mode:"manual",compatibilityMode:false}});
  vi.mocked(invoke).mockResolvedValue({analysisId:"9",mode:"manual",compatibilityMode:false,options:[]});
  await expect(bilibiliIpc.resolutionOptions(preview as never,"manual",false)).rejects.toBeInstanceOf(BilibiliAnalysisIpcError);
});
it("checks event name and payload before delivering progress and unregisters",async()=>{
  const unlisten=vi.fn();vi.mocked(listen).mockResolvedValue(unlisten);const receive=vi.fn();
  const stop=await bilibiliIpc.listenProgress(receive);
  expect(listen).toHaveBeenCalledWith("bilibili-download-progress",expect.any(Function));
  const callback=vi.mocked(listen).mock.calls[0]![1];
  const payload={jobId:"8",state:"downloading",downloadedBytes:1,totalBytes:2,percent:50,speedBytesPerSecond:null,etaSeconds:null,messageKey:"download.downloading"};
  callback({event:"bilibili-download-progress",id:1,payload});expect(receive).toHaveBeenCalledOnce();
  expect(()=>callback({event:"download-progress",id:1,payload})).toThrow(BilibiliDownloadIpcError);
  expect(()=>callback({event:"bilibili-download-progress",id:1,payload:{...payload,url:"https://private"}})).toThrow(BilibiliDownloadIpcError);
  stop();expect(unlisten).toHaveBeenCalledOnce();
});
