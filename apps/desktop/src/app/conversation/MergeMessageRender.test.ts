import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Seen = {
  proofs: { kind: string; image: string | null; video: string | null; caption: string }[];
  inCard: boolean;
  title: string;
  beforeChanges: boolean;
  thumbnail: [number, number] | null;
  underAnswer: boolean;
  rows: number;
  read: string[];
  checkedInThread: boolean;
  headers: string[];
  card: { title: string; merged: string; buttons: string[]; rowWords: string[] } | null;
  warning: string[];
  tab: string | null;
  reviews: string[];
  revert: { shown: string; sent: string[]; after: string[] };
};

const render = async (t: Parameters<typeof renderFixturePage>[0], query = "") =>
  JSON.parse(await renderFixturePage(t, `merge-message.html?drive=1${query}`, "merge-message-result", 15000)) as Seen;

// The proofs the thread attached show under the answer inside its Preview card, before the
// Changes card: captioned thumbnails two to a row; no row of their own in the work.
test("a turn's proofs show captioned in its Preview card under its answer", { timeout: 60000 }, async (t) => {
  const seen = await render(t);
  assert.deepEqual(seen.proofs.map(({ caption, ...rest }) => ({ ...rest, caption: caption.replace(/\d{1,2}:\d{2}.*$/, "").trim() })), [
    { kind: "image", image: "The install section, fixed", video: null, caption: "1.The install section, fixed" },
    { kind: "video", image: null, video: "data:video/webm;", caption: "2.install.webm" },
  ]);
  assert.equal(seen.inCard, true);
  assert.equal(seen.title, "Screenshots and recordings");
  assert.equal(seen.beforeChanges, true);
  assert.equal(seen.thumbnail?.[1], 178);
  assert.equal(seen.underAnswer, true);
  assert.equal(seen.rows, 0);
  assert.deepEqual(seen.read, ["proof-shot", "proof-video"]);
});

// What was checked never lists in the thread, and nothing is wrong here: no footer row. A turn
// that merged with no files of its own counted shows the merge's own Changes card, Revert in it;
// Revert asks in the card's footer, then sends the thread its fixed request once.
test("a merge with nothing counted shows its own Changes card with Revert, and no Checked list", { timeout: 60000 }, async (t) => {
  const seen = await render(t);
  assert.equal(seen.checkedInThread, false);
  assert.deepEqual(seen.warning, []);
  assert.deepEqual(seen.headers, ["Merged into main (PR #7)"]);
  assert.equal(seen.card, null);
  assert.deepEqual(seen.revert, {
    shown: "Revert",
    sent: ["Revert merge f00dfee (PR #7) on main and push it the way it went in"],
    after: ["Reverting…"],
  });
});

// A turn that edited files and merged says so once, in its Changes card's header, with Revert and
// View changes there (no Discard) and no "Merged" on its rows.
test("the Changes card's header carries the merge, Revert and View changes", { timeout: 60000 }, async (t) => {
  const seen = await render(t, "&edited=1");
  assert.deepEqual(seen.card, { title: "Edited 2 files", merged: "Merged into main (PR #7)", buttons: ["Revert", "View changes"], rowWords: [] });
  assert.deepEqual(seen.headers, []);
});

// Merged by the next request, the turn that edited the files still says they merged, with Revert;
// the merging turn shows the merge's own card.
test("a merge in a later turn shows on the edited turn's card and in its own turn", { timeout: 60000 }, async (t) => {
  const seen = await render(t, "&later=1");
  assert.equal(seen.card?.merged, "Merged into main (PR #7)");
  assert.deepEqual(seen.card?.buttons, ["Revert", "View changes"]);
  assert.deepEqual(seen.headers, ["Merged into main (PR #7)"]);
  assert.deepEqual(seen.warning, []);
});

// Something really wrong with the merged code is the Changes card's footer row, which opens the
// Progress tab, which lists each.
test("an open finding and a failed CI run read as the card's footer row, which opens the Progress tab", { timeout: 60000 }, async (t) => {
  const seen = await render(t, "&warn=1");
  assert.deepEqual(seen.warning, ["One review finding is still open and CI failed"]);
  assert.equal(seen.tab, "panel:reviews");
  // There, under Merging, only what is really wrong: no stale checks, no proofs line.
  assert.deepEqual(seen.reviews, ["! Codex review of 0000aaa..9999bbb: one finding still open", "! CI failed on a1b2c3d"]);
});
