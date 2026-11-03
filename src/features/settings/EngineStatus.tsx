import type { ToolProvisioningStatus } from "../../shared/contracts/toolProvisioning";
import type { Locale } from "../../shared/i18n/locale";
import { engineStatusLabel } from "./toolStatus";

export function EngineStatus({
  locale,
  status,
}: {
  locale: Locale;
  status: ToolProvisioningStatus | null | undefined;
}) {
  return <p className="engine-status">{engineStatusLabel(status, locale)}</p>;
}
