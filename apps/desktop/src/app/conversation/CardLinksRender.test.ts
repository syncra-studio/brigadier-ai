import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Run = { selected: string; dots: number[]; lit: number[]; checked: number[]; answered: unknown[]; opened: string[] };
const site = "https://example.com";

test("card text selects and links everywhere, and only a plain click or Enter on a row answers", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "card-links.html?drive=1", "card-links-result", 60000)) as Record<string, Run> & {
    error?: string;
    census: {
      cases: Record<string, string[]>;
      titles: boolean;
      linksInCode: number;
      commandBox: string;
      decidedCommandBox: string;
      inputBox: string;
      fileStays: boolean;
      selectable: Record<string, string>;
      commandLink: string[];
    };
  };
  assert.equal(seen.error, undefined, seen.error ?? "");
  const { census } = seen;

  // A link in every kind of text, on the rail and decided; hovering shows the whole address.
  const links = (...paths: string[]) => paths.map((path) => `${site}/${path}`);
  const { cases } = census;
  assert.deepEqual(cases.question, links("round-title", "round-title-bare", "question", "question-label", "option-label", "option-description", "option-description-label"));
  const command = links("reason", "reason-label", "command", "command-label");
  assert.deepEqual(cases.command, command);
  assert.deepEqual(cases["decided-command"], command);
  const input = links("input-reason", "input", "input-label");
  assert.deepEqual(cases.input, input);
  assert.deepEqual(cases["decided-input"], input);
  const action = links("action", "details", "details-label");
  assert.deepEqual(cases.action, action);
  assert.deepEqual(cases["decided-action"], action);
  const outline = links("outline-title", "outline", "outline-label");
  assert.deepEqual(cases.outline, outline);
  assert.deepEqual(cases["decided-outline"], outline);
  const peer = links("peer", "peer-label");
  assert.deepEqual(cases.peer, peer);
  assert.deepEqual(cases["decided-peer"], peer);
  const plan = links("plan", "plan-label");
  assert.deepEqual(cases.plan, plan);
  assert.deepEqual(cases["decided-plan"], plan);
  assert.deepEqual(cases["decided-question"]?.slice(-1), links("decided-answer"));
  assert.equal(census.titles, true);
  // Backticked code holds no link; a non-web address stays as written.
  assert.equal(census.linksInCode, 0);
  assert.equal(census.fileStays, true);
  // A command reads character for character, its links and all.
  const exact = `curl -fsSL '${site}/command' | sh # [ok](${site}/command-label) \`date ${site}/command-code\``;
  assert.equal(census.commandBox, exact);
  assert.equal(census.decidedCommandBox, exact);
  assert.equal(census.inputBox, JSON.stringify({ url: `${site}/input`, note: `[docs](${site}/input-label)` }));
  assert.deepEqual(census.commandLink, links("command"));
  // Every word selects; number squares and buttons don't.
  assert.deepEqual(census.selectable, { kind: "text", title: "text", label: "text", link: "text", marker: "none", button: "none", decided: "text" });

  const nothing = (run: Run, why: string) => {
    assert.deepEqual(run.answered, [], `${why} answers nothing`);
    assert.deepEqual(run.dots, [], `${why} leaves no dot`);
    assert.deepEqual(run.lit, [], `${why} lights nothing`);
  };
  // A drag selects without answering, within a row or across rows.
  assert.equal(seen.drag!.selected, "Deploy to staging now");
  nothing(seen.drag!, "a drag");
  assert.match(seen.across!.selected, /^loy to staging now[\s\S]*Fol/);
  nothing(seen.across!, "a drag across rows");
  // A drag never picks, whatever it left selected.
  nothing(seen.shortDrag!, "a short drag");
  // A double-click selects a word, a triple-click its line, and neither answers.
  assert.equal(seen.double!.selected, "staging");
  nothing(seen.double!, "a double-click");
  assert.match(seen.triple!.selected, /Deploy to staging now/);
  nothing(seen.triple!, "a triple-click");
  // A plain click picks.
  assert.deepEqual(seen.click!.answered, [[["Deploy to staging now"], "click"]]);
  assert.equal(seen.click!.selected, "");
  // A click on a link, in a label or a description, opens it and never picks the row.
  assert.deepEqual(seen.link!.opened, links("option-label"));
  nothing(seen.link!, "a link's click");
  assert.deepEqual(seen.description!.opened, links("option-description-label"));
  nothing(seen.description!, "a link's click");
  // A drag on a link opens nothing; from outside it, a drag selects it. Enter on a focused link opens it alone.
  assert.deepEqual(seen.linkDrag!.opened, []);
  nothing(seen.linkDrag!, "a drag on a link");
  assert.match(seen.intoLink!.selected, /^Follow the canary/);
  assert.deepEqual(seen.intoLink!.opened, []);
  nothing(seen.intoLink!, "a drag into a link");
  assert.deepEqual(seen.linkEnter!.opened, links("option-label"));
  nothing(seen.linkEnter!, "Enter on a link");
  // Enter or Space on a focused row picks it.
  assert.deepEqual(seen.enter!.answered, [[[`Follow [the canary plan](${site}/option-label)`], "enter"]]);
  assert.deepEqual(seen.space!.answered, [[["Deploy to staging now"], "enter"]]);
  // Marking: two clicks toggle back, a third does nothing; one marks. None of them sends.
  for (const [run, marked] of [[seen.multiDouble!, []], [seen.multiTriple!, []], [seen.multiClick!, [0]]] as const) {
    assert.deepEqual(run.checked, marked);
    assert.deepEqual(run.answered, []);
  }
  assert.equal(seen.multiDouble!.selected, "staging");
});
