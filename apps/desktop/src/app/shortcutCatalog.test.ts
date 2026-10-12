import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { MENU_KEYS } from "@/app/paneShortcuts";
import {
  SHORTCUTS,
  formatKeys,
  parseStroke,
  searchShortcuts,
  shortcutKeys,
  shortcutsFor,
  strokeOf,
} from "@/app/shortcutCatalog";
import { sessionTabKey } from "@/state/sessionTabKeys";

const press = (code: string, key: string, mods: { ctrl?: boolean; alt?: boolean; shift?: boolean; cmd?: boolean } = {}) => ({
  code,
  key,
  ctrlKey: mods.ctrl ?? false,
  altKey: mods.alt ?? false,
  shiftKey: mods.shift ?? false,
  metaKey: mods.cmd ?? false,
});

const CODE_OF: Record<string, string> = {
  "[": "BracketLeft",
  "]": "BracketRight",
  "`": "Backquote",
  Tab: "Tab",
  ",": "Comma",
  "←": "ArrowLeft",
  "→": "ArrowRight",
  "=": "Equal",
  "-": "Minus",
};
const codeOf = (key: string) => CODE_OF[key] ?? (/^\d$/.test(key) ? `Digit${key}` : `Key${key}`);

const ARROWS: Record<string, string> = { Left: "←", Right: "→" };

/** A Tauri accelerator ("Alt+CmdOrCtrl+S") in the catalog's notation. */
const accelerator = (text: string) => {
  const parts = text.split("+");
  const has = (name: string) => parts.includes(name);
  const key = parts.at(-1) ?? "";
  return `${has("Ctrl") ? "⌃" : ""}${has("Alt") ? "⌥" : ""}${has("Shift") ? "⇧" : ""}${has("CmdOrCtrl") ? "⌘" : ""}${ARROWS[key] ?? key}`;
};

const ids = (found: { id: string }[]) => found.map((s) => s.id);

test("every shortcut has its own id, and keys written ⌃⌥⇧⌘ then the key", () => {
  assert.equal(new Set(SHORTCUTS.map((s) => s.id)).size, SHORTCUTS.length);
  for (const shortcut of SHORTCUTS) {
    for (const chip of shortcut.keys) {
      for (const part of chip.split(" ")) {
        assert.equal(formatKeys(part, true), part, `${shortcut.id}: ${part} is in order`);
        assert.ok(parseStroke(part).key, `${shortcut.id}: ${part} has a key`);
      }
    }
  }
});

test("a menu-owned shortcut's keys are the keys its menu item sends", () => {
  for (const shortcut of SHORTCUTS.filter((s) => s.menu)) {
    const menu = MENU_KEYS[shortcut.menu ?? ""];
    assert.ok(menu, `${shortcut.id}: menu item ${shortcut.menu} exists`);
    const stroke = parseStroke((shortcut.keys[0] ?? "").replace("1–9", "1"));
    assert.equal(codeOf(stroke.key), menu.code, shortcut.id);
    assert.equal(stroke.shift, menu.shift ?? false, `${shortcut.id} shift`);
    assert.equal(stroke.alt, menu.alt ?? false, `${shortcut.id} alt`);
    assert.equal(stroke.ctrl, menu.control ?? false, `${shortcut.id} control`);
    assert.equal(stroke.cmd, !menu.control, `${shortcut.id} command`);
  }
});

