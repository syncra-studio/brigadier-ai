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
      mountedBefore: number; bar: { top: number; right: number } | null; maxWidth: string | null;
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
  // The thread mounted only the messages near the view, not the 80 loaded.
  assert.ok(opened.mountedBefore > 0 && opened.mountedBefore < 40, `mounted: ${opened.mountedBefore}`);
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
  // Marked: those in the messages the thread mounts.
  assert.ok(keys.found.size >= 1 && keys.found.size <= 7, `marked: ${keys.found.size}`);
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
  assert.ok(seen.many.size >= 1 && seen.many.size <= 250, `marked: ${seen.many.size}`);
  // A new query changes the marks in place: WebKit repaints only the ranges a set gains or loses.
  assert.equal(seen.many.sameMarks, true);

  // A word nowhere: nothing.
  assert.equal(seen.none.label, "0 results");
  assert.equal(seen.none.size, 0);
  assert.equal(seen.none.active, null);
  assert.equal(seen.none.previous, true);
  assert.equal(seen.none.next, true);

  // A word only in a turn far above the view, not mounted, is found and gone to.
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

// #569: a folded code block mounts only the lines it shows, the rest while Find has a text, so a
// word on a hidden line is still found.
test("Find counts a match on a folded code block's hidden lines, which mount only while it searches", { timeout: 180000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-find.html?drive=code", "thread-find-result", 150000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined);
  assert.deepEqual(seen.before, { lines: 11, shown: false });
  assert.deepEqual(seen.typed, { label: "1 / 1 result", lines: 30, more: "Show more· lines 11–30" });
  assert.deepEqual(seen.closed, { lines: 11 });
});

type Landed = { label: string | null; tag: string | null; turn: number; place: Place };

test("Find counts matches in messages far from those the thread mounts, and each step lands on that very match", { timeout: 180000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-find.html?drive=far", "thread-find-result", 150000)) as {
    error?: string; mounted: { rows: number; of: number }; counted: Landed; forward: Landed[]; back: Landed[];
  };
  assert.equal(seen.error, undefined);
  // 200 turns: the thread mounts a few of their 400 messages.
  assert.equal(seen.mounted.of, 400);
  assert.ok(seen.mounted.rows > 0 && seen.mounted.rows < 40, `mounted: ${seen.mounted.rows}`);
  // Match n is "lynx-" and the nth letter, in turn 4 (a, b, c), 101 (d) and 191 (e).
  const turns = [4, 4, 4, 101, 191];
  // A match in another message is centred; the next one in the same message, already in view, stays put.
  let turn: number | null = null;
  const lands = (landed: Landed, nth: number, label: string) => {
    assert.equal(landed.label, `${nth} / 5 results`, label);
    assert.equal(landed.tag, `-${"abcde"[nth - 1]}`, `${label}: the match gone to`);
    assert.equal(landed.turn, turns[nth - 1], `${label}: its turn`);
    if (landed.turn !== turn) shownCentred(landed.place, label);
    else assert.equal(landed.place?.inView, true, `${label}: in view`);
    turn = landed.turn;
  };
  // All five counted, and the one typing went to marked and shown.
  const first = at(seen.counted.label);
  lands(seen.counted, first, "typed");
  // Enter goes on round all five, ⇧Enter back round them, each mounted and gone to.
  seen.forward.forEach((landed, index) => lands(landed, ((first + index) % 5) + 1, `Enter ${index + 1}`));
  const last = ((first + 4) % 5) + 1;
  seen.back.forEach((landed, index) => lands(landed, ((last - 2 - index + 10) % 5) + 1, `⇧Enter ${index + 1}`));
});
