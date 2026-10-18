import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("the rich composer: Markdown in a draft, a pulled queue message and a recalled prompt shows as formatting; typed markers format and go as Markdown; Enter sends, Shift+Enter is a new line or list item; code blocks colour and are left by Shift+Enter, a closing fence or ↓; no key formats; menus open in lists and after formatting; drafts keep Markdown; image chips and pasted text work as in the plain field; the Plain text composer switch turns the open draft to text and back", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "rich-composer.html", "rich-composer-result", 90000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  // A draft: a heading with italic, a mention whose name has Markdown in it (still one chip), code a mark.
  assert.deepEqual(seen.draft, { heading: true, italic: true, chip: ["chat"], code: true });
  assert.equal(seen.draftSent, "## Draft *title*\nAsk @Release *draft* about `x`");
  assert.deepEqual(seen.draftMentions, [{ type: "chat", id: "c1", title: "Release *draft*" }]);
  // ↑ pulls the queued message back, then walks back to an earlier prompt, both formatted.
  assert.deepEqual(seen.queued, { items: 2, code: true });
  assert.deepEqual(seen.recalled, { items: 2, bold: true });
  assert.equal(seen.marks, "**bold** and *it*");
  assert.equal(seen.list, "- a\n- b");
  assert.equal(seen.lines, "a\nb");
  assert.equal(seen.coloured, true);
  assert.equal(seen.codeSent, "```ts\nlet x = 1\n```");
  assert.equal(seen.codeLeft, "```\nx\ny\n```\nafter");
  assert.equal(seen.fenceLeft, "```\nz\n```\nw");
  assert.equal(seen.downLeft, "```\nq\n```\nr");
  assert.deepEqual(seen.noBold, { bold: false, sent: "plainx" });
  assert.equal(seen.menus, true);
  assert.equal(seen.task, "- [x] task");
  assert.equal(seen.draftKept, true);
  assert.deepEqual(seen.image, { sent: "[Image #1] look", inline: [1] });
  assert.deepEqual(seen.literal, { bold: false, sent: "\\*\\*x\\*\\* and snake_case" });
  // The Plain text composer switch: the same draft as literal text, and formatted again.
  assert.deepEqual(seen.plain, { text: "**b** and `c` see [l](https://x.y) ", chips: 0, bold: false });
  assert.equal(seen.richAgain, "**b** and `c` see [l](https://x.y)");
});
