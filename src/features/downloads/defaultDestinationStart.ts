import type { SelectionIntent } from "../../shared/contracts/selection";
import { approveDefaultDestination } from "../../shared/ipc/destinationSettings";
import { startDownload } from "../../shared/ipc/downloads";

export async function startDownloadInDefaultFolder(analysisId: string, intent: SelectionIntent) {
  const destination = await approveDefaultDestination();
  const started = await startDownload(analysisId, destination.destinationId, intent);
  return { destination, started };
}
