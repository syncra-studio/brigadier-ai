/**
 * A Browser tab's developer tools on Electron, over a faked shell: the toolbar's dev button and its menu, the DevTools
 * pane's place and dock, the device bar, and the picker's strips and toast. ?drive=1 drives them and writes what it saw
 * to #browser-dev-tools-result (BrowserDevToolsRender.test.ts reads it).
 */
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { BrowserTab } from "@/app/side-panel/BrowserTab";
import { Toaster, TooltipProvider } from "@/components/overlays";
import { mockIPC } from "@/ipc/nativeMocks";
import { useBrowserDev } from "@/state/browserDev";
import { useBrowsers } from "@/state/browsers";
import { CHAT_TAB, useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { fixtureClick, fixtureInput } from "./input";

const params = new URLSearchParams(location.search);
const ID = "browser:dev";
const SESSION = "session";

document.documentElement.dataset.shell = "electron";
const still = document.createElement("style");
still.textContent = "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
document.head.append(still);

useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  settings: { ...state.settings, browserEnabled: true },
  selection: { type: "conversation", id: SESSION },
}));
useBrowsers.setState({ pages: { [ID]: { url: "http://localhost:4173/", title: "Shop", loading: false, ready: true, blocked: null } as never } });
useSessionTabs.setState({
  sessions: { [SESSION]: { tabs: [{ kind: "browser", id: ID, opener: CHAT_TAB, url: "http://localhost:4173/", title: "" }], active: ID } },
});

const PICKED = {
  url: "http://localhost:4173/",
  selector: "#buy",
  tag: "button",
  attrs: { id: "buy" },
  text: "Buy widget",
  password: false,
  component: null,
  rect: { x: 0, y: 0, width: 100, height: 30 },
  styles: { display: "block" },
};

type Asked = { command: string; args: Record<string, unknown> };
const asked: Asked[] = [];
let answerPick: (picked: unknown) => void = () => {};
mockIPC(async (command, payload) => {
  const args = (payload ?? {}) as Record<string, unknown>;
  if (command.startsWith("browser_devtools") || command.startsWith("browser_device") || command.startsWith("browser_pick")) asked.push({ command, args });
  switch (command) {
    case "browser_pick":
      return new Promise((resolve) => { answerPick = resolve; });
    case "browser_pick_stop":
      answerPick(null);
      return null;
    case "browser_device_set":
    case "browser_device_clear":
      return false;
    case "browser_history":
    case "browser_downloads":
    case "browser_sites":
    case "browser_import_sources":
      return [];
    default:
      return null;
  }
});

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (check: () => boolean) => {
  for (let tries = 0; tries < 80 && !check(); tries++) await wait(25);
  return check();
};
const one = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const namedButton = (name: string) =>
  all<HTMLButtonElement>("button").find((each) => each.getAttribute("aria-label") === name || text(each.querySelector(".sr-only")) === name || text(each) === name);
const menuItems = () => all('[data-slot="browser-dev-menu"] [role^="menuitem"]').map(text);
const lastOf = (command: string) => asked.filter((each) => each.command === command).at(-1)?.args;
const countOf = (command: string) => asked.filter((each) => each.command === command).length;
const toasts = () => all('[data-slot="toast"]').map((element) => text(element).replace(/Dismiss$/, ""));

async function openMenu(): Promise<void> {
  await fixtureClick(namedButton("Developer tools")!);
  await until(() => menuItems().length > 0);
}
async function choose(item: string): Promise<void> {
  await openMenu();
  const found = all('[data-slot="browser-dev-menu"] [role^="menuitem"]').find((each) => text(each) === item);
  if (found) await fixtureClick(found);
  await until(() => menuItems().length === 0);
  // Focus back on the button shows its tooltip, over what is under it: the pointer goes elsewhere, as a user's would.
  await fixtureInput({ type: "pointer", x: 300, y: 500 });
  (document.activeElement as HTMLElement | null)?.blur();
  await until(() => one("[data-radix-popper-content-wrapper]") === null);
}
const size = (args: Record<string, unknown> | undefined) => {
  const bounds = args?.bounds as { width: number; height: number; y: number } | null | undefined;
  return bounds === undefined ? "none" : bounds === null ? null : { width: Math.round(bounds.width), height: Math.round(bounds.height), y: Math.round(bounds.y) };
};

