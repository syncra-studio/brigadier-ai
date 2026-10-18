/**
 * Every keyboard shortcut Brigadier has, in one list: Settings › Keyboard shortcuts shows it, and
 * tooltips read their key labels from it. Each entry names its keys in macOS notation, modifiers
 * in the order ⌃⌥⇧⌘ (`formatKeys` writes them as Ctrl/Alt/Shift elsewhere, where ⌘ is Ctrl).
 * The handlers still match their own keys; `menu` names the macOS menu item that owns an entry's
 * first key, and shortcutCatalog.test.ts checks the two agree.
 */

import type { FollowUp } from "@/ipc/generated";

export type ShortcutArea =
  | "General"
  | "Conversations"
  | "Tabs and panes"
  | "Composer"
  | "Requests"
  | "Files and browser";

export type Shortcut = {
  /** Stable, for a future where keys can be changed. */
  id: string;
  name: string;
  line: string;
  area: ShortcutArea;
  /**
   * One chip per key. "⌘1–9" is a range of digits; "Esc Esc" is two presses.
   */
  keys: readonly string[];
  /** The Panes, Browser or View menu item (macOS) that owns the first key, as `MENU_KEYS` names it. */
  menu?: string;
  /** Only on macOS (its menu bar holds it). */
  macOnly?: boolean;
};

