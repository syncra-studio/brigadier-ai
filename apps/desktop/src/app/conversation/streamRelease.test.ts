import assert from "node:assert/strict";
import { test } from "node:test";

import { releasedText, shownText } from "@/app/conversation/streamRelease";

/** What shows after each prefix of `text`, the distinct steps in order. */
function steps(text: string): string[] {
  const shown: string[] = [];
  for (let end = 0; end <= text.length; end++) {
    const now = releasedText(text.slice(0, end));
    if (shown.at(-1) !== now) shown.push(now);
  }
  return shown;
}

test("a streaming reply shows its finished lines only, nothing before its first", () => {
  assert.equal(releasedText(""), "");
  assert.equal(releasedText("Looking at the **te"), "");
  assert.equal(releasedText("Looking at the **test**.\nThen the [doc"), "Looking at the **test**.\n");
  assert.deepEqual(steps("One.\nTwo **bold**.\n"), ["", "One.\n", "One.\nTwo **bold**.\n"]);
});

test("each step a reply shows is a finished line more than the one before", () => {
  const reply = "Done:\n\n- one\n- two\n\n```ts\nconst a = 1;\n```\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\nThat's all.\n";
  const shown = steps(reply);
  assert.equal(shown.at(-1), reply);
  for (const [index, step] of shown.entries()) {
    assert.ok(step === "" || step.endsWith("\n"), `half a line: ${JSON.stringify(step)}`);
    if (index > 0) assert.ok(step.startsWith(shown[index - 1] ?? ""), "only grows");
  }
});

test("a table shows with its header and divider together, then a row at a time", () => {
  const before = "Results:\n\n";
  assert.equal(releasedText(`${before}| Name | Score |\n`), before);
  assert.equal(releasedText(`${before}| Name | Score |\n| --- | --- |\n`), `${before}| Name | Score |\n| --- | --- |\n`);
  assert.equal(releasedText(`${before}| Name | Score |\n| --- | --- |\n| a | 1 |\n| b`), `${before}| Name | Score |\n| --- | --- |\n| a | 1 |\n`);
  assert.deepEqual(steps(`${before}| Name | Score |\n| --- | --- |\n| a | 1 |\n`), [
    "",
    "Results:\n",
    before,
    `${before}| Name | Score |\n| --- | --- |\n`,
    `${before}| Name | Score |\n| --- | --- |\n| a | 1 |\n`,
  ]);
  // A line with a bar that turns out to be no table is held one line only.
  assert.equal(releasedText("Use a | b here.\nAnd more.\n"), "Use a | b here.\nAnd more.\n");
});

test("a code block grows a line at a time; a bar in it holds nothing", () => {
  assert.equal(releasedText("Run:\n```sh\n"), "Run:\n```sh\n");
  assert.equal(releasedText("Run:\n```sh\nls | wc -l\n"), "Run:\n```sh\nls | wc -l\n");
  assert.equal(releasedText("Run:\n```sh\nls | wc -l\necho hi\n``"), "Run:\n```sh\nls | wc -l\necho hi\n");
  // After its closing fence, a bar opens a table again.
  assert.equal(releasedText("```\nx\n```\n| A | B |\n"), "```\nx\n```\n");
  // A longer fence holds a shorter run inside it; tildes close only tildes.
  assert.equal(releasedText("````md\n```\n| A |\n"), "````md\n```\n| A |\n");
  assert.equal(releasedText("~~~\n```\n| A |\n"), "~~~\n```\n| A |\n");
  // A fence in a quote closes in the quote.
  assert.equal(releasedText("> ```\n> x\n> ```\n| A |\n"), "> ```\n> x\n> ```\n");
});

test("a reply that ended shows whole, its last line unfinished too", () => {
  assert.equal(shownText("All done | see above", false), "All done | see above");
  assert.equal(shownText("All done | see above", true), "");
});
