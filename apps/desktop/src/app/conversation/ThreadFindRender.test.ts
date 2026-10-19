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
      rulesBefore: string[]; rulesOpen: string[];
      foldedBefore: string | null; foldedAfter: boolean; bar: { top: number; right: number } | null; maxWidth: string | null;
      placeholder: string | null; value: string | null; focused: boolean; selected: number[] | null; label: string | null;
      size: number; active: string | null; buttons: (string | null)[]; matchColour: number[]; activeColour: number[];
      selectable: (string | null)[];
    };
    composer: { typed: string | null; label: string | null; size: number; focused: boolean; selected: number[] | null };
    keys: { found: Found; enters: (string | null)[]; shiftEnter: string | null; commandG: string | null; shiftCommandG: string | null };
    revealed: { label: string | null; place: Place; moved: number; after: string | null };
    again: { focused: boolean; selected: number[] | null; value: string | null };
    many: Found & { sameMarks: boolean };
    none: Found & { previous: boolean | null; next: boolean | null };
    folded: Found;
    clipped: Found & { before: string | null; after: string | null };
    inner: { label: string | null; inBox: boolean; inView: boolean };
    inButton: { label: string | null; ringed: boolean; left: boolean };
    details: { closed: string | null; opened: string | null; shut: string | null };
    closed: { bar: boolean; finding: boolean; matches: boolean; active: boolean; emptied: (number | null)[]; rules: string[] };
    keepsPlace: { focusBack: boolean; moved: number };
    multiline: { value: string | null; label: string | null; size: number; closedByButton: boolean };
  };
  assert.equal(seen.error, undefined);

  // ⌘F with a word selected in the thread: the bar at the top right (8px down, 16px in, at most
  // 70% of the 1280px window), the word as its text, selected, and its one match marked.
  const { opened } = seen;
  // Find's colours are on the page only while it marks: a `::highlight()` rule slows every style pass.
  assert.deepEqual(opened.rulesBefore, []);
  assert.deepEqual(opened.rulesOpen.toSorted(), ["thread-find", "thread-find-active"]);
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
  // WebKit marks only selectable text (and puts a mark on other text onto the next selectable
  // text): while the thread holds marks, its text is selectable, the app's select-none rows too.
  assert.deepEqual(opened.selectable, ["none", "text"]);

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
  // A new query changes the marks in place: WebKit repaints only the ranges a set gains or loses.
  assert.equal(seen.many.sameMarks, true);

  // A word nowhere: nothing.
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
  // A match in a box that scrolls on its own, out of its sight: the box and the thread bring it into view.
  assert.deepEqual(seen.inner, { label: "1 / 1 result", inBox: true, inView: true });
  // Text in a button is never marked in WebKit: the button holding the match gone to has a ring,
  // taken off when no match in it is gone to.
  assert.deepEqual(seen.inButton, { label: "1 / 1 result", ringed: true, left: false });
  // Opening a report's Details finds what it holds; closing it drops the match again.
  assert.deepEqual(seen.details, { closed: "0 results", opened: "1 / 1 result", shut: "0 results" });
  assert.deepEqual(seen.closed, { bar: false, finding: false, matches: false, active: false, emptied: [0, 0], rules: [] });
  // Esc gives the keyboard back to what had it before, without scrolling the thread to it.
  assert.deepEqual(seen.keepsPlace, { focusBack: true, moved: 0 });

  // A selection over two lines opens the bar with no text; × closes it.
  assert.deepEqual(seen.multiline, { value: "", label: null, size: 0, closedByButton: true });
});

test("Find keeps the keyboard when the composer's editor loads after it opened", { timeout: 180000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-find.html?drive=late-editor", "thread-find-result", 150000)) as {
    error?: string; standIn: boolean; focusedBefore: boolean; editorLoaded: boolean; focusedAfter: boolean;
  };
  assert.equal(seen.error, undefined);
  assert.deepEqual(seen, { standIn: true, focusedBefore: true, editorLoaded: true, focusedAfter: true });
});

type Folds = {
  error?: string;
  before: { wombat: string | null; badger: string | null; mounted: string | null };
  typed: { label: string | null; size: number; wombat: string | null; badger: string | null; kept: string | null; height: number | null };
  entered: { label: string | null; active: string | null; place: Place; wombat: string | null; badger: string | null };
  closed: { wombat: string | null; badger: string | null; badgerFold: string | null };
  shut: { full: boolean; midway: boolean; after: string | null };
};
let folds: Promise<Folds> | null = null;
/** One run of the folds drive, read by both tests below. */
const foldsRun = (t: Parameters<typeof renderFixtureLive>[0]) =>
  (folds ??= renderFixtureLive(t, "thread-find.html?drive=folds", "thread-find-result", 150000).then((text) => JSON.parse(text) as Folds));

