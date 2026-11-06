// What the menu-bar items and the app menu show, as in src-tauri/src/shell.rs and computer_menu.rs: plain
// data and text, with no Electron in it, so the tests can read it. tray.ts makes the real menus from these.
import type { MenuItemConstructorOptions } from "electron";

import type { ComputerSession, ComputerSetup, ComputerUseStatus } from "@/ipc/generated";

type Template = MenuItemConstructorOptions[];
type Chats = readonly { id: string; title: string }[];

/** Longest title the "Running" list shows before cutting it with "…". */
const RUNNING_TITLE_CHARS = 48;
/** A session's title longer than this has its label cut, with an ellipsis; the app stays whole. */
const LABEL_CHARS = 48;

const SEPARATOR: MenuItemConstructorOptions = { type: "separator" };

/** Cuts `text` by whole characters, as Rust's `chars` counts them. */
function cut(text: string, room: number): string {
  const characters = [...text];
  return characters.length > room ? `${characters.slice(0, room - 1).join("").trimEnd()}…` : text;
}

/** The menu-bar item's menu: open, the conversations running now ("Running"), quit. */
export function trayTemplate(running: Chats, act: { open(): void; quit(): void; chat(id: string): void }): Template {
  return [
    { label: "Open Brigadier", click: () => act.open() },
    SEPARATOR,
    ...(running.length === 0 ? [] : [
      { label: "Running", enabled: false },
      ...running.map(({ id, title }) => ({ label: cut(title, RUNNING_TITLE_CHARS), click: () => act.chat(id) })),
      SEPARATOR,
    ]),
    { label: "Quit Brigadier", click: () => act.quit() },
  ];
}

export function plural(apps: number): string {
  return apps === 1 ? "1 app" : `${apps} apps`;
}

/** A session's title, "<label> · <app>": a long label is cut, never the app's name. */
export function sessionTitle(label: string, app: string): string {
  return `${cut(label, Math.max(LABEL_CHARS - ([...app].length + 3), 8))} · ${app}`;
}

/** A line on setup, when it needs the user or the helper is in trouble, and whether it offers to finish. */
export function setupLine(setup: ComputerSetup): { line: string; finish: boolean } | null {
  switch (setup.state) {
    case "missing":
    case "confirmCapture":
      return { line: "Setup isn't finished", finish: true };
    case "restarting":
      return { line: "The helper isn't responding; restarting it", finish: false };
    case "failed":
      return { line: "Computer use isn't working right now", finish: true };
    default:
      return null;
  }
}

/** The sessions the item lists, or null when it's hidden: it shows only while an agent is using apps. */
export function showing(shown: boolean, status: ComputerUseStatus | null): ComputerSession[] | null {
  return shown && status && status.sessions.length > 0 ? status.sessions : null;
}

export type ComputerActions = {
  open(session: ComputerSession): void;
  show(session: ComputerSession): void;
  stop(session: ComputerSession): void;
  settings(): void;
  setup(): void;
};

/**
 * The computer-use item's menu. Each action holds the session its menu showed, even when the status
 * changed while the menu was open.
 */
export function computerTemplate(status: ComputerUseStatus, sessions: ComputerSession[], act: ComputerActions): Template {
  const setup = setupLine(status.setup);
  return [
    { label: "Agents using apps", enabled: false },
    ...sessions.map((session) => ({
      label: sessionTitle(session.label, session.app),
      submenu: [
        { label: "Open in Brigadier", enabled: Boolean(session.conversationId), click: () => act.open(session) },
        { label: `Show ${session.app}`, click: () => act.show(session) },
        SEPARATOR,
        { label: `Stop using ${session.app}`, click: () => act.stop(session) },
      ],
    })),
    ...(setup ? [
      SEPARATOR,
      { label: setup.line, enabled: false },
      ...(setup.finish ? [{ label: "Finish setup…", click: () => act.setup() }] : []),
    ] : []),
    SEPARATOR,
    { label: "Computer use settings…", click: () => act.settings() },
  ];
}

