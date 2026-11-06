// A focused <webview> holds the keyboard: a key goes to its page first and the host page never sees it. The
// shortcuts that run the browser and its tabs are taken here, before the page, and go on as the menu's items do
// (src/app/paneShortcuts.ts). Every other key stays the page's: ⌘B or ⌘S in an editor, ⌘← in a field, typing.
// One the page leaves unhandled still reaches the app menu.
import type { Input, WebContents } from "electron";

type Press = Pick<Input, "type" | "key" | "code" | "meta" | "control" | "alt" | "shift" | "isComposing">;

/** ⌘ and the key, as the character it types: a menu accelerator follows the layout, not the physical key. */
const COMMAND: Record<string, string> = {
  t: "new", w: "close", l: "address", f: "find", g: "find-next", r: "reload",
  "=": "zoom-in", "+": "zoom-in", "-": "zoom-out", "0": "zoom-reset",
  "[": "back", "]": "forward",
  ...Object.fromEntries(Array.from({ length: 9 }, (_, index) => [String(index + 1), `tab-${index + 1}`])),
};
const COMMAND_SHIFT: Record<string, string> = {
  t: "reopen", r: "force-reload", g: "find-previous", b: "new-browser",
  "=": "zoom-in", "+": "zoom-in",
  "[": "previous", "{": "previous", "]": "next", "}": "next",
};
const COMMAND_ALT: Record<string, string> = {
  w: "close-others", s: "new-side-chat", n: "new-page",
  arrowleft: "previous-tab", arrowright: "next-tab",
};

/** What the key types, lowercase. Alt changes the character and layouts move the digits, so those read the key's place. */
function typed({ key, code, alt }: Press): string {
  const place = /^Key([A-Z])$/.exec(code)?.[1]?.toLowerCase() ?? /^Digit(\d)$/.exec(code)?.[1];
  if (alt) return place ?? key.toLowerCase();
  if (key.length === 1 && key >= " " && key <= "~" && !/^Digit/.test(code)) return key.toLowerCase();
  return place ?? key.toLowerCase();
}

/** The `MENU_KEYS` name of a press the app takes ahead of the page; null for one the page keeps. */
export function guestKey(press: Press): string | null {
  if (press.type !== "keyDown" || press.isComposing) return null;
  const { meta, control, alt, shift } = press;
  if (control && !meta && !alt) {
    if (press.code === "Tab") return shift ? "cycle-previous" : "cycle-next";
    if (press.code === "Backquote" && !shift) return "terminal-alternate";
    return null;
  }
  if (!meta || control) return null;
  const table = alt ? (shift ? null : COMMAND_ALT) : shift ? COMMAND_SHIFT : COMMAND;
  return table?.[typed(press)] ?? null;
}

/** Which profile a press switches to, 0-based: ⌥⌘1-9 on macOS, Ctrl+Shift+1-9 elsewhere; null for any other press. */
export function profileKey(press: Press, mac: boolean): number | null {
  if (press.type !== "keyDown" || press.isComposing) return null;
  const { meta, control, alt, shift } = press;
  if (!(mac ? alt && meta && !control && !shift : control && shift && !alt && !meta)) return null;
  const digit = /^(?:Digit|Numpad)([1-9])$/.exec(press.code)?.[1];
  return digit ? Number(digit) - 1 : null;
}

/** Off macOS the page hears no pane shortcut yet (usePaneShortcuts), so there a key taken here would be lost; a profile key has its own event. */
export function installGuestKeys(guest: WebContents): void {
  const mac = process.platform === "darwin";
  guest.on("before-input-event", (event, input) => {
    const profile = profileKey(input, mac);
    if (profile !== null) {
      event.preventDefault();
      guest.hostWebContents?.send("brigadier:guest-key");
      guest.hostWebContents?.send("brigadier:event", "browser-profile-key", profile);
      return;
    }
    if (!mac) return;
    // Not taken: the page's own Esc still happens; the find bar only hears of it.
    if (input.type === "keyDown" && input.key === "Escape") guest.hostWebContents?.send("brigadier:guest-escape", { from: guest.id });
    const key = guestKey(input);
    if (!key) return;
    event.preventDefault();
    // Sent first: what the shortcut does may be to put the keyboard in a field of the app (tabLayer's takeKeyboard).
    guest.hostWebContents?.send("brigadier:guest-key");
    guest.hostWebContents?.send("brigadier:event", "pane-shortcut", key);
  });
}
