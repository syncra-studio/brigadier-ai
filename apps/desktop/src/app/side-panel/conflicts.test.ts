import assert from "node:assert/strict";
import { test } from "node:test";

import {
  acceptConflict,
  conflictCount,
  editorSegments,
  mergedResult,
  parseConflicts,
  toggleSide,
  UNTOUCHED,
  type Conflict,
} from "@/app/side-panel/conflicts";

const MARKED = [
  "top\n",
  "<<<<<<< HEAD\n",
  "mine\n",
  "=======\n",
  "theirs\n",
  ">>>>>>> feature\n",
  "middle\n",
  "<<<<<<< HEAD\n",
  "mine 2\n",
  "=======\n",
  "theirs 2\n",
  ">>>>>>> feature\n",
  "end",
].join("");

test("a marked file reads as its plain parts and its conflicts, labels and all", () => {
  const segments = parseConflicts(MARKED);
  assert.equal(conflictCount(segments), 2);
  const first = segments[1] as Conflict;
  assert.deepEqual(
    [first.current, first.incoming, first.base, first.currentLabel, first.incomingLabel],
    [["mine\n"], ["theirs\n"], null, "HEAD", "feature"],
  );
  assert.deepEqual(segments[0], { type: "common", lines: ["top\n"] });
  assert.deepEqual(segments[4], { type: "common", lines: ["end"] });
  // Markers that never close are plain text.
  assert.equal(conflictCount(parseConflicts("a\n<<<<<<< HEAD\nb\n=======\nc\n")), 0);
});

test("Accept Current, Incoming and Both replace only their own conflict", () => {
  assert.equal(
    acceptConflict(MARKED, 0, "current"),
    MARKED.replace("<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> feature\n", "mine\n"),
  );
  const incoming = acceptConflict(MARKED, 1, "incoming");
  assert.ok(incoming.endsWith("middle\ntheirs 2\nend"), incoming);
  assert.ok(incoming.includes("<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> feature\n"), "the first stays as git marked it");
  const both = acceptConflict(acceptConflict(MARKED, 0, "both"), 0, "both");
  assert.equal(both, "top\nmine\ntheirs\nmiddle\nmine 2\ntheirs 2\nend");
  // Windows line endings stay.
  assert.equal(acceptConflict("<<<<<<< HEAD\r\na\r\n=======\r\nb\r\n>>>>>>> x\r\n", 0, "incoming"), "b\r\n");
});

test("the merge editor's result starts from the common version and takes the sides as picked", () => {
  const merged = "a\n<<<<<<< Current\nmain\n||||||| Base\nbase\n=======\nfeature\n>>>>>>> Incoming\nc\n";
  const segments = parseConflicts(merged);
  const conflict = segments[1] as Conflict;
  assert.deepEqual(conflict.base, ["base\n"]);
  assert.equal(conflict.baseLabel, "Base");
  assert.equal(mergedResult(segments, [UNTOUCHED]), "a\nbase\nc\n");
  const incoming = toggleSide(UNTOUCHED, "incoming");
  assert.deepEqual(incoming, { order: ["incoming"], handled: true });
  assert.equal(mergedResult(segments, [incoming]), "a\nfeature\nc\n");
  // Accepting the other side too combines them in the order accepted.
  assert.equal(mergedResult(segments, [toggleSide(incoming, "current")]), "a\nfeature\nmain\nc\n");
  // Accept Combination (Current First).
  assert.equal(mergedResult(segments, [toggleSide(UNTOUCHED, "current", true)]), "a\nmain\nfeature\nc\n");
  // Remove Incoming leaves it handled, back to the common version.
  const removed = toggleSide(incoming, "incoming");
  assert.deepEqual(removed, { order: [], handled: true });
  assert.equal(mergedResult(segments, [removed]), "a\nbase\nc\n");
  // A conflict written back keeps its markers and labels.
  assert.equal(acceptConflict(merged + merged, 1, "current"), merged + "a\nmain\nc\n");
});

test("markers as long as .gitattributes sets them read and write back at that length", () => {
  const long = "<<<<<<<<<< HEAD\nmine\n=======\nstill mine\n==========\ntheirs\n>>>>>>>>>> feature\nend\n";
  const segments = parseConflicts(long);
  assert.equal(conflictCount(segments), 1);
  const conflict = segments[0] as Conflict;
  assert.deepEqual([conflict.current, conflict.incoming, conflict.size], [["mine\n", "=======\n", "still mine\n"], ["theirs\n"], 10]);
  assert.equal(acceptConflict(long, 0, "incoming"), "theirs\nend\n");
  // An untouched conflict keeps its own marker length.
  assert.equal(acceptConflict(long + long, 1, "current"), long + "mine\n=======\nstill mine\nend\n");
  // A closing marker of another length doesn't close it, nor does a longer run of < open one.
  assert.equal(conflictCount(parseConflicts("<<<<<<< HEAD\na\n==========\nb\n>>>>>>>>>> x\n")), 0);
  assert.equal(conflictCount(parseConflicts("<<<<<<<< not a marker\n")), 0);
});

test("the merge editor keeps the working file's edits and takes each conflict's common version from git", () => {
  const working = "edited top\n<<<<<<< HEAD\nmain\n=======\nfeature\n>>>>>>> feature\nc\n";
  const merged = "a\n<<<<<<< Current\nmain\n||||||| Base\nbase\n=======\nfeature\n>>>>>>> Incoming\nc\n";
  const segments = editorSegments(working, merged)!;
  assert.equal(mergedResult(segments, [UNTOUCHED]), "edited top\nbase\nc\n");
  assert.equal(mergedResult(segments, [toggleSide(UNTOUCHED, "incoming")]), "edited top\nfeature\nc\n");
  // Once a conflict was accepted in place the two no longer pair: no merge editor.
  assert.equal(editorSegments(acceptConflict(working + working, 0, "current"), merged + merged), null);
  assert.equal(editorSegments("no conflicts\n", merged), null);
});
