import assert from "node:assert/strict";
import { test } from "node:test";

import { changeNotes, markedName, markerNumber, mockupNotesOf, notePlacement, notesMessage, percent, pointIn, sendEnded, takeNotes } from "./mockupNotes";

test("a click is kept as fractions of the image, inside it", () => {
  const size = { width: 200, height: 100 };
  assert.deepEqual(pointIn({ x: 50, y: 80 }, size), { x: 0.25, y: 0.8 });
  assert.deepEqual(pointIn({ x: -20, y: 140 }, size), { x: 0, y: 1 });
  assert.equal(percent(0.3), "30%");
  assert.equal(percent(1 / 3), "33.3%");
  assert.equal(percent(1.2), "100%");
});

test("the notes go as one message, an Image block per marked screenshot, numbered as its markers", () => {
  const notes = [
    { id: "a", screen: 2, x: 0.3, y: 0.4, text: " Make this card blue " },
    { id: "b", screen: 1, x: 0.5, y: 0.05, text: "Move it up." },
    { id: "c", screen: 2, x: 0.85, y: 0.85, text: "Taller footer band" },
  ];
  assert.equal(markedName(2), "mockup-2-notes.png");
  assert.equal(markerNumber(notes, notes[2]!), 2);
  assert.equal(markerNumber(notes, notes[1]!), 1);
  assert.equal(
    notesMessage(2, notes),
    "Notes on the mockup (version 2), numbered as the markers on the attached screenshots:\n\n"
    + "Image 1 (mockup-1-notes.png):\n1. (x: 50%, y: 5%) Move it up.\n\n"
    + "Image 2 (mockup-2-notes.png):\n1. (x: 30%, y: 40%) Make this card blue\n2. (x: 85%, y: 85%) Taller footer band",
  );
});

test("a note's box sits right of its marker, else left, else below, else above, inside the image", () => {
  const box = { width: 294, height: 44 };
  const surface = { width: 800, height: 600 };
  assert.deepEqual(notePlacement({ x: 100, y: 300 }, box, surface, 27, 8), { left: 127, top: 278 });
  assert.deepEqual(notePlacement({ x: 700, y: 300 }, box, surface, 27, 8), { left: 379, top: 278 });
  assert.deepEqual(notePlacement({ x: 100, y: 10 }, box, surface, 27, 8), { left: 127, top: 8 });
  const narrow = { width: 400, height: 600 };
  assert.deepEqual(notePlacement({ x: 200, y: 100 }, box, narrow, 27, 8), { left: 53, top: 127 });
  assert.deepEqual(notePlacement({ x: 200, y: 580 }, box, narrow, 27, 8), { left: 53, top: 509 });
});

test("a session's notes are kept apart from another's", () => {
  changeNotes("one", () => ({ commenting: true, notes: [{ id: "a", screen: 1, x: 0, y: 0, text: "x" }] }));
  assert.equal(mockupNotesOf("one").notes.length, 1);
  assert.equal(mockupNotesOf("two").notes.length, 0);
  assert.equal(mockupNotesOf("two").commenting, false);
});

test("one send at a time per session, even when its tab opens again mid-send, and it clears only what it took", () => {
  const first = { id: "a", screen: 1, x: 0.1, y: 0.1, text: "Bluer" };
  changeNotes("sending", () => ({ commenting: true, notes: [first] }));
  const taken = takeNotes("sending");
  assert.deepEqual(taken, [first]);
  // A tab mounted again has no guard of its own: the session's says a send runs.
  assert.equal(mockupNotesOf("sending").sending, true);
  assert.equal(takeNotes("sending"), null);
  // A note made meanwhile (it can't be from the tab, which freezes them) outlives the send.
  const later = { id: "b", screen: 1, x: 0.5, y: 0.5, text: "Wider" };
  changeNotes("sending", (current) => ({ notes: [...current.notes, later] }));
  const sent = { version: 1, shots: ["s1"], dismissed: false };
  sendEnded("sending", taken!, sent);
  assert.deepEqual(mockupNotesOf("sending").notes, [later]);
  assert.equal(mockupNotesOf("sending").sending, false);
  assert.deepEqual(mockupNotesOf("sending").sent, sent);

  // A failed send keeps its notes to try again.
  changeNotes("failing", () => ({ commenting: true, notes: [first] }));
  sendEnded("failing", takeNotes("failing")!, null);
  assert.deepEqual(mockupNotesOf("failing").notes, [first]);
  assert.equal(mockupNotesOf("failing").commenting, true);
  assert.equal(mockupNotesOf("failing").failed, true);
  assert.equal(mockupNotesOf("failing").sending, false);
});
