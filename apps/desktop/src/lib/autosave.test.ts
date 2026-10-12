import assert from "node:assert/strict";
import { test } from "node:test";

import {
  autosave,
  begin,
  edited,
  external,
  failed,
  landed,
  quiet,
  shown,
  status,
} from "@/lib/autosave";

test("an edit is dirty, saves, then shows Saved until it goes quiet", () => {
  let state = edited(autosave("old"), "new");
  assert.equal(status(state), "dirty");
  assert.equal(shown(state), "new");
  let value: string | null;
  [state, value] = begin(state);
  assert.equal(value, "new");
  assert.equal(status(state), "saving");
  assert.equal(shown(state), "new");
  state = landed(state);
  assert.equal(status(state), "saved");
  assert.equal(state.saved, "new");
  assert.equal(status(quiet(state)), "idle");
});

test("typing back what's saved leaves nothing to save", () => {
  const state = edited(edited(autosave("same"), "changed"), "same");
  assert.equal(status(state), "idle");
  assert.deepEqual(begin(state)[1], null);
});

test("typing while a save is on its way keeps the new edit waiting", () => {
  let [state] = begin(edited(autosave(""), "one"));
  state = edited(state, "one two");
  assert.deepEqual(begin(state)[1], null, "one save at a time");
  state = landed(state);
  assert.equal(state.saved, "one");
  assert.equal(status(state), "dirty");
  assert.equal(shown(state), "one two");
  assert.equal(begin(state)[1], "one two");
});

test("a failed save is an edit again", () => {
  let [state] = begin(edited(autosave("a"), "b"));
  state = failed(state);
  assert.equal(status(state), "dirty");
  assert.equal(shown(state), "b");
  assert.equal(state.saved, "a");
});

test("a value saved elsewhere shows only while the box has no edit of its own", () => {
  assert.equal(shown(external(autosave("a"), "b")), "b");
  assert.equal(shown(external(edited(autosave("a"), "mine"), "b")), "mine");
});
