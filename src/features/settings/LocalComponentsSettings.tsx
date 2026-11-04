import type { ToolInventory, ToolState } from "../../shared/contracts/managedTools";
import type { ToolProvisioningErrorCode, ToolProvisioningStatus } from "../../shared/contracts/toolProvisioning";
import type { Catalog } from "../../shared/i18n/catalog";
import {TikTokComponentStatus} from "./TikTokComponentStatus";

interface LocalComponentsSettingsProps {
  copy: Catalog;
  status: ToolProvisioningStatus | null;
  inventory: ToolInventory | null | undefined;
  repairPending: boolean;
  repairError?: ToolProvisioningErrorCode | null;
  onRepair: () => void;
}

function stateLabel(state: ToolState, copy: Catalog): string {
  return state === "ready"
    ? copy.componentReady
    : state === "missing"
      ? copy.componentMissing
      : copy.componentNeedsRepair;
}

function repairErrorLabel(error: ToolProvisioningErrorCode | null | undefined, copy: Catalog) {
  if (error === "busy") return copy.repairBusy;
  if (error === "download_in_progress") return copy.repairDownloadInProgress;
  return error ? copy.repairFailed : null;
}

export function LocalComponentsSettings({
  copy,
  status,
  inventory,
  repairPending,
  repairError,
  onRepair,
}: LocalComponentsSettingsProps) {
  const disabled = repairPending
    || status === null
    || status.state === "preparing"
    || status.state === "repairing";
  const errorLabel = repairErrorLabel(repairError, copy);
  return (
    <section className="local-components" aria-labelledby="local-components-title">
      <div>
        <h2 id="local-components-title">{copy.localComponentsTitle}</h2>
      </div>
      {inventory === null ? <p>{status?.state === "failed" ? copy.componentsUnavailable : copy.componentsLoading}</p> : inventory === undefined ? <p>{copy.componentsUnavailable}</p> : (
        <ul className="local-component-list">
          {inventory.tools.map((tool) => (
            <li key={tool.name}>
              <span>{tool.name}</span>
              <span>{tool.version ?? stateLabel(tool.state, copy)}</span>
              <span className={tool.state === "ready" ? "component-state ready" : "component-state"}>{stateLabel(tool.state, copy)}</span>
            </li>
          ))}
        </ul>
      )}
      {errorLabel ? <p className="component-repair-error" role="status">{errorLabel}</p> : null}
      {status?.state==="ready"?<TikTokComponentStatus copy={copy} />:null}
      <button type="button" disabled={disabled} onClick={onRepair}>
        {repairPending || status?.state === "repairing" ? copy.repairingComponents : copy.verifyAndRepair}
      </button>
    </section>
  );
}
