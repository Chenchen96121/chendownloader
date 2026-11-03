export interface PasteRequestState { request: number; revision: number; pending: boolean }
export interface PasteTicket { request: number; revision: number }
export function createPasteState(): PasteRequestState { return {request: 0, revision: 0, pending: false}; }
export function beginPaste(state: PasteRequestState): {state: PasteRequestState; ticket: PasteTicket} | null {
  if (state.pending) return null;
  const next = {...state, request: state.request + 1, pending: true};
  return {state: next, ticket: {request: next.request, revision: next.revision}};
}
export function invalidatePaste(state: PasteRequestState): PasteRequestState {
  return {...state, request: state.request + 1, revision: state.revision + 1, pending: false};
}
export function finishPaste(state: PasteRequestState, ticket: PasteTicket, available: boolean): {state: PasteRequestState; apply: boolean} {
  if (!state.pending || state.request !== ticket.request || state.revision !== ticket.revision) return {state, apply: false};
  return {state: {...state, pending: false}, apply: available};
}