test("the native menu's accelerators are the catalog's keys", () => {
  const shell = readFileSync(new URL("../../src-tauri/src/shell.rs", import.meta.url), "utf8");
  const menu = new Map<string, string>();
  for (const [, id, keys] of shell.matchAll(/\(\s*"([a-z-]+)",\s*"[^"]+",\s*"([^"]+)",?\s*\)/g)) if (id && keys) menu.set(id, accelerator(keys));
  assert.ok(menu.size > 15, "the Panes and View items were read");
  // Items rustfmt spreads over lines are read too.
  for (const id of ["reopen", "previous", "force-reload", "zoom-in", "zoom-out", "page-back"]) assert.ok(menu.has(id), id);
  for (const shortcut of SHORTCUTS.filter((s) => s.menu && menu.has(s.menu))) {
    assert.equal(menu.get(shortcut.menu ?? ""), shortcut.keys[0], shortcut.id);
  }
  assert.match(shell, /"Settings…"\)\s*\.accelerator\("CmdOrCtrl\+,"\)/);
  assert.match(shell, /"Open Folder…"\)\s*\.accelerator\("CmdOrCtrl\+O"\)/);
  assert.match(shell, /"Close Window"\)\s*\.accelerator\("CmdOrCtrl\+Shift\+W"\)/);
  assert.match(shell, /"Close tab, page or terminal"\)\s*\.accelerator\("CmdOrCtrl\+W"\)/);
  // A second key of a row has its own item.
  for (const [item, id] of [["previous-tab", "previous"], ["next-tab", "next"]] as const) {
    const shortcut = SHORTCUTS.find((s) => s.id === id);
    assert.equal(menu.get(item), shortcut?.keys[1], item);
    const stroke = parseStroke(shortcut?.keys[1] ?? "");
    assert.deepEqual(MENU_KEYS[item], { code: codeOf(stroke.key), alt: true }, item);
  }
});

test("session tab shortcuts are what the tab keys handler does, on both platforms", () => {
  const expected: Record<string, ReturnType<typeof sessionTabKey>> = {
    "new-tab": { type: "new", kind: "newTab" },
    "new-browser": { type: "new", kind: "browser" },
    "new-side-chat": { type: "new", kind: "sideChat" },
    "new-page": { type: "new", kind: "page" },
    close: { type: "close" },
    "close-others": { type: "closeOthers" },
    previous: { type: "step", step: -1 },
    next: { type: "step", step: 1 },
    "cycle-next": { type: "step", step: 1 },
    "cycle-previous": { type: "step", step: -1 },
    "show-tab": { type: "number", number: 5 },
    "new-terminal": { type: "new", kind: "terminal" },
  };
  for (const mac of [true, false]) {
    for (const [id, action] of Object.entries(expected)) {
      const shortcut = SHORTCUTS.find((s) => s.id === id);
      assert.ok(shortcut, id);
      const stroke = parseStroke((shortcut.keys[0] ?? "").replace("1–9", "5"));
      const event = {
        ...press(codeOf(stroke.key), stroke.key, {
          ctrl: stroke.ctrl || (!mac && stroke.cmd),
          alt: stroke.alt,
          shift: stroke.shift,
          cmd: mac && stroke.cmd,
        }),
        isComposing: false,
      };
      assert.deepEqual(sessionTabKey(event, mac), action, `${id} on ${mac ? "macOS" : "other"}`);
    }
  }
});

test("keys read as macOS glyphs there and as Ctrl, Alt and Shift elsewhere", () => {
  assert.equal(formatKeys("⇧⌘A", false), "Ctrl+Shift+A");
  assert.equal(formatKeys("⌥⌘I", false), "Ctrl+Alt+I");
  assert.equal(formatKeys("⌃`", false), "Ctrl+`");
  assert.equal(formatKeys("⌘⏎", false), "Ctrl+Enter");
  assert.equal(formatKeys("Esc Esc", false), "Esc Esc");
  assert.equal(shortcutKeys("search", true), "⌘K");
  assert.equal(shortcutKeys("search", false), "Ctrl+K");
  assert.equal(shortcutKeys("inspector", false), "Ctrl+Alt+I");
  assert.ok(shortcutsFor(true).some((s) => s.id === "close-window"));
  assert.ok(!shortcutsFor(false).some((s) => s.id === "close-window"));
});

