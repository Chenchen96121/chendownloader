import type { BilibiliAnalysisPreview } from "../../shared/contracts/bilibili";
import { sample } from "../analyze/selectionTestFixtures";

// Synthetic public DTO only: not a real BV/av/CID relationship or live-site evidence.
export const biliSample: BilibiliAnalysisPreview = {
  ...sample, analysisId: "17", part: 2,
  media: { ...sample.media, platform: "bilibili", mediaId: "BV1xx411c7mD_p2", title: "Bilibili <P2>" },
};
export function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
