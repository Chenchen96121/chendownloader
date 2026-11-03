import type { XDownloadResult } from "../../shared/contracts/x";
import type { Catalog } from "../../shared/i18n/catalog";
export function xMessage(template: string, values: Record<string, number>) {
  return template.replace(/\{(\w+)\}/gu, (_, key: string) => String(values[key] ?? ""));
}
export function xResultSummary(result: XDownloadResult, copy: Catalog): string {
  if (result.state === "partial") return xMessage(copy.xPartialSummary, { completed: result.completedCount, failed: result.failedCount });
  if (result.state === "failed") return xMessage(copy.xFailedSummary, { count: result.failedCount });
  if (result.state === "canceled") return xMessage(copy.xCanceledSummary, { completed: result.completedCount });
  const wholePost = !result.contentChanged && result.unavailableCount === 0 && result.completedCount === result.postItemCount;
  return xMessage(wholePost ? copy.xWholeCompletedSummary : copy.xSelectedSavedSummary, { count: result.completedCount });
}
