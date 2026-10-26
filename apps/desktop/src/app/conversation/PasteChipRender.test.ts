import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

for (const mode of ["rich", "plain"] as const) test(`text pasted as a chip, in the ${mode} field: an image and a pasted text of the same number come back through a draft, ↑ and a pulled queue message; rich text pastes as a chip sent as Markdown, a bold phrase as formatting, long text as a chip or, into an empty field, a card; the chip opens the Pages editor full size over the Chat, no dialog, and keeps an edit as it is made; a sent message shows it, opening read only (issue 427)`, { timeout: 120000 }, async (t) => {
  const page = mode === "plain" ? "paste-chip.html?plain" : "paste-chip.html";
  const seen = JSON.parse(await renderFixtureLive(t, page, "paste-chip-result", 90000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  const image = { pasted: false, inline: 1, mime: "image/png" };
  const markdown = { pasted: true, inline: 1, mime: "text/markdown" };
  assert.deepEqual(seen.draft, { text: "[Image #1] see [Pasted text #1]", kinds: [image, markdown] });
  assert.deepEqual(seen.queued, { text: "queued Pasted text #1", image: false });
  assert.deepEqual(seen.recalled, { text: "earlier [Image #1] [Pasted text #1]", kinds: [image, markdown] });
  assert.deepEqual(seen.rich, { text: "see [Pasted text #1] ok", kinds: [markdown], content: "## Title\n\n- a\n- b" });
  // The plain field keeps the bold phrase's words only.
  assert.deepEqual(seen.phrase, mode === "rich" ? { bold: "bold", chip: false, sent: "a **bold** words" } : { bold: null, chip: false, sent: "a bold words" });
  assert.deepEqual(seen.longChip, { text: "x [Pasted text #1]", kinds: [{ pasted: true, inline: 1, mime: "text/plain" }], whole: true });
  assert.deepEqual(seen.card, { chip: false, typed: "" });
  assert.deepEqual(seen.cardSent, [{ pasted: true, inline: null, mime: "text/plain" }]);
  assert.deepEqual(seen.both, {
    text: "[Image #1] [Pasted text #1]",
    kinds: [{ ...image, mime: "image/png" }, markdown],
    edited: "## Title\n\n- a\n- b more",
  });
  // A copied chip comes back with its text: numbered on in the same composer, as itself in one without it.
  assert.deepEqual(seen.copied, { text: "c [Pasted text #1] c [Pasted text #2]", kinds: [markdown, { ...markdown, inline: 2 }], same: true });
  assert.deepEqual(seen.copiedAgain, { text: "again c [Pasted text #1]", kinds: [markdown], content: "## Title\n\n- a\n- b" });
  // The rich field sends them escaped, so the chip is no code span; the plain field keeps them as typed.
  if (mode === "rich") assert.equal(seen.ticked, "e \\`[Pasted text #1] \\`");
  assert.equal(seen.big, "Pasted text #1· Start and end are sent");
  assert.deepEqual(seen.opened, { pane: "Pasted text #1", dialog: false });
  assert.deepEqual(seen.bubble, { pane: "Pasted text #1", dialog: false, done: false, text: "a", handles: 0 });
  // A chip sent in a code mark (as in the user's screenshot, its text a `Pasted text.md`) is a chip that opens.
  assert.deepEqual(seen.inCode, { pane: "Pasted text #1", code: false, text: "Also Eric messaged me this Pasted text #1" });
});