export const OPEN_FOLDER_ITEM = "open-folder";
export const UNINSTALL_ITEM = "uninstall";
export const SETTINGS_ITEM = "settings";
export const FEEDBACK_ITEM = "send-feedback";
export const QUIT_ITEM = "app-quit";
export const CLOSE_WINDOW_ITEM = "close-main-window";

// The items the page handles as keys: its id (after "pane:"), title and accelerator.
const VIEW_ITEMS = [
  ["sidebar", "Toggle Sidebar", "CmdOrCtrl+B"],
  ["right-sidebar", "Toggle Right Sidebar", "Alt+CmdOrCtrl+B"],
  ["show-files", "Files", "CmdOrCtrl+Shift+E"],
  ["show-search", "Search", "CmdOrCtrl+Shift+F"],
  ["show-source", "Source", "Ctrl+Shift+G"],
  ["back", "Back", "CmdOrCtrl+["],
  ["forward", "Forward", "CmdOrCtrl+]"],
] as const;

const PANE_ITEMS = [
  ["terminal", "Toggle bottom terminal", "CmdOrCtrl+J"],
  ["terminal-alternate", "Terminal", "Ctrl+`"],
  ["new", "New tab", "CmdOrCtrl+T"],
  ["new-side-chat", "New side chat", "Alt+CmdOrCtrl+S"],
  ["new-page", "New page", "Alt+CmdOrCtrl+N"],
  ["save", "Save file", "CmdOrCtrl+S"],
  ["save-as", "Save file as…", "CmdOrCtrl+Shift+S"],
  ["files", "Find file", "CmdOrCtrl+P"],
  ["show-pages", "Show pages", "Alt+CmdOrCtrl+E"],
  ["cycle-next", "Next session tab", "Ctrl+Tab"],
  ["cycle-previous", "Previous session tab", "Ctrl+Shift+Tab"],
  ["reopen", "Reopen closed tab, page or terminal", "CmdOrCtrl+Shift+T"],
  ["previous", "Previous tab, page or terminal", "CmdOrCtrl+Shift+["],
  ["next", "Next tab, page or terminal", "CmdOrCtrl+Shift+]"],
  ["previous-tab", "Previous tab", "Alt+CmdOrCtrl+Left"],
  ["next-tab", "Next tab", "Alt+CmdOrCtrl+Right"],
  ["close-others", "Close other tabs", "Alt+CmdOrCtrl+W"],
] as const;

/** The Browser menu's items, in groups: what opens a page, then what acts on the page in front. */
const BROWSER_ITEMS = [
  [
    ["new-browser", "New browser tab", "CmdOrCtrl+Shift+B"],
    ["address", "Focus browser address", "CmdOrCtrl+L"],
  ],
  [
    ["page-back", "Page back", "CmdOrCtrl+Left"],
    ["page-forward", "Page forward", "CmdOrCtrl+Right"],
    ["reload", "Reload page", "CmdOrCtrl+R"],
    ["force-reload", "Reload page without the cache", "CmdOrCtrl+Shift+R"],
  ],
  [
    ["find", "Find…", "CmdOrCtrl+F"],
    ["find-next", "Find next", "CmdOrCtrl+G"],
    ["find-previous", "Find previous", "CmdOrCtrl+Shift+G"],
  ],
  [
    ["zoom-in", "Zoom in", "CmdOrCtrl+="],
    ["zoom-out", "Zoom out", "CmdOrCtrl+-"],
    ["zoom-reset", "Actual size", "CmdOrCtrl+0"],
  ],
] as const;

/**
 * The macOS app menu: the standard one Tauri makes, with the app's own items in it. Its Quit is ours, the
 * orderly one. `chosen` hears each own item's id; the standard ones are Electron's roles.
 */