export const SHORTCUTS: readonly Shortcut[] = [
  // General
  { id: "settings", name: "Settings", line: "Open Settings, or leave it", area: "General", keys: ["⌘,"] },
  { id: "search", name: "Search", line: "Search projects and conversations", area: "General", keys: ["⌘K"] },
  { id: "open-folder", name: "Open folder", line: "Add a folder as a project", area: "General", keys: ["⌘O"] },
  { id: "sidebar", name: "Toggle sidebar", line: "Show or hide the sidebar", area: "General", keys: ["⌘B", "⇧⌘S"], menu: "sidebar" },
  { id: "back", name: "Back", line: "Go back to the place shown before", area: "General", keys: ["⌘["], menu: "back" },
  { id: "forward", name: "Forward", line: "Go forward again", area: "General", keys: ["⌘]"], menu: "forward" },
  { id: "inspector", name: "Inspector", line: "Show or hide the developer inspector", area: "General", keys: ["⌥⌘I"] },
  { id: "close-window", name: "Close window", line: "Close Brigadier's window", area: "General", keys: ["⇧⌘W"], macOnly: true },

  // Conversations
  {
    id: "archive",
    name: "Archive",
    line: "Archive the open conversation, or the ones picked in the sidebar",
    area: "Conversations",
    keys: ["⇧⌘A"],
  },
  { id: "undo-archive", name: "Undo archive", line: "Bring back what was just archived", area: "Conversations", keys: ["⌘Z"] },
  { id: "rename", name: "Rename", line: "Rename the open conversation", area: "Conversations", keys: ["⌥⌘R"] },
  { id: "pin", name: "Pin", line: "Pin or unpin the open conversation", area: "Conversations", keys: ["⌥⌘P"] },
  {
    id: "delete-picked",
    name: "Delete picked",
    line: "Delete the conversations picked in the sidebar",
    area: "Conversations",
    keys: ["⌘⌫"],
  },
  { id: "clear-picked", name: "Clear picked", line: "Let go of the rows picked in the sidebar", area: "Conversations", keys: ["Esc"] },
  {
    id: "previous-message",
    name: "Previous message",
    line: "Scroll the thread to your message before the one at the top, loading older ones as needed",
    area: "Conversations",
    keys: ["⌥↑"],
  },
  { id: "next-message", name: "Next message", line: "Scroll the thread to your next message", area: "Conversations", keys: ["⌥↓"] },

  // Tabs and panes
  { id: "new-tab", name: "New tab", line: "Open a new tab in the session", area: "Tabs and panes", keys: ["⌘T"], menu: "new" },
  { id: "new-browser", name: "New browser tab", line: "Open a web page in a tab", area: "Tabs and panes", keys: ["⇧⌘B"], menu: "new-browser" },
  { id: "new-side-chat", name: "New side chat", line: "Ask something beside the session", area: "Tabs and panes", keys: ["⌥⌘S"], menu: "new-side-chat" },
  { id: "new-page", name: "New page", line: "Start a page of notes in a tab, saved as you type", area: "Tabs and panes", keys: ["⌥⌘N"], menu: "new-page" },
  { id: "close", name: "Close tab", line: "Close the tab, page or terminal in front", area: "Tabs and panes", keys: ["⌘W"], menu: "close" },
  { id: "close-others", name: "Close other tabs", line: "Close every tab but Chat and the one in front", area: "Tabs and panes", keys: ["⌥⌘W"], menu: "close-others" },
  { id: "close-focused", name: "Close focused tab", line: "Close the tab that has the keyboard in the tab strip", area: "Tabs and panes", keys: ["⌫"] },
  { id: "reopen", name: "Reopen closed tab", line: "Bring back the last tab, page or terminal closed", area: "Tabs and panes", keys: ["⇧⌘T"], menu: "reopen" },
  { id: "previous", name: "Previous tab", line: "Show the tab, page or terminal to the left", area: "Tabs and panes", keys: ["⇧⌘[", "⌥⌘←"], menu: "previous" },
  { id: "next", name: "Next tab", line: "Show the tab, page or terminal to the right", area: "Tabs and panes", keys: ["⇧⌘]", "⌥⌘→"], menu: "next" },
  { id: "cycle-next", name: "Next session tab", line: "Go round the session's tabs", area: "Tabs and panes", keys: ["⌃Tab"], menu: "cycle-next" },
  {
    id: "cycle-previous",
    name: "Previous session tab",
    line: "Go round the session's tabs the other way",
    area: "Tabs and panes",
    keys: ["⌃⇧Tab"],
    menu: "cycle-previous",
  },
  { id: "show-tab", name: "Show tab", line: "⌘1 shows the chat, ⌘2 to ⌘9 the tabs after it", area: "Tabs and panes", keys: ["⌘1–9"], menu: "tab-1" },
  { id: "terminal", name: "Toggle terminal", line: "Show or hide the bottom terminal", area: "Tabs and panes", keys: ["⌘J"], menu: "terminal" },
  {
    id: "new-terminal",
    name: "Terminal",
    line: "A new terminal tab in a live session; elsewhere, the bottom terminal",
    area: "Tabs and panes",
    keys: ["⌃`"],
    menu: "terminal-alternate",
  },
  { id: "files", name: "Find file", line: "Search the session's files", area: "Tabs and panes", keys: ["⌘P"], menu: "files" },
  { id: "right-sidebar", name: "Toggle right sidebar", line: "Show or hide the session's files and changes", area: "Tabs and panes", keys: ["⌥⌘B"], menu: "right-sidebar" },
  { id: "show-files", name: "Files", line: "Show the session's files in the right sidebar", area: "Tabs and panes", keys: ["⇧⌘E"], menu: "show-files" },
  { id: "show-search", name: "Search", line: "Search the session's files in the right sidebar", area: "Tabs and panes", keys: ["⇧⌘F"], menu: "show-search" },
  { id: "show-source", name: "Source", line: "Show the session's changes in the right sidebar", area: "Tabs and panes", keys: ["⌃⇧G"], menu: "show-source" },
  { id: "show-pages", name: "Show pages", line: "List the pages, from a page in front", area: "Tabs and panes", keys: ["⌥⌘E"], menu: "show-pages" },

  // Composer
  // The follow-up's other way as with the default follow-up; `shortcutsFor` names it for the
  // one chosen in Settings › General › Composer.
  { id: "send", name: "Send", line: "Send the message", area: "Composer", keys: ["⏎"] },
  { id: "new-line", name: "New line", line: "Start a new line in the message", area: "Composer", keys: ["⇧⏎"] },
  {
    id: "follow-up-other",
    name: "Steer",
    line: "While an answer works, send the message into it now",
    area: "Composer",
    keys: ["⌥⏎"],
  },
  {
    id: "start-in-tab",
    name: "Start as a new session",
    line: "Send the message as a new session in the background, and stay here",
    area: "Composer",
    keys: ["⌘⏎"],
  },
  {
    id: "history",
    name: "Earlier prompts",
    line: "Walk back through your prompts, and forward again; in an empty field ↑ takes back a queued message first",
    area: "Composer",
    keys: ["↑", "↓"],
  },
  { id: "dictate", name: "Dictate", line: "Start or stop dictation", area: "Composer", keys: ["⌃⇧D"] },
  { id: "commands", name: "Commands", line: "Open the commands menu", area: "Composer", keys: ["/"] },
  { id: "mention", name: "Mention", line: "Mention a file or a conversation", area: "Composer", keys: ["@"] },
  { id: "stop", name: "Stop", line: "Stop the reply that is running", area: "Composer", keys: ["Esc Esc"] },

  // Requests
  { id: "allow", name: "Allow", line: "Answer the request card in front with its main choice", area: "Requests", keys: ["⏎"] },
  { id: "decline", name: "Decline", line: "Turn down the request card in front", area: "Requests", keys: ["Esc"] },
  { id: "answer", name: "Pick an answer", line: "Choose one of a question's answers by its number", area: "Requests", keys: ["1–9"] },

  // Files and browser
  { id: "save", name: "Save file", line: "Save the file in front", area: "Files and browser", keys: ["⌘S"], menu: "save" },
  { id: "save-as", name: "Save file as", line: "Save the file in front under a new name", area: "Files and browser", keys: ["⇧⌘S"], menu: "save-as" },
  { id: "find", name: "Find", line: "Find text in the chat, or in the file or web page in front", area: "Files and browser", keys: ["⌘F"], menu: "find" },
  { id: "find-next", name: "Find next", line: "Go to the next match in the chat or web page", area: "Files and browser", keys: ["⌘G"], menu: "find-next" },
  { id: "find-previous", name: "Find previous", line: "Go to the previous match in the chat or web page", area: "Files and browser", keys: ["⇧⌘G"], menu: "find-previous" },
  { id: "go-to-line", name: "Go to line", line: "Jump to a line of the file in front", area: "Files and browser", keys: ["⌃G"] },
  { id: "address", name: "Browser address", line: "Type an address in the browser tab", area: "Files and browser", keys: ["⌘L"], menu: "address" },
  { id: "page-back", name: "Page back", line: "Go back a page in the browser tab", area: "Files and browser", keys: ["⌘←"], menu: "page-back" },
  { id: "page-forward", name: "Page forward", line: "Go forward a page in the browser tab", area: "Files and browser", keys: ["⌘→"], menu: "page-forward" },
  { id: "reload", name: "Reload page", line: "Load the browser tab's page again", area: "Files and browser", keys: ["⌘R"], menu: "reload" },
  {
    id: "force-reload",
    name: "Reload without the cache",
    line: "Load the page and everything on it fresh from the site",
    area: "Files and browser",
    keys: ["⇧⌘R"],
    menu: "force-reload",
  },
  { id: "zoom-in", name: "Zoom in", line: "Make the web page larger", area: "Files and browser", keys: ["⌘="], menu: "zoom-in" },
  { id: "zoom-out", name: "Zoom out", line: "Make the web page smaller", area: "Files and browser", keys: ["⌘-"], menu: "zoom-out" },
  { id: "zoom-reset", name: "Actual size", line: "Show the web page at 100%", area: "Files and browser", keys: ["⌘0"], menu: "zoom-reset" },
  { id: "commit", name: "Commit", line: "Commit with the message typed", area: "Files and browser", keys: ["⌘⏎"] },
  { id: "review-keep", name: "Keep change", line: "Keep the change in view of the file under review", area: "Files and browser", keys: ["⌘Y"] },
  { id: "review-undo", name: "Undo change", line: "Undo the change in view of the file under review", area: "Files and browser", keys: ["⌘N"] },
  { id: "review-keep-file", name: "Keep file", line: "Keep every change of the file under review", area: "Files and browser", keys: ["⌘⏎"] },
  { id: "review-next-file", name: "Next file to review", line: "Open the next file with changes to review", area: "Files and browser", keys: ["⌥L"] },
  { id: "review-previous-file", name: "Previous file to review", line: "Open the previous file with changes to review", area: "Files and browser", keys: ["⌥H"] },
];

