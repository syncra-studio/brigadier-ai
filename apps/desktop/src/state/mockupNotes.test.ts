import assert from "node:assert/strict";
import { test } from "node:test";

import { markedName, notesMessage, regionFrom } from "./mockupNotes";

test("a dragged region is kept as fractions of the image, whatever the direction, inside it", () => {
  const size = { width: 200, height: 100 };
  assert.deepEqual(regionFrom({ x: 150, y: 80 }, { x: 50, y: 20 }, size), { x: 0.25, y: 0.2, width: 0.5, height: 0.6000000000000001 });
  assert.deepEqual(regionFrom({ x: -20, y: 50 }, { x: 260, y: 140 }, size), { x: 0, y: 0.5, width: 1, height: 0.5 });
  assert.equal(regionFrom({ x: 10, y: 10 }, { x: 13, y: 14 }, size), null, "a click marks nothing");
  assert.notEqual(regionFrom({ x: 10, y: 10 }, { x: 10, y: 30 }, size), null, "a tall thin drag counts");
});

test("the notes go as one numbered message naming the marked screenshots", () => {
  const region = { x: 0, y: 0, width: 0.1, height: 0.1 };
  assert.equal(markedName(2), "mockup-2-notes.png");
  assert.equal(
    notesMessage("Density toggle", 1, [{ id: "a", screen: 1, region, text: " Move it up. " }]),
    'Changes to the mockup "Density toggle" (version 1):\n1. Move it up.\n\nThe numbered areas are marked on mockup-1-notes.png.',
  );
  assert.equal(
    notesMessage("Density toggle", 2, [
      { id: "a", screen: 2, region, text: "Smaller label." },
      { id: "b", screen: 1, region, text: "Move it up." },
    ]),
    'Changes to the mockup "Density toggle" (version 2):\n1. Screen 2: Smaller label.\n2. Screen 1: Move it up.\n\nThe numbered areas are marked on mockup-1-notes.png, mockup-2-notes.png.',
  );
});