export function appMenuTemplate(appName: string, chosen: (id: string) => void): Template {
  const item = (id: string, label: string, accelerator?: string): MenuItemConstructorOptions => ({
    id,
    label,
    ...(accelerator ? { accelerator } : {}),
    click: () => chosen(id),
  });
  const keyed = (items: readonly (readonly [string, string, string])[]) =>
    items.map(([id, label, accelerator]) => item(`pane:${id}`, label, accelerator));
  return [
    {
      label: appName,
      submenu: [
        { role: "about" },
        SEPARATOR,
        item(SETTINGS_ITEM, "Settings…", "CmdOrCtrl+,"),
        SEPARATOR,
        { role: "services" },
        SEPARATOR,
        { role: "hide" },
        { role: "hideOthers" },
        SEPARATOR,
        item(UNINSTALL_ITEM, "Uninstall Brigadier…"),
        SEPARATOR,
        item(QUIT_ITEM, `Quit ${appName}`, "CmdOrCtrl+Q"),
      ],
    },
    {
      label: "File",
      submenu: [
        item(OPEN_FOLDER_ITEM, "Open Folder…", "CmdOrCtrl+O"),
        SEPARATOR,
        // Cmd+W belongs to the focused browser page or terminal shell, else a session's tab in front.
        item("pane:close", "Close tab, page or terminal", "CmdOrCtrl+W"),
        item(CLOSE_WINDOW_ITEM, "Close Window", "CmdOrCtrl+Shift+W"),
      ],
    },
    {
      label: "Edit",
      submenu: [
        { role: "undo" },
        { role: "redo" },
        SEPARATOR,
        { role: "cut" },
        { role: "copy" },
        { role: "paste" },
        { role: "selectAll" },
      ],
    },
    { label: "View", submenu: [{ role: "togglefullscreen" }, SEPARATOR, ...keyed(VIEW_ITEMS)] },
    { role: "windowMenu", submenu: [{ role: "minimize" }, { role: "zoom" }, SEPARATOR, { role: "close" }] },
    { role: "help", submenu: [item(FEEDBACK_ITEM, "Send Feedback…")] },
    {
      label: "Panes",
      submenu: [
        ...keyed(PANE_ITEMS),
        ...Array.from({ length: 9 }, (_, index) =>
          item(`pane:tab-${index + 1}`, index === 0 ? "Show chat" : `Show tab ${index + 1}`, `CmdOrCtrl+${index + 1}`)),
      ],
    },
    {
      label: "Browser",
      submenu: BROWSER_ITEMS.flatMap((group, at) => [...(at > 0 ? [SEPARATOR] : []), ...keyed(group)]),
    },
  ];
}

/** The items' ids, titles and accelerators the page handles as keys, for the test against the Tauri build. */
export const KEYED_ITEMS: readonly (readonly [string, string, string])[] = [...VIEW_ITEMS, ...PANE_ITEMS, ...BROWSER_ITEMS.flat()];

/** An icon.ico entry for an icon of `wanted` px, or its nearest larger one: its PNG. */
export function icoEntry(ico: Uint8Array, wanted: number): Uint8Array {
  const view = new DataView(ico.buffer, ico.byteOffset, ico.byteLength);
  const entries = Array.from({ length: view.getUint16(4, true) }, (_, index) => {
    const at = 6 + 16 * index;
    const size = view.getUint8(at) || 256;
    return { size, png: ico.subarray(view.getUint32(at + 12, true), view.getUint32(at + 12, true) + view.getUint32(at + 8, true)) };
  });
  const rank = ({ size }: { size: number }) => (size < wanted ? 1_000_000 : 0) + Math.abs(size - wanted);
  const best = entries.reduce<(typeof entries)[number] | null>((chosen, each) => (!chosen || rank(each) < rank(chosen) ? each : chosen), null);
  if (!best) throw new Error("icon.ico has no entries");
  return best.png;
}