/**
 * The built-in browser's shortcuts, gone while it is off (Settings › Browser). Finding stays: ⌘F,
 * ⌘G and ⇧⌘G find in the chat as well.
 */
export const BROWSER_SHORTCUTS: ReadonlySet<string> = new Set([
  "new-browser",
  "address",
  "page-back",
  "page-forward",
  "reload",
  "force-reload",
  "zoom-in",
  "zoom-out",
  "zoom-reset",
]);

/** How the composer sends, as Settings › General › Composer sets it. */
export type ComposerKeys = { followUp: FollowUp };

const DEFAULT_COMPOSER_KEYS: ComposerKeys = { followUp: "queue" };

/** The composer's entries with the follow-up's other way named for the follow-up chosen. */
function withComposerKeys(shortcut: Shortcut, { followUp }: ComposerKeys): Shortcut {
  if (shortcut.id !== "follow-up-other" || followUp === "queue") return shortcut;
  return { ...shortcut, name: "Queue", line: "While an answer works, send the message to wait its turn" };
}

/**
 * The shortcuts this platform has, with the browser's only while it is on and the composer's
 * keys as chosen.
 */
export function shortcutsFor(mac: boolean, browser = true, composer = DEFAULT_COMPOSER_KEYS): Shortcut[] {
  return SHORTCUTS.filter(
    (shortcut) => (mac || !shortcut.macOnly) && (browser || !BROWSER_SHORTCUTS.has(shortcut.id)),
  ).map((shortcut) => withComposerKeys(shortcut, composer));
}

export type Stroke = { ctrl: boolean; alt: boolean; shift: boolean; cmd: boolean; key: string };

const MODIFIERS: Record<string, "ctrl" | "alt" | "shift" | "cmd"> = { "⌃": "ctrl", "⌥": "alt", "⇧": "shift", "⌘": "cmd" };