test("a key press reads in the catalog's notation", () => {
  assert.equal(strokeOf(press("BracketLeft", "[", { cmd: true })), "⌘[");
  assert.equal(strokeOf(press("BracketLeft", "{", { cmd: true, shift: true })), "⇧⌘[");
  assert.equal(strokeOf(press("KeyD", "D", { ctrl: true, shift: true })), "⌃⇧D");
  assert.equal(strokeOf(press("Digit2", "@", { shift: true })), "@");
  assert.equal(strokeOf(press("Slash", "/")), "/");
  assert.equal(strokeOf(press("Enter", "Enter", { shift: true })), "⇧⏎");
  assert.equal(strokeOf(press("Escape", "Escape")), "Esc");
  assert.equal(strokeOf(press("MetaLeft", "Meta", { cmd: true })), null);
});

test("search finds shortcuts by words and by a key press", () => {
  assert.deepEqual(ids(searchShortcuts(SHORTCUTS, { words: "dictat" }, true)), ["dictate"]);
  assert.deepEqual(ids(searchShortcuts(SHORTCUTS, { words: "side chat" }, true)), ["new-side-chat"]);
  assert.deepEqual(ids(searchShortcuts(SHORTCUTS, { press: "⌘[" }, true)), ["back"]);
  assert.deepEqual(ids(searchShortcuts(SHORTCUTS, { press: "⌘5" }, true)), ["show-tab"]);
  assert.deepEqual(ids(searchShortcuts(SHORTCUTS, { press: "⇧⌘S" }, true)), ["sidebar", "save-as"]);
  assert.deepEqual(ids(searchShortcuts(SHORTCUTS, { press: "Esc" }, true)), ["clear-picked", "stop", "decline"]);
  assert.deepEqual(ids(searchShortcuts(SHORTCUTS, { press: "3" }, true)), ["answer"]);
  // Off macOS Ctrl is both ⌘ and ⌃.
  assert.deepEqual(ids(searchShortcuts(shortcutsFor(false), { press: "⌃T" }, false)), ["new-tab"]);
  assert.deepEqual(ids(searchShortcuts(shortcutsFor(false), { press: "⌃`" }, false)), ["new-terminal"]);
});

const shortcutIds = (browser: boolean) => shortcutsFor(true, browser).map((shortcut) => shortcut.id);

const BROWSER_ONLY = [
  "new-browser",
  "address",
  "page-back",
  "page-forward",
  "reload",
  "force-reload",
  "find-next",
  "find-previous",
  "zoom-in",
  "zoom-out",
  "zoom-reset",
];

test("off, the built-in browser's shortcuts are gone", () => {
  for (const id of BROWSER_ONLY) {
    assert.ok(shortcutIds(true).includes(id), id);
    assert.ok(!shortcutIds(false).includes(id), id);
  }
  // ⌘F still finds in a file.
  assert.ok(shortcutIds(false).includes("find"));
  assert.equal(shortcutIds(true).length - shortcutIds(false).length, BROWSER_ONLY.length);
});

test("the Browser menu holds the browser's items, which leave with it", () => {
  const shell = readFileSync(new URL("../../src-tauri/src/shell.rs", import.meta.url), "utf8");
  const menu = shell.slice(shell.indexOf("const BROWSER_ITEMS"), shell.indexOf("const BROWSER_MENU"));
  const items = [...menu.matchAll(/\(\s*"([a-z-]+)",/g)].map(([, id]) => id);
  assert.deepEqual(items.toSorted(), [...BROWSER_ONLY, "find"].toSorted());
  // Each reaches the page's tab as its key, and the Panes menu no longer has them.
  const panes = shell.slice(shell.indexOf('Submenu::new(app, "Panes"'), shell.indexOf("menu.append(&panes)"));
  for (const id of items) {
    assert.ok(MENU_KEYS[id ?? ""], `${id} has its key`);
    assert.ok(!new RegExp(`\\(\\s*"${id}",`).test(panes), `${id} is not in Panes`);
  }
});
