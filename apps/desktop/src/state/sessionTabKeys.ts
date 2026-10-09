import type { NewTabKind } from "./sessionTabs";

type Key = Pick<KeyboardEvent, "code" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey" | "isComposing"> & {
  getModifierState?: (key: string) => boolean;
};
export type TabAction = { type: "new"; kind: NewTabKind } | { type: "close" } |
  { type: "number"; number: number } | { type: "step"; step: 1 | -1 };

export function sessionTabKey(event: Key, mac: boolean): TabAction | null {
  if (event.isComposing || event.getModifierState?.("AltGraph")) return null;
  if (event.ctrlKey && !event.metaKey && !event.altKey && event.code === "Tab")
    return { type: "step", step: event.shiftKey ? -1 : 1 };
  if (!(mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey)) return null;
  if (event.altKey) {
    if (event.shiftKey) return null;
    if (event.code === "KeyS") return { type: "new", kind: "sideChat" };
    if (event.code === "KeyN") return { type: "new", kind: "document" };
    return null;
  }
  if (event.shiftKey) {
    if (event.code === "KeyB") return { type: "new", kind: "browser" };
    if (event.code === "BracketLeft" || event.code === "BracketRight")
      return { type: "step", step: event.code === "BracketLeft" ? -1 : 1 };
    return null;
  }
  if (event.code === "KeyT") return { type: "new", kind: "terminal" };
  if (event.code === "KeyW") return { type: "close" };
  const digit = /^Digit([1-9])$/.exec(event.code);
  return digit ? { type: "number", number: Number(digit[1]) } : null;
}

export const NEW_TAB_MENU: readonly { kind: NewTabKind; label: string; shortcut: string }[] = [
  { kind: "terminal", label: "New terminal tab", shortcut: "⌘T" },
  { kind: "browser", label: "New browser tab", shortcut: "⌘⇧B" },
  { kind: "sideChat", label: "New side chat", shortcut: "⌥⌘S" },
  { kind: "document", label: "New file", shortcut: "⌥⌘N" },
];