async function drive(): Promise<string> {
  const seen: Record<string, unknown> = {};
  await until(() => namedButton("Developer tools") !== undefined);

  // The menu: its three items, and the dock only with DevTools on.
  await openMenu();
  seen.menu = menuItems();
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  one('[data-slot="browser-dev-menu"]')?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await until(() => menuItems().length === 0);

  // DevTools on: main hears where the pane goes, beside the page and above the floating composer.
  await choose("DevTools");
  await until(() => countOf("browser_devtools_open") > 0);
  seen.opened = countOf("browser_devtools_open");
  const composer = one('[data-slot="floating-composer"]')!.getBoundingClientRect();
  const opening = size(lastOf("browser_devtools_open"));
  seen.aboveComposer = opening !== null && opening !== "none" && opening.y + opening.height === Math.round(composer.top - 12);
  await openMenu();
  seen.menuOn = menuItems();
  // A menu over the pane: it steps aside while the menu is open (the native view would cover it).
  const dock = all('[data-slot="browser-dev-menu"] [role^="menuitem"]').find((each) => text(each) === "Dock DevTools below")!;
  window.dispatchEvent(new Event("resize"));
  seen.menuHides = await until(() => size(lastOf("browser_devtools_place")) === null);
  await fixtureClick(dock);
  await until(() => one('[data-slot="browser-devtools"]') !== null && menuItems().length === 0);
  await wait(50);
  const below = one('[data-slot="browser-devtools"]')!.getBoundingClientRect();
  seen.dock = useBrowserDev.getState().tabs[ID]?.dock;
  seen.belowIsWide = below.width > below.height;
  seen.separator = one('[role="separator"][aria-label="Resize DevTools"]')?.getAttribute("aria-orientation");
  await openMenu();
  seen.menuBelow = menuItems().at(-1);
  await fixtureClick(all('[data-slot="browser-dev-menu"] [role^="menuitem"]').find((each) => text(each) === "Dock DevTools beside")!);
  await until(() => menuItems().length === 0);
  seen.dockBack = useBrowserDev.getState().tabs[ID]?.dock;

  // A dialog anywhere hides the pane: it is a native view above the app.
  const dialog = document.createElement("div");
  dialog.setAttribute("role", "dialog");
  document.body.append(dialog);
  window.dispatchEvent(new Event("resize"));
  seen.dialogHides = await until(() => size(lastOf("browser_devtools_place")) === null);
  dialog.remove();
  window.dispatchEvent(new Event("resize"));
  seen.dialogGone = await until(() => size(lastOf("browser_devtools_place")) !== null);

  await choose("DevTools");
  await until(() => countOf("browser_devtools_close") > 0);
  seen.closed = countOf("browser_devtools_close");

  // Device sizes: the bar, a device, rotation, leaving.
  await choose("Device sizes");
  await until(() => one('[data-slot="browser-device-bar"]') !== null && countOf("browser_device_set") > 0);
  const device = () => lastOf("browser_device_set")?.device as { name: string; width: number; height: number; scale: number };
  const first = device();
  seen.firstDevice = { width: first.width, height: first.height };
  seen.fields = all<HTMLInputElement>('[data-slot="browser-device-bar"] input').map((each) => each.getAttribute("aria-label"));
  await fixtureClick(namedButton("Turn the device")!);
  await until(() => device().width === first.height);
  seen.turned = { width: device().width, height: device().height };
  const select = one<HTMLSelectElement>('select[aria-label="Device"]')!;
  seen.options = [...select.options].length;
  await fixtureClick(namedButton("Leave device sizes")!);
  await until(() => one('[data-slot="browser-device-bar"]') === null && countOf("browser_device_clear") > 0);
  seen.leftBar = one('[data-slot="browser-device-bar"]') === null;
  seen.cleared = countOf("browser_device_clear");

  // The picker: its strip, Stop, then a pick with no chat open (Added to chat needs a real composer: not here).
  await choose("Pick an element");
  await until(() => one('[data-slot="browser-picking"]') !== null);
  seen.picking = text(one('[data-slot="browser-picking"]'));
  await fixtureClick(one('[data-slot="browser-picking"] button')!);
  await until(() => one('[data-slot="browser-picking"]') === null);
  seen.stopped = { strip: one('[data-slot="browser-picking"]') !== null, asked: countOf("browser_pick_stop") };

  await choose("Pick an element");
  await until(() => one('[data-slot="browser-picking"]') !== null);
  answerPick(PICKED);
  await until(() => one('[data-slot="browser-picked"]') !== null);
  seen.picked = text(one('[data-slot="browser-picked"]'));
  await fixtureClick(namedButton("Add to chat")!);
  await until(() => toasts().length > 0);
  seen.noChat = toasts();
  seen.stillPicked = one('[data-slot="browser-picked"]') !== null;

  return JSON.stringify(seen);
}

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (!params.has("drive")) return;
    let stopped = false;
    void drive().then(
      (seen) => { if (!stopped) setResult(seen); },
      (error: unknown) => { if (!stopped) setResult(JSON.stringify({ error: String(error) })); },
    );
    return () => { stopped = true; };
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen">
        <main data-slot="pane-workspace" className="relative flex h-full min-w-0 flex-1 flex-col">
          <BrowserTab conversationId={ID} active />
          <div data-slot="floating-composer" style={{ position: "absolute", left: 0, right: 0, bottom: 20, height: 60 }} />
        </main>
      </div>
      <Toaster />
      {result && <pre id="browser-dev-tools-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
