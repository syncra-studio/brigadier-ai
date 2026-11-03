import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Seen = {
  proofs: { kind: string; image: string | null; video: string | null; caption: string }[];
  inCard: boolean;
  title: string;
  thumbnail: [number, number] | null;
  underAnswer: boolean;
  rows: number;
  read: string[];
  checkedInThread: boolean;
};

const render = async (t: Parameters<typeof renderFixturePage>[0], query = "") =>
  JSON.parse(await renderFixturePage(t, `merge-message.html?drive=1${query}`, "merge-message-result", 15000)) as Seen;

// The proofs the thread attached show under the answer inside its Preview card: captioned thumbnails two to a row; no row of their own in the work.
test("a turn's proofs show captioned in its Preview card under its answer", { timeout: 60000 }, async (t) => {
  const seen = await render(t, "&edited=1");
  assert.deepEqual(seen.proofs.map(({ caption, ...rest }) => ({ ...rest, caption: caption.replace(/\d{1,2}:\d{2}.*$/, "").trim() })), [
    { kind: "screenshot", image: "The install section, fixed", video: null, caption: "1.The install section, fixed" },
    { kind: "recording", image: null, video: "data:video/webm;", caption: "2.install.webm" },
  ]);
  assert.equal(seen.inCard, true);
  assert.equal(seen.title, "Screenshots and recordings");
  assert.equal(seen.thumbnail?.[1], 178);
  assert.equal(seen.underAnswer, true);
  assert.equal(seen.rows, 0);
  assert.deepEqual(seen.read, ["proof-shot", "proof-video"]);
});

// What was checked never lists in the thread.
test("a merge has no Checked list in the thread", { timeout: 60000 }, async (t) => {
  const seen = await render(t);
  assert.equal(seen.checkedInThread, false);
});
