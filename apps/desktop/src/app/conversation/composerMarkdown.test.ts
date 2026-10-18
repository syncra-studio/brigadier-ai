import assert from "node:assert/strict";
import { test } from "node:test";

import { $isDirectiveNode } from "@assistant-ui/react-lexical";
import { createHeadlessEditor } from "@lexical/headless";
import { $dfs } from "@lexical/utils";
import { $createLineBreakNode, $createParagraphNode, $createTextNode, $getRoot, $isTextNode, type LexicalEditor } from "lexical";

import { imageMarkerAt } from "@/app/conversation/inlineImages";
import { chipFormatter } from "@/components/assistant-ui/elements/chipText";

import { $exportComposer, $importComposer, CARET, COMPOSER_NODES } from "./composerMarkdown";

const known = [
  { label: "Release *draft*", type: "chat", id: "chat:c1" },
  { label: "notes `v2`", type: "chat", id: "chat:c2" },
  { label: "task-9", type: "task", id: "task:9" },
];
const formatter = chipFormatter(
  (text, at) => known.find((mention) => text.startsWith(mention.label, at)) ?? null,
  imageMarkerAt,
  { code: false, links: true },
);

function editor(): LexicalEditor {
  return createHeadlessEditor({ nodes: COMPOSER_NODES, onError: (error) => { throw error; } });
}

function imported(markdown: string): LexicalEditor {
  const field = editor();
  field.update(() => $importComposer(markdown, formatter.parse, formatter), { discrete: true });
  return field;
}

const exported = (field: LexicalEditor) => field.getEditorState().read(() => $exportComposer());
const roundTrip = (markdown: string) => exported(imported(markdown));

test("every format and block the rich composer offers comes back unchanged", () => {
  for (const markdown of [
    "one line",
    "a\nb",
    "a\n\nb",
    "a\n\n\nb",
    "# Heading one\n## Two\n###### Six",
    "**bold** *italic* ~~strike~~ `code` [a link](https://example.com)",
    "- bulleted\n- list\ntext after",
    "1. numbered\n2. list",
    "- [ ] open task\n- [x] done task",
    "> a quote\n> on two lines",
    "```ts\nconst a = 1;\n\tindented\n```\nafter",
    "---",
    "  leading and trailing spaces  ",
  ]) {
    assert.equal(roundTrip(markdown), markdown);
  }
});

test("a second round trip changes nothing", () => {
  for (const markdown of ["- a\n  - nested", "**bold @task-9 x**", "x [not](a link) y", "*** "]) {
    const once = roundTrip(markdown);
    assert.equal(roundTrip(once), once, markdown);
  }
});

test("text typed or pasted as text stays text: only the markers that would format it go escaped", () => {
  const cases: [string, string][] = [
    ["**x** and snake_case", "\\*\\*x\\*\\* and snake_case"],
    ["snake_case_name and 2 * 3", "snake_case_name and 2 * 3"],
    ["a `b` c", "a \\`b\\` c"],
    ["# not a heading", "\\# not a heading"],
    ["- not a list", "\\- not a list"],
    ["1. not a list", "1\\. not a list"],
    ["> not a quote", "\\> not a quote"],
    ["```", "\\`\\`\\`"],
    ["---", "\\---"],
    ["see [a](b)", "see [a]\\(b)"],
    ["C:\\path\\*", "C:\\path\\\\*"],
    ["#hashtag", "#hashtag"],
  ];
  for (const [text, markdown] of cases) {
    const field = editor();
    field.update(() => $getRoot().append($createParagraphNode().append($createTextNode(text))), { discrete: true });
    assert.equal(exported(field), markdown, text);
    const back = imported(markdown);
    back.getEditorState().read(() => {
      assert.equal($getRoot().getTextContent(), text, markdown);
      for (const { node } of $dfs()) {
        assert.ok(["root", "paragraph", "text"].includes(node.getType()), `${markdown}: ${node.getType()}`);
        if ($isTextNode(node)) assert.equal(node.getFormat(), 0, markdown);
      }
    });
  }
});

test("a line after a line break in a paragraph is a line start: its block marker goes escaped", () => {
  const field = editor();
  field.update(
    () => $getRoot().append($createParagraphNode().append($createTextNode("a"), $createLineBreakNode(), $createTextNode("# literal"))),
    { discrete: true },
  );
  assert.equal(exported(field), "a\n\\# literal");
  imported(exported(field)).getEditorState().read(() => {
    assert.ok([...$dfs()].every(({ node }) => node.getType() !== "heading"));
  });
});

/** The chips the editor holds, by id, in order. */
function chips(field: LexicalEditor): string[] {
  return field.getEditorState().read(() =>
    [...$dfs()].flatMap(({ node }) => ($isDirectiveNode(node) ? [node.getDirectiveItem().id] : [])),
  );
}

test("chips stay whole, Markdown in their names too, and stay text in code", () => {
  const markdown = "Ask @Release *draft* and @notes `v2` with @task-9 [Image #2] https://a.b/c_d_e";
  assert.equal(roundTrip(markdown), markdown);
  assert.deepEqual(chips(imported(markdown)), ["chat:c1", "chat:c2", "task:9", "image:2", "url:https://a.b/c_d_e"]);
  // Emphasis closing right after a link stays emphasis, outside the link's chip.
  const bold = imported("**see https://example.com**");
  assert.deepEqual(chips(bold), ["url:https://example.com"]);
  bold.getEditorState().read(() => {
    assert.ok([...$dfs()].some(({ node }) => $isTextNode(node) && node.hasFormat("bold") && node.getTextContent() === "see "));
  });
  // In a heading and a list item too.
  assert.deepEqual(chips(imported("## About @task-9\n- see [Image #1]")), ["task:9", "image:1"]);
  // Code keeps them as their text.
  for (const code of ["`@task-9`", "```\n@task-9 [Image #1]\n```"]) {
    assert.equal(roundTrip(code), code);
    assert.deepEqual(chips(imported(code)), []);
  }
});

/** Where `CARET` lands for the caret at `offset` in the text node holding `text`. */
function caretAt(markdown: string, text: string, offset: number): number {
    const field = imported(markdown);
    return field.getEditorState().read(() => {
      const found = [...$dfs()].map(({ node }) => node).find((each) => $isTextNode(each) && each.getTextContent() === text)!;
      const marked = $exportComposer({ key: found.getKey(), offset });
      assert.equal(marked.replace(CARET, ""), $exportComposer());
      return marked.indexOf(CARET);
    });
  }

test("the caret's place in the Markdown is where the menus look for `@` and `/`", () => {
  assert.equal(caretAt("- see @", "see @", 5), "- see @".length);
  assert.equal(caretAt("**b** @", " @", 2), "**b** @".length);
  assert.equal(caretAt("# T\n> q /x", "q /x", 4), "# T\n> q /x".length);
});
