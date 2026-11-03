import {expect, it} from "vitest";
import {beginPaste, createPasteState, invalidatePaste, finishPaste} from "./pasteState";

it("allows only one pending click and applies a current result exactly once", () => {
  const begun = beginPaste(createPasteState());
  expect(begun).not.toBeNull();
  expect(beginPaste(begun!.state)).toBeNull();
  const done = finishPaste(begun!.state, begun!.ticket, true);
  expect(done.apply).toBe(true);
  expect(done.state.pending).toBe(false);
  expect(finishPaste(done.state, begun!.ticket, true).apply).toBe(false);
});
it("ignores responses after same-value edits, parse, unmount, or input lock", () => {
  for (const available of [true, false]) {
    const old = beginPaste(createPasteState())!;
    const edited = invalidatePaste(old.state);
    const newer = beginPaste(edited)!;
    expect(finishPaste(edited, old.ticket, available).apply).toBe(false);
    expect(finishPaste(newer.state, old.ticket, available).state).toBe(newer.state);
    expect(finishPaste(newer.state, newer.ticket, false).apply).toBe(false);
  }
});
