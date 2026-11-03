import { isDownloadTask, isQueueTerminal, type DownloadTask } from "../../shared/contracts/downloadQueue";
import { cancelDownload, openCompletedLocation, DownloadCommandError } from "../../shared/ipc/downloads";
import { cancelInstagramDownload, openInstagramCompletedLocation } from "../../shared/ipc/instagram";
import { cancelXDownload, openXCompletedLocation } from "../../shared/ipc/x";
import { bilibiliIpc } from "../../shared/ipc/bilibili";
export interface QueueActions {
  cancelYoutube(jobId: string): Promise<void>; cancelInstagram(jobId: string): Promise<void>;
  openYoutube(jobId: string): Promise<void>; openInstagram(jobId: string): Promise<void>;
  cancelX(jobId: string): Promise<void>; openX(jobId: string): Promise<void>;
  cancelBilibili(jobId: string): Promise<void>; openBilibili(jobId: string): Promise<void>;
}
const nativeActions: QueueActions = { cancelYoutube: cancelDownload, cancelInstagram: cancelInstagramDownload, cancelX:cancelXDownload, openYoutube: openCompletedLocation, openInstagram: openInstagramCompletedLocation, openX:openXCompletedLocation,cancelBilibili:bilibiliIpc.cancelDownload,openBilibili:bilibiliIpc.openCompletedLocation };
export async function cancelQueueTask(task: DownloadTask, api = nativeActions): Promise<void> {
  if (!isDownloadTask(task) || isQueueTerminal(task.state)) throw new DownloadCommandError("invalid_request");
  await ({ youtube:api.cancelYoutube, instagram:api.cancelInstagram, x:api.cancelX,bilibili:api.cancelBilibili }[task.platform])(task.jobId);
}
export async function openQueueTask(task: DownloadTask, api = nativeActions): Promise<void> {
  if (!isDownloadTask(task) || !isQueueTerminal(task.state) || task.displayName === null) throw new DownloadCommandError("invalid_request");
  await ({ youtube:api.openYoutube, instagram:api.openInstagram, x:api.openX,bilibili:api.openBilibili }[task.platform])(task.jobId);
}
