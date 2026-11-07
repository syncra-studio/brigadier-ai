import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

import type { ComputerSession, ComputerSetup, ComputerUseStatus } from "@/ipc/generated";

import { appMenuTemplate, computerTemplate, icoEntry, KEYED_ITEMS, plural, sessionTitle, setupLine, showing, trayTemplate } from "./trayMenus.ts";

const read = (path: string) => fs.readFileSync(new URL(path, import.meta.url), "utf8");
const session = (id: string, app: string, conversationId: string | null = null): ComputerSession =>
  ({ session: id, conversationId, taskId: null, label: id, app, bundleId: "", pid: 0 });
const status = (sessions: ComputerSession[], setup: ComputerSetup = { state: "ready" }) => ({ sessions, setup }) as ComputerUseStatus;
const pngWidth = (bytes: Uint8Array) => new DataView(bytes.buffer, bytes.byteOffset).getUint32(16);
const noActions = { open() {}, show() {}, stop() {}, settings() {}, setup() {} };

test("the keyed items are the Tauri build's, and the page knows each", () => {
  const shell = read("../../src-tauri/src/shell.rs");
  const tauri = [...shell.matchAll(/\(\s*"([a-z-]+)",\s*"([^"]+)",\s*"([^"]+)",?\s*\)/g)].map((match) => match.slice(1, 4));
  assert.deepEqual(KEYED_ITEMS.map((each) => [...each]).toSorted(), tauri.toSorted());
  const keys = read("../../src/app/paneShortcuts.ts");
  for (const [id] of KEYED_ITEMS) assert.ok(keys.includes(`${id}:`) || keys.includes(`"${id}":`), `${id} is no key the page knows`);
});

test("the app menu has the Tauri build's menus, in its order, with our Quit in place of the standard one", () => {
  const menu = appMenuTemplate("Brigadier", () => {});
  assert.deepEqual(menu.map((each) => each.label ?? each.role), ["Brigadier", "File", "Edit", "View", "windowMenu", "help", "Panes", "Browser"]);
  const items = (at: number) => (menu[at]?.submenu as { type?: string; label?: string; role?: string }[]);
  assert.deepEqual(items(0).map((each) => each.type ?? each.label ?? each.role), [
    "about", "separator", "Settings…", "separator", "services", "separator", "hide", "hideOthers", "separator", "Uninstall Brigadier…", "separator", "Quit Brigadier",
  ]);
  assert.deepEqual(items(1).map((each) => each.type ?? each.label), ["Open Folder…", "separator", "Close tab, page or terminal", "Close Window"]);
  assert.equal(items(6).filter((each) => /^Show (chat|tab \d)$/.test(each.label ?? "")).length, 9);
});

test("a menu item says which it is", () => {
  const chosen: string[] = [];
  const [app] = appMenuTemplate("Brigadier", (id) => chosen.push(id));
  const quit = (app?.submenu as { label?: string; click?: () => void }[] | undefined)?.find((each) => each.label === "Quit Brigadier");
  quit?.click?.();
  assert.deepEqual(chosen, ["app-quit"]);
});

test("the Running list shows only with chats, cuts a long title, and opens the chat picked", () => {
  const picked: string[] = [];
  const act = { open() {}, quit() {}, chat: (id: string) => picked.push(id) };
  assert.deepEqual(trayTemplate([], act).map((each) => each.label ?? each.type), ["Open Brigadier", "separator", "Quit Brigadier"]);
  const menu = trayTemplate([{ id: "c1", title: `${"a".repeat(46)}  ${"b".repeat(10)}` }], act);
  assert.deepEqual(menu.map((each) => each.label ?? each.type), ["Open Brigadier", "separator", "Running", `${"a".repeat(46)}…`, "separator", "Quit Brigadier"]);
  assert.equal(menu[2]?.enabled, false);
  (menu[3] as { click: () => void }).click();
  assert.deepEqual(picked, ["c1"]);
});

test("setup shows only when it needs the user or the helper is in trouble", () => {
  for (const state of ["ready", "notSetUp", "off"] as const) assert.equal(setupLine({ state }), null);
  assert.deepEqual(setupLine({ state: "missing" }), { line: "Setup isn't finished", finish: true });
  assert.deepEqual(setupLine({ state: "restarting" }), { line: "The helper isn't responding; restarting it", finish: false });
  assert.equal(setupLine({ state: "failed", message: "gone" })?.finish, true);
});

test("long labels are cut with an ellipsis and the app stays whole", () => {
  assert.equal(sessionTitle("Fix the login", "TextEdit"), "Fix the login · TextEdit");
  const shown = sessionTitle("a".repeat(80), "System Settings");
  assert.equal([...shown].length, 48);
  assert.ok(shown.endsWith("… · System Settings"));
  assert.equal(plural(1), "1 app");
  assert.equal(plural(3), "3 apps");
});

test("the item shows only while an agent is using apps", () => {
  assert.equal(showing(true, null), null);
  assert.equal(showing(true, status([])), null);
  assert.equal(showing(true, status([session("a", "TextEdit")]))?.length, 1);
  assert.equal(showing(false, status([session("a", "TextEdit")])), null);
});

test("an action names the session its menu showed, and Open needs a conversation", () => {
  const stopped: string[] = [];
  const a = session("a", "TextEdit");
  const b = session("b", "Finder", "c1");
  const menu = computerTemplate(status([a, b], { state: "missing" }), [a, b], { ...noActions, stop: ({ session: id }) => stopped.push(id) });
  assert.deepEqual(menu.map((each) => each.label ?? each.type), [
    "Agents using apps", "a · TextEdit", "b · Finder", "separator", "Setup isn't finished", "Finish setup…", "separator", "Computer use settings…",
  ]);
  const submenu = (at: number) => menu[at]?.submenu as { label?: string; enabled?: boolean; click?: () => void }[];
  assert.equal(submenu(1)[0]?.enabled, false);
  assert.equal(submenu(2)[0]?.enabled, true);
  assert.equal(submenu(2)[3]?.label, "Stop using Finder");
  submenu(2)[3]?.click?.();
  assert.deepEqual(stopped, ["b"]);
});

test("the Windows icon is icon.ico's entry for the size, or its nearest larger one", () => {
  const ico = new Uint8Array(fs.readFileSync(new URL("../../src-tauri/icons/icon.ico", import.meta.url)));
  const png = icoEntry(ico, 16);
  assert.deepEqual([...png.subarray(1, 4)], [0x50, 0x4e, 0x47]);
  assert.equal(pngWidth(png), 16);
  assert.ok(pngWidth(icoEntry(ico, 18)) >= 18);
});
