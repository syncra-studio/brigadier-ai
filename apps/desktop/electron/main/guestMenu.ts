// A page's right-click menu. The OS draws it, so no pane or window edge can clip it. What it offers follows
// the thing clicked, as in a normal browser: Back, Forward and Reload only on the page itself. Inspect comes
// with section 9.
import { BrowserWindow, clipboard, Menu, type ContextMenuParams, type MenuItemConstructorOptions, type WebContents } from "electron";

import { note } from "./events";
import { unfocused } from "./identity";
import { openAsTab } from "./guestPopups";

type Item = { id: string; label: string; enabled?: boolean; run: () => void };

/** A dev run must not write the user's clipboard or raise a dialog (L8): it says what it would have done. */
const outward = (what: string, run: () => void) => () => {
  if (unfocused) note("guest-menu-effect", { what });
  else run();
};

function sections(contents: WebContents, params: ContextMenuParams): Item[][] {
  const { editFlags, isEditable, linkURL, mediaType, selectionText, srcURL, x, y } = params;
  const found: Item[][] = [];
  if (linkURL) {
    found.push([
      ...(/^https?:/.test(linkURL) ? [{ id: "open-link", label: "Open Link in New Tab", run: () => openAsTab(contents, linkURL) }] : []),
      { id: "copy-link", label: "Copy Link Address", run: outward(`copy-link ${linkURL}`, () => clipboard.writeText(linkURL)) },
    ]);
  }
  if (mediaType === "image" && srcURL) {
    found.push([
      { id: "save-image", label: "Save Image As…", run: outward(`save-image ${srcURL}`, () => contents.downloadURL(srcURL)) },
      { id: "copy-image", label: "Copy Image", run: outward(`copy-image ${x},${y}`, () => contents.copyImageAt(x, y)) },
    ]);
  }
  if (isEditable) {
    found.push([
      { id: "cut", label: "Cut", enabled: editFlags.canCut, run: outward("cut", () => contents.cut()) },
      { id: "copy", label: "Copy", enabled: editFlags.canCopy, run: outward("copy", () => contents.copy()) },
      { id: "paste", label: "Paste", enabled: editFlags.canPaste, run: outward("paste", () => contents.paste()) },
    ]);
  } else if (selectionText.trim()) {
    found.push([{ id: "copy", label: "Copy", run: outward("copy", () => contents.copy()) }]);
  }
  // A Mac right-click selects the word under it, so text alone mustn't take these away.
  if (linkURL || isEditable || (mediaType === "image" && srcURL)) return found;
  const history = contents.navigationHistory;
  return [...found, [
    { id: "back", label: "Back", enabled: history.canGoBack(), run: () => history.goBack() },
    { id: "forward", label: "Forward", enabled: history.canGoForward(), run: () => history.goForward() },
    { id: "reload", label: "Reload", run: () => contents.reload() },
  ]];
}

let lastMenu: Menu | null = null;

export function installGuestMenu(contents: WebContents): void {
  contents.on("context-menu", (_event, params) => {
    const template: MenuItemConstructorOptions[] = sections(contents, params).flatMap((section, index) => [
      ...(index > 0 ? [{ type: "separator" as const }] : []),
      ...section.map(({ id, label, enabled, run }) => ({ id, label, enabled: enabled ?? true, click: run })),
    ]);
    const menu = Menu.buildFromTemplate(template);
    lastMenu = menu;
    // Parked and unfocused, the OS's menu would come up under the user's pointer and hold it (L8).
    if (unfocused) {
      note("guest-menu", { webContents: contents.id, items: lastMenuItems() });
      return;
    }
    const window = BrowserWindow.fromWebContents(contents.hostWebContents ?? contents);
    if (window) menu.popup({ window });
  });
}

/** For the dev control: what the last menu offered, and choosing from it by the item's own click. */
export const lastMenuItems = () => lastMenu?.items.map(({ id, label, enabled, type }) => ({ id, label, enabled, type })) ?? [];
export function chooseFromLastMenu(id: string): boolean {
  const item = lastMenu?.getMenuItemById(id);
  if (!item) return false;
  item.click();
  return true;
}
