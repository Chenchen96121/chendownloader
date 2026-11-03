import type { ToolProvisioningStatus } from "../../shared/contracts/toolProvisioning";
import type { Catalog } from "../../shared/i18n/catalog";
import { Icon } from "../../shared/ui/Icon";

interface StartupGateProps {
  status: ToolProvisioningStatus | null;
  copy: Catalog;
}

export function StartupGate({ status, copy }: StartupGateProps) {
  const state = status?.state ?? "preparing";
  const message = state === "ready"
    ? copy.componentsReady
    : state === "repairing"
      ? copy.componentsRepairing
      : state === "failed"
        ? copy.componentsFailed
        : copy.componentsPreparing;
  return (
    <section className={`startup-gate startup-gate--${state}`} aria-live="polite" aria-busy={state === "preparing" || state === "repairing"}>
      <span className="startup-gate-mark" aria-hidden="true"><Icon name={state === "failed" ? "warning" : "loader"} size={26} /></span>
      <h2>{message}</h2>
    </section>
  );
}
