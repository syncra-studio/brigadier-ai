import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { SHORTCUTS } from "@/app/shortcutCatalog";

// A project holds sessions; there is no workspace layer any more, so no text the user reads
// may name one. Internal names (types, IPC methods, data-slots) keep theirs.
const WORD = /\bworkspaces?\b/i;

/** Where the sidebar, the tab strip and the session's menus write what the user reads. */
const FILES = [
  "app/shell/AppSidebar.tsx",
  "app/shell/sidebar/nav.tsx",
  "app/shell/TopBar.tsx",
  "app/shell/SessionTabBar.tsx",
  "app/shell/ChatActions.tsx",
  "app/conversation/ForkMenu.tsx",
  "app/conversation/SlashCommands.tsx",
  "app/side-panel/NewTabTools.tsx",
  "app/side-panel/TaskDetails.tsx",
  "app/conversation/ActionCards.tsx",
  "app/inspector/providers/ProvidersTab.tsx",
  "app/inspector/providers/StartSessionForm.tsx",
  "app/inspector/providers/SessionHeader.tsx",
];

/** The JSX text and the sentence-like strings of `source`, its comments left out. */
function readable(source: string): string[] {
  const code = source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
  // JSX text: between tags, with none of the quotes or semicolons of code around generics.
  const jsx = [...code.matchAll(/>([^<>{}"=;]+)</g)].map((match) => (match[1] ?? "").trim());
  const strings = [...code.matchAll(/"([^"\\\n]*)"|`([^`]*)`/g)]
    // What a template puts in (`${workspace.target}`) is code, not text.
    .map((match) => match[1] ?? (match[2] ?? "").replace(/\$\{[^}]*\}/g, ""))
    // A path, a slot or a value ("pane-workspace", "@/state/workspaceCache") is a name, not text.
    .filter((text) => /\s/.test(text) || /^[A-Z]/.test(text));
  return [...jsx, ...strings].filter(Boolean);
}

test("no shortcut or menu item says workspace", () => {
  for (const shortcut of SHORTCUTS) {
    assert.doesNotMatch(`${shortcut.name} ${shortcut.line} ${shortcut.menu ?? ""}`, WORD, shortcut.id);
  }
});

test("the sidebar, the tab strip and the session's menus never say workspace", () => {
  for (const file of FILES) {
    const said = readable(readFileSync(new URL(`../${file}`, import.meta.url), "utf8")).filter((text) => WORD.test(text));
    assert.deepEqual(said, [], file);
  }
});

test("a raw session's header names its folder, not a workspace", () => {
  // `readable` leaves out a one-word string, which is all this label was.
  const source = readFileSync(new URL("./inspector/providers/SessionHeader.tsx", import.meta.url), "utf8");
  assert.doesNotMatch(source, /\?\s*"workspace"\s*:/);
});