/** One key press of macOS notation ("⌃⇧D"). */
export function parseStroke(text: string): Stroke {
  const stroke: Stroke = { ctrl: false, alt: false, shift: false, cmd: false, key: "" };
  let rest = text;
  for (let modifier = MODIFIERS[rest.charAt(0)]; rest.length > 1 && modifier; modifier = MODIFIERS[rest.charAt(0)]) {
    stroke[modifier] = true;
    rest = rest.slice(1);
  }
  stroke.key = rest;
  return stroke;
}

function writeStroke(stroke: Stroke, mac: boolean): string {
  if (mac) {
    return `${stroke.ctrl ? "⌃" : ""}${stroke.alt ? "⌥" : ""}${stroke.shift ? "⇧" : ""}${stroke.cmd ? "⌘" : ""}${stroke.key}`;
  }
  const key = stroke.key === "⏎" ? "Enter" : stroke.key === "⌫" ? "Backspace" : stroke.key;
  return [stroke.ctrl || stroke.cmd ? "Ctrl" : "", stroke.alt ? "Alt" : "", stroke.shift ? "Shift" : "", key]
    .filter(Boolean)
    .join("+");
}

/** A chip's keys as this platform writes them: "⇧⌘A" on macOS, "Ctrl+Shift+A" elsewhere. */
export function formatKeys(keys: string, mac: boolean): string {
  return keys
    .split(" ")
    .map((stroke) => writeStroke(parseStroke(stroke), mac))
    .join(" ");
}

/** The keys of a shortcut by id, for a tooltip ("" when there is none). */
export function shortcutKeys(id: string, mac: boolean): string {
  const shortcut = SHORTCUTS.find((entry) => entry.id === id);
  const first = shortcut?.keys[0];
  return first ? formatKeys(first, mac) : "";
}

const CODES: Record<string, string> = {
  BracketLeft: "[",
  BracketRight: "]",
  Comma: ",",
  Period: ".",
  Slash: "/",
  Backslash: "\\",
  Semicolon: ";",
  Quote: "'",
  Backquote: "`",
  Minus: "-",
  Equal: "=",
  Enter: "⏎",
  NumpadEnter: "⏎",
  Escape: "Esc",
  Backspace: "⌫",
  Delete: "⌫",
  Tab: "Tab",
  Space: "Space",
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
};

type Press = Pick<KeyboardEvent, "code" | "key" | "ctrlKey" | "altKey" | "shiftKey" | "metaKey">;

/**
 * A key press in macOS notation, or null for a modifier alone. A plain symbol typed with Shift
 * ("@") is the symbol, as the composer reads it.
 */
export function strokeOf(event: Press): string | null {
  if (["Control", "Alt", "Shift", "Meta", "CapsLock", "Fn", "AltGraph"].includes(event.key)) return null;
  const letter = /^Key([A-Z])$/.exec(event.code)?.[1];
  const digit = /^(?:Digit|Numpad)([0-9])$/.exec(event.code)?.[1];
  const plain = !event.ctrlKey && !event.altKey && !event.metaKey;
  if (plain && !letter && !CODES[event.code] && event.key.length === 1 && !/^\d$/.test(event.key)) return event.key;
  const key = letter ?? digit ?? CODES[event.code] ?? (event.key.length === 1 ? event.key.toUpperCase() : event.key);
  return writeStroke(
    { ctrl: event.ctrlKey, alt: event.altKey, shift: event.shiftKey, cmd: event.metaKey, key },
    true,
  );
}

/** Whether a press (in macOS notation) is one of a chip's keys on this platform. */
function strokeMatches(chip: string, press: string, mac: boolean): boolean {
  const pressed = parseStroke(press);
  return chip.split(" ").some((part) => {
    const stroke = parseStroke(part);
    const range = /^(\d)–(\d)$/.exec(stroke.key);
    const [, low = "", high = ""] = range ?? [];
    const key = range && /^\d$/.test(pressed.key) && pressed.key >= low && pressed.key <= high ? pressed.key : stroke.key;
    return writeStroke({ ...stroke, key }, mac) === writeStroke(pressed, mac);
  });
}

/** The shortcuts a search finds: by words in the name or line, or by a key press. */
export function searchShortcuts(
  shortcuts: readonly Shortcut[],
  search: { words: string } | { press: string },
  mac: boolean,
): Shortcut[] {
  if ("press" in search) {
    return shortcuts.filter((shortcut) => shortcut.keys.some((chip) => strokeMatches(chip, search.press, mac)));
  }
  const words = search.words.toLowerCase().split(/\s+/).filter(Boolean);
  return shortcuts.filter((shortcut) => {
    const text = `${shortcut.name} ${shortcut.line} ${shortcut.area}`.toLowerCase();
    return words.every((word) => text.includes(word));
  });
}
