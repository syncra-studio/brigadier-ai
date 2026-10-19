import assert from "node:assert/strict";
import { test } from "node:test";

import { blocksOf, bottomLineCard, PICK_HREF, splitAnswer, spokenAnswer } from "./answerLayout";
import { splitReport } from "./phaseView";
import { CODING, FACT, FREE, HOWTO, TRADING } from "@/fixtures/answerLayoutTexts";

/** The coding reply as `ReportText` splits it: its `### Details` off first. */
const CODING_HEAD = splitReport(CODING)!.head;

const kinds = (layout: ReturnType<typeof splitAnswer>) => layout?.blocks.map((block) => block.kind);

test("the trading answer splits into its bottom line, parts, folds, Ask next and sources", () => {
  const layout = splitAnswer(TRADING, "chat");
  assert.ok(layout);
  assert.match(layout.bottomLine ?? "", /^Don't trade for quick money/);
  assert.ok(bottomLineCard(layout));
  assert.deepEqual(kinds(layout), ["steps", null, "pick", "caution"]);
  const [steps, , pick, caution] = layout.blocks;
  assert.equal(steps?.kind, "steps");
  if (steps?.kind === "steps") {
    assert.equal(steps.props.steps.length, 5);
    assert.deepEqual(steps.props.steps[1], { title: "Open the real account.", detail: 'Choose **Invest**, not "CFD": CFDs are bets on prices, not real shares.' });
  }
  if (pick?.kind === "pick") {
    assert.match(pick.props.markdown, new RegExp(`\\| Trading 212 Invest \\[My pick for you\\]\\(${PICK_HREF}\\) \\|`));
    assert.ok(!pick.props.markdown.includes("★"));
  }
  if (caution?.kind === "caution") {
    assert.equal(caution.props.title, "Avoid these");
    assert.equal(caution.props.items.length, 4);
    assert.match(caution.props.items[3]!, /CNPF first: 022 85 95 95\.$/);
  }
  assert.deepEqual(
    layout.folds.map((fold) => fold.title),
    ["Why not fast income", "Taxes in Moldova", "How to spot a scam", "Who can help, for free", "What I couldn't check"],
  );
  assert.deepEqual(layout.folds[0]!.blocks[0], { kind: "chance", props: { of: 80, caption: "small CFD accounts lose money" }, text: layout.folds[0]!.blocks[0]!.text });
  assert.deepEqual(layout.askNext, ["Walk me through the practice account", "How much should I put in each month?", "What is an index fund?"]);
  assert.equal(layout.sources?.length, 12);
  // Markers link to their source, in view and in folds.
  assert.match(layout.bottomLine ?? "", /lose \[6\]\(brigadier-cite:6\)/);
  assert.match(layout.folds[1]!.blocks[0]!.text, /\[9\]\(brigadier-cite:9\)/);
});

test("a session lays the trading answer out too: it has explicit parts", () => {
  assert.deepEqual(kinds(splitAnswer(TRADING, "session")), ["steps", null, "pick", "caution"]);
});

test("an answer with no parts, or one that keeps today's look, gives no layout", () => {
  for (const mode of ["chat", "session"] as const) {
    assert.equal(splitAnswer(FACT, mode), null, mode);
    assert.equal(splitAnswer(FREE, mode), null, mode);
    assert.equal(splitAnswer("Done: the tests pass.", mode), null, mode);
  }
  // A session's coding reply: a paragraph, a numbered list, a table and a stray `---`.
  assert.equal(splitAnswer(CODING_HEAD, "session"), null);
  assert.equal(splitAnswer(TRADING, "off"), null);
});

test("steps alone lay out a Chat's how-to, never a session's answer", () => {
  const layout = splitAnswer(HOWTO, "chat");
  assert.deepEqual(kinds(layout), ["steps", null]);
  assert.ok(layout && bottomLineCard(layout));
  assert.equal(splitAnswer(HOWTO, "session"), null);
  // In a Chat the same shape is a how-to's steps, whatever its words.
  assert.deepEqual(kinds(splitAnswer(CODING_HEAD, "chat")), ["steps", null, null]);
});

const list = (count: number) => Array.from({ length: count }, (_, index) => `${index + 1}. Do step ${index + 1}. Then wait.`).join("\n");

test("steps take 3 to 7 items right after the opening paragraph", () => {
  assert.equal(splitAnswer(`Opening.\n\n${list(2)}`, "chat"), null);
  assert.deepEqual(kinds(splitAnswer(`Opening.\n\n${list(3)}`, "chat")), ["steps"]);
  assert.deepEqual(kinds(splitAnswer(`Opening.\n\n${list(7)}`, "chat")), ["steps"]);
  assert.equal(splitAnswer(`Opening.\n\n${list(8)}`, "chat"), null);
  // Not right after the opening paragraph.
  assert.equal(splitAnswer(`Opening.\n\nMore.\n\n${list(3)}`, "chat"), null);
  // Nested, or not from 1.
  assert.equal(splitAnswer(`Opening.\n\n${list(3)}\n   - a detail`, "chat"), null);
  assert.equal(splitAnswer("Opening.\n\n2. One.\n3. Two.\n4. Three.", "chat"), null);
  // A loose list (blank lines between items) is still one list.
  assert.deepEqual(kinds(splitAnswer("Opening.\n\n1. One.\n\n2. Two.\n\n3. Three.", "chat")), ["steps"]);
});

test("a half-followed answer keeps what it got right and the rest stays Markdown", () => {
  // A table with no ★, a quote that isn't an alert, a chance block with no number: Markdown.
  const half = [
    "Pick the cheaper one.",
    "",
    "| Option | Cost |",
    "|---|---|",
    "| A | $1 |",
    "",
    "> Avoid these",
    "> - Scams",
    "",
    "```chance",
    "most of them",
    "```",
    "",
    "---",
    "",
    "### Why",
    "Because it costs less.",
  ].join("\n");
  const layout = splitAnswer(half, "session");
  assert.ok(layout, "its folds still lay it out");
  assert.deepEqual(kinds(layout), [null, null, null]);
  assert.deepEqual(layout.folds.map((fold) => fold.title), ["Why"]);
});

test("a `---` folds only what is all `###` sections after it", () => {
  // A rule between paragraphs, a rule under a paragraph's line (a heading), a rule in a fence.
  assert.equal(splitAnswer("One.\n\n---\n\nTwo.\n\n### After\nThree.", "session"), null);
  assert.equal(splitAnswer("One.\n---\n\n### After\nThree.", "session"), null);
  assert.equal(splitAnswer("One.\n\n```\n---\n### not a fold\n```", "session"), null);
  const layout = splitAnswer("One.\n\n---\n\n### A\nx\n\n#### inner\ny\n\n### B\n```\n### in code\n```", "session");
  assert.deepEqual(layout?.folds.map((fold) => [fold.title, fold.blocks.map((block) => block.text)]), [
    ["A", ["x", "#### inner\ny"]],
    ["B", ["```\n### in code\n```"]],
  ]);
  // Folds that open with nothing before them: no bottom line.
  assert.equal(splitAnswer("---\n\n### A\nx", "session")?.bottomLine, null);
});

test("parts are keyed on shape, not words: a Romanian answer splits the same", () => {
  const ro = [
    "Nu tranzacționa pentru bani rapizi.",
    "",
    "> [!CAUTION]",
    "> Evită acestea",
    "> - CFD-uri și forex.",
    "",
    "---",
    "",
    "### Taxe în Moldova",
    "12% pe dividende.",
    "",
    "### Ask next",
    "- Ce este un fond indexat?",
  ].join("\n");
  const layout = splitAnswer(ro, "session");
  assert.deepEqual(kinds(layout), ["caution"]);
  assert.deepEqual(layout?.folds.map((fold) => fold.title), ["Taxe în Moldova"]);
  assert.deepEqual(layout?.askNext, ["Ce este un fond indexat?"]);
});

test("Ask next takes a short list only, at most three, and alone shows no card", () => {
  const layout = splitAnswer("It is 12%.\n\n### Ask next\n- One?\n- Two?\n- Three?\n- Four?", "chat");
  assert.deepEqual(layout?.askNext, ["One?", "Two?", "Three?"]);
  assert.ok(layout && !bottomLineCard(layout));
  assert.equal(splitAnswer("It is 12%.\n\n### Ask next\nWhatever you like.", "chat"), null);
});

test("a streaming prefix splits without throwing and only settles what is whole", () => {
  for (let end = 0; end <= TRADING.length; end += 37) {
    const partial = TRADING.slice(0, end);
    for (const mode of ["chat", "session"] as const) splitAnswer(partial, mode);
  }
  // An unclosed chance fence and a table cut before its ★ row stay Markdown.
  const cut = TRADING.slice(0, TRADING.indexOf("80 of 100") + 4);
  assert.ok(!splitAnswer(cut, "session")?.folds.some((fold) => fold.blocks.some((block) => block.kind === "chance")));
  const table = TRADING.slice(0, TRADING.indexOf("| Trading 212"));
  assert.deepEqual(kinds(splitAnswer(table, "chat")), ["steps", null, null]);
});

test("blocks keep a fence whole across blank lines", () => {
  assert.deepEqual(blocksOf("a\n\n```\nx\n\ny\n```\n\nb"), ["a", "```\nx\n\ny\n```", "b"]);
});

test("read aloud says the opening and the parts in view, never folds, sources or Ask next", () => {
  const spoken = spokenAnswer(splitAnswer(TRADING, "chat")!);
  assert.match(spoken, /^Don't trade for quick money: .* most beginners who trade often lose\. If you/);
  assert.match(spoken, /1\. Practise first\. Open the free/);
  assert.match(spoken, /Avoid these\nForex, CFDs/);
  assert.match(spoken, /Trading 212 Invest \(my pick\)/);
  assert.ok(spoken.endsWith("\n\nMore details are folded below."));
  for (const hidden of ["Taxes", "80 out of 100", "Walk me through", "helpcentre", "brigadier-cite"]) assert.ok(!spoken.includes(hidden), hidden);
  // Without folds, no notice.
  assert.ok(!spokenAnswer(splitAnswer(HOWTO, "chat")!).includes("folded"));
});