test("Find counts a match only in a closed work fold, and Enter opens the fold and goes to it", { timeout: 180000 }, async (t) => {
  const seen = await foldsRun(t);
  assert.equal(seen.error, undefined);
  // Folded when its turn ended, its rows not mounted.
  assert.deepEqual(seen.before, { wombat: "false", badger: "false", mounted: null });
  // Typed: counted, and the fold left closed, its rows kept for Find with no height.
  assert.deepEqual(seen.typed, { label: "1 / 1 result", size: 1, wombat: "false", badger: "false", kept: "closed", height: 0 });
  // Enter: its fold opens, and the match is marked and centred in view.
  assert.equal(seen.entered.label, "1 / 1 result");
  assert.equal(seen.entered.wombat, "true");
  assert.equal(seen.entered.active, "wombat");
  shownCentred(seen.entered.place, "wombat");
});

test("Find leaves the other work folds closed, and closing it keeps the one it opened", { timeout: 180000 }, async (t) => {
  const seen = await foldsRun(t);
  assert.equal(seen.error, undefined);
  assert.equal(seen.entered.badger, "false");
  // Closed: the fold gone to stays open as the user's own choice; the other holds no rows again.
  assert.deepEqual(seen.closed, { wombat: "true", badger: "false", badgerFold: null });
  // Closed by hand afterwards, it animates down from its full height, never jumps, then holds nothing.
  assert.deepEqual(seen.shut, { full: true, midway: true, after: null });
});

test("Find counts a match only in a closed work group inside opened work, and Enter opens the group and goes to it", { timeout: 180000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-find.html?drive=groups", "thread-find-result", 150000)) as {
    error?: string;
    before: { work: string | null; group: string | null; rows: number };
    typed: { label: string | null; active: string | null; group: string | null; height: number | null };
    entered: { label: string | null; active: string | null; place: Place; group: string | null; work: string | null };
    closed: { group: string | null; kept: boolean };
  };
  assert.equal(seen.error, undefined);
  // The turn's work opened by hand, its group closed and showing no rows.
  assert.deepEqual(seen.before, { work: "true", group: "false", rows: 0 });
  // Typed: counted, the group left closed, its rows kept for Find with no height.
  assert.deepEqual(seen.typed, { label: "1 / 1 result", active: "ocelot", group: "false", height: 0 });
  // Enter: the group opens, and the match is marked and centred in view.
  assert.equal(seen.entered.label, "1 / 1 result");
  assert.equal(seen.entered.group, "true");
  assert.equal(seen.entered.work, "true");
  assert.equal(seen.entered.active, "ocelot");
  shownCentred(seen.entered.place, "ocelot");
  // Closed: the group gone to stays open, and nothing is kept for Find any more.
  assert.deepEqual(seen.closed, { group: "true", kept: false });
});

test("Find counts matches in pages not loaded without loading them; a step to one loads down to it and goes there", { timeout: 180000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-find.html?drive=older", "thread-find-result", 150000)) as {
    error?: string;
    kiwi: { label: string | null; active: string | null; turn: number; loads: number };
    back: { first: string | null; label: string | null; active: string | null; turn: number; place: Place; loads: number };
    quokka: { first: string | null; label: string | null; active: string | null; shown: boolean; loads: number };
    entered: { label: string | null; active: string | null; turn: number; place: Place; loads: number };
  };
  assert.equal(seen.error, undefined);
  // One kiwi loaded, one in a page not loaded: both counted, the loaded one gone to, nothing loaded.
  assert.deepEqual(seen.kiwi, { label: "2 / 2 results", active: "kiwi", turn: 46, loads: 0 });
  // ⇧Enter: the page holding the other loads, and it is gone to, centred.
  assert.equal(seen.back.label, "1 / 2 results");
  assert.equal(seen.back.active, "kiwi");
  assert.equal(seen.back.turn, 18);
  assert.equal(seen.back.loads, 1);
  shownCentred(seen.back.place, "older kiwi");
  // A word only in the oldest page: counted, nothing loaded or marked while typing.
  assert.deepEqual(seen.quokka, { first: seen.quokka.first, label: "1 / 1 result", active: null, shown: false, loads: 1 });
  // Enter: the pages down to it load, and it is gone to, centred.
  assert.equal(seen.entered.label, "1 / 1 result");
  assert.equal(seen.entered.active, "quokka");
  assert.equal(seen.entered.turn, 4);
  assert.equal(seen.entered.loads, 4);
  shownCentred(seen.entered.place, "quokka");
});

test("Find stepping back into an older page keeps the counted match past 250 results", { timeout: 180000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-find.html?drive=older-capped", "thread-find-result", 150000));
  assert.equal(seen.error, undefined);
  assert.deepEqual(seen.before, { label: "11 / 250+ results", turn: 21, occurrence: 1, loads: 0, size: 240 }, JSON.stringify(seen));
  assert.deepEqual(seen.back, { label: "10 / 250+ results", turn: 20, occurrence: 60, loads: 1, size: 250 });
  assert.deepEqual(seen.previous, { label: "9 / 250+ results", turn: 20, occurrence: 59, loads: 1, size: 250 });
  assert.deepEqual(seen.next, seen.back);
});
