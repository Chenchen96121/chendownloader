import type {
  ToolProvisioningErrorCode,
  ToolProvisioningStatus,
} from "../../shared/contracts/toolProvisioning";

export const MAX_PROVISIONING_READS = 240;

export interface ProvisioningViewState {
  status: ToolProvisioningStatus | null;
  readPending: boolean;
  readCount: number;
  repairPending: boolean;
  repairError: ToolProvisioningErrorCode | null;
}

export function createProvisioningState(): ProvisioningViewState {
  return {
    status: null,
    readPending: false,
    readCount: 0,
    repairPending: false,
    repairError: null,
  };
}

export function beginProvisioningRead(state: ProvisioningViewState): ProvisioningViewState {
  if (state.readPending || state.readCount >= MAX_PROVISIONING_READS) return state;
  return { ...state, readPending: true, readCount: state.readCount + 1 };
}

export function receiveProvisioningStatus(
  state: ProvisioningViewState,
  status: ToolProvisioningStatus,
): ProvisioningViewState {
  return {
    ...state,
    status,
    readPending: false,
    repairPending: status.state === "repairing" ? state.repairPending : false,
    repairError: null,
  };
}

export function beginRepair(state: ProvisioningViewState): ProvisioningViewState {
  if (
    state.repairPending
    || state.status === null
    || state.status.state === "preparing"
    || state.status.state === "repairing"
  ) return state;
  return {
    ...state,
    status: { state: "repairing" },
    repairPending: true,
    repairError: null,
  };
}

export function failRepair(
  state: ProvisioningViewState,
  error: ToolProvisioningErrorCode,
): ProvisioningViewState {
  return { ...state, repairPending: false, repairError: error };
}

export function shouldPollProvisioning(state: ProvisioningViewState): boolean {
  return state.readCount < MAX_PROVISIONING_READS
    && (state.status === null
      || state.status.state === "preparing"
      || state.status.state === "repairing");
}

export function shouldShowWorkspace(state: ProvisioningViewState): boolean {
  return state.status?.state === "ready";
}
