import type { ToolState } from "../../shared/contracts/managedTools";
import type { ToolProvisioningStatus } from "../../shared/contracts/toolProvisioning";
import { catalogs } from "../../shared/i18n/catalog";
import type { Locale } from "../../shared/i18n/locale";

export function engineStatusLabel(status: ToolProvisioningStatus | null | undefined, locale: Locale): string {
  const copy = catalogs[locale];
  return status === null ? copy.engineLoading : status === undefined ? copy.engineUnavailable
    : status.state === "ready" ? copy.engineReady
      : status.state === "preparing" ? copy.componentsPreparing
        : status.state === "repairing" ? copy.componentsRepairing : copy.engineNotReady;
}

export function toolStateLabel(state: ToolState, locale: Locale): string {
  const copy = catalogs[locale];
  switch (state) {
    case "ready":
      return copy.componentReady;
    case "missing":
      return copy.componentMissing;
    case "integrity_failed":
      return copy.componentNeedsRepair;
  }
}
