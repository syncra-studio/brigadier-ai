import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Place = { text: string; inView: boolean; centreOff: number } | null;
type Found = { label: string | null; size: number; active: string | null; place: Place };

/** "3 / 7 results" → 3. */
const at = (label: string | null) => Number(/^(\d+) \//.exec(label ?? "")?.[1] ?? Number.NaN);

function shownCentred(place: Place, label: string): void {
  assert.ok(place, `${label}: a match is gone to`);
  assert.equal(place.inView, true, `${label}: in view`);
  assert.ok(Math.abs(place.centreOff) <= 2, `${label}: off the view's centre by ${place.centreOff}`);
}

test("Find in the chat counts, marks and goes to matches in what the thread shows", { timeout: 180000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-find.html?drive=1", "thread-find-result", 150000)) as {
    error?: string;
    opened: {
      foldedBefore: string | null; foldedAfter: boolean; bar: { top: number; right: number } | null; maxWidth: string | null;
      placeholder: string | null; value: string | null; focused: boolean; selected: number[] | null; label: string | null;
      size: number; active: string | null; buttons: (string | null)[]; matchColour: number[]; activeColour: number[];
    };
    composer: { typed: string | null; label: string | null; size: number; focused: boolean; selected: number[] | null };
    keys: { found: Found; enters: (string | null)[]; shiftEnter: string | null; commandG: string | null; shiftCommandG: string | null };
    revealed: { label: string | null; place: Place; moved: number; after: string | null };
    again: { focused: boolean; selected: number[] | null; value: string | null };
    many: Found;
    none: Found & { previous: boolean | null; next: boolean | null };
    folded: Found;
    clipped: Found & { before: string | null; after: string | null };
    closed: { bar: boolean; matches: boolean; active: boolean };
    multiline: { value: string | null; label: string | null; size: number; closedByButton: boolean };
  };
  assert.equal(seen.error, undefined);

  // ⌘F with a word selected in the thread: the bar at the top right (8px down, 16px in, at most
  // 70% of the 1280px window), the word as its text, selected, and its one match marked.
  const { opened } = seen;
  assert.deepEqual(opened.bar, { top: 8, right: 16 });
  assert.equal(opened.maxWidth, `${1280 * 0.7}px`);
  assert.equal(opened.placeholder, "Search chat…");
  assert.equal(opened.value, "solitary");
  assert.equal(opened.focused, true);
  assert.deepEqual(opened.selected, [0, 8]);
  assert.equal(opened.label, "1 / 1 result");
  assert.equal(opened.size, 1);
  assert.equal(opened.active, "solitary");
  assert.deepEqual(opened.buttons, ["Previous result", "Next result", "Close find"]);
  // The turns folded behind "10 earlier messages" opened with the bar.
  assert.equal(opened.foldedBefore, "10 earlier messages");
  assert.equal(opened.foldedAfter, false);
  // Every match a see-through yellow, the one gone to a stronger orange.
  const [mr = 0, mg = 0, mb = 0, ma = 0] = opened.matchColour;
  assert.ok(mr > mb + 80 && mg > mb + 80 && ma > 60 && ma < 140, `yellow: ${opened.matchColour.join(",")}`);
  const [ar = 0, ag = 0, ab = 0, aa = 0] = opened.activeColour;
  assert.ok(ar > ag + 40 && ag > ab && aa > 180, `orange: ${opened.activeColour.join(",")}`);

  // The composer's text is never searched; ⌘F from it takes the focus back to the bar's text.
  const { composer } = seen;
  assert.match(composer.typed ?? "", /solitary/);
  assert.equal(composer.label, "1 / 1 result");
  assert.equal(composer.size, 1);
  assert.equal(composer.focused, true);
  assert.deepEqual(composer.selected, [0, 8]);

  // Seven matches: Enter steps through all of them, round from the last to the first; ⇧Enter,
  // ⌘G and ⇧⌘G step back, on and back.
  const { keys } = seen;
  assert.match(keys.found.label ?? "", /^[1-7] \/ 7 results$/);
  assert.equal(keys.found.size, 7);
  assert.equal(keys.found.active, "needle");
  const first = at(keys.found.label);
  assert.deepEqual(keys.enters.map(at), Array.from({ length: 7 }, (_, index) => ((first + index) % 7) + 1));
  assert.equal(keys.enters.at(-1), keys.found.label);
  assert.equal(at(keys.shiftEnter), ((first + 5) % 7) + 1);
  assert.equal(at(keys.commandG), first);
  assert.equal(at(keys.shiftCommandG), ((first + 5) % 7) + 1);

  // The first needle, far above: it comes into view centred between the bar and the composer,
  // and a new message below no longer moves the view.
  const { revealed } = seen;
  assert.equal(revealed.label, "1 / 7 results");
  shownCentred(revealed.place, "first needle");
  assert.equal(revealed.place?.text, "needle");
  assert.equal(revealed.moved, 0);
  assert.equal(revealed.after, "1 / 7 results");

  // ⌘F again: the field has the focus, its text selected.
  assert.deepEqual(seen.again, { focused: true, selected: [0, 6], value: "needle" });

  // 280 "tick"s loaded: counting stops at 250.
  assert.match(seen.many.label ?? "", /^\d+ \/ 250\+ results$/);
  assert.equal(seen.many.size, 250);

  // A word only in a page never loaded: nothing.
  assert.equal(seen.none.label, "0 results");
  assert.equal(seen.none.size, 0);
  assert.equal(seen.none.active, null);
  assert.equal(seen.none.previous, true);
  assert.equal(seen.none.next, true);

  // A word only in a turn that was folded away is found.
  assert.equal(seen.folded.label, "1 / 1 result");
  assert.equal(seen.folded.active, "zephyr");
  shownCentred(seen.folded.place, "zephyr");

  // A word in a long message's clipped tail counts, and going to it opens the message.
  assert.equal(seen.clipped.before, "false");
  assert.equal(seen.clipped.label, "1 / 1 result");
  assert.equal(seen.clipped.after, "true");
  shownCentred(seen.clipped.place, "marmalade");

  // Esc closes the bar and clears the marks.
  assert.deepEqual(seen.closed, { bar: false, matches: false, active: false });

  // A selection over two lines opens the bar with no text; × closes it.
  assert.deepEqual(seen.multiline, { value: "", label: null, size: 0, closedByButton: true });
});
