import type { TikTokDownloadResult } from "../../shared/contracts/tiktok";
import type { Catalog } from "../../shared/i18n/catalog";
export function tiktokMessage(template: string, values: Record<string, number>) {
  return template.replace(/\{(\w+)\}/gu, (_, key: string) => String(values[key] ?? ""));
}
export function tiktokResultSummary(result: TikTokDownloadResult, copy: Catalog): string {
  if (result.state === "partial") return tiktokMessage(copy.tiktokPartialSummary, { completed: result.completedCount, failed: result.failedCount });
  if (result.state === "failed") return tiktokMessage(copy.tiktokFailedSummary, { count: result.failedCount });
  if (result.state === "canceled") return tiktokMessage(copy.tiktokCanceledSummary, { completed: result.completedCount });
  const wholePost = !result.contentChanged && result.unavailableCount === 0 && result.completedCount === result.postItemCount;
  return tiktokMessage(wholePost ? copy.tiktokWholeCompletedSummary : copy.tiktokSelectedSavedSummary, { count: result.completedCount });
}
