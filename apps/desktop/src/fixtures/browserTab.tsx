/**
 * The Browser tab's toolbar over a faked shell: its measurements, the address idle and focused,
 * the site information button and popover, Reload while loading, the ⋯ menu, and where each of
 * the menu's Settings rows leads (Settings › Browser and Import Browser Data render beside it as
 * they do in the app's main area). Then the page's native actions: the zoom stepper, Print, the
 * toolbar's screenshot, the find bar (typed, stepped, kept per tab, closed) and the tab's keys.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { create } from "zustand";

import { BrowserTab } from "@/app/conversation/BrowserTab";
import { BrowserImportPage } from "@/app/settings/BrowserImportPage";
import { BrowserPage } from "@/app/settings/BrowserPage";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Request } from "@/ipc/generated";
import { useBrowserFind } from "@/state/browserFind";
import { useBrowserSettingsView } from "@/state/browserSettings";
import { useBrowsers } from "@/state/browsers";
import { useApp } from "@/state/store";
import { useToasts } from "@/state/toasts";

/*
 * ?drive=1 measures and drives the toolbar and writes what it saw to #browser-tab-result
 * (BrowserTabRender.test.ts reads it); ?full=1 turns Show full URL on. Without ?drive the tab
 * stays on a loaded page, for screenshots.
 */
const params = new URLSearchParams(location.search);
const full = params.has("full");
const ID = "c-browser";
const OTHER = "c-browser-2";
const PAGE = "https://www.example.com/docs/intro?q=1#top";
/** Which of two Browser tabs is in front. */
const useFront = create<{ id: string }>(() => ({ id: ID }));

if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}

useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  settings: { ...state.settings, browserEnabled: true, browserFullUrl: full },
  selection: { type: "conversation", id: ID },
}));

const setPage = (url: string, loading = false) =>
  useBrowsers.setState(({ pages }) => ({
    pages: { ...pages, [ID]: { ...pages[ID], url, title: "Example Domain", loading, ready: true, blocked: null } },
  }));
setPage(PAGE);

/**
 * What the tab asked the shell for that matters here: Back/Reload/Stop, outside opens, and the
 * page's native actions.
 */
const asked: unknown[] = [];
/** The faked page's matches of "foo": three, the one selected going round. */
let match = 0;
mockIPC(async (command, payload) => {
  const args = (payload ?? {}) as Record<string, unknown>;
  switch (command) {
    case "browser_zoom":
      asked.push({ zoom: args.zoom, id: args.id });
      return args.zoom;
    case "browser_find": {
      asked.push({ find: args.query, step: args.step, id: args.id });
      if (args.query !== "foo") return { found: false, current: 0, total: 0 };
      match = args.step === "type" ? Math.max(1, match) : args.step === "next" ? (match % 3) + 1 : match === 1 ? 3 : match - 1;
      return { found: true, current: match, total: 3 };
    }
    case "browser_print":
    case "browser_snapshot":
      asked.push({ [command.slice("browser_".length)]: args.id });
      return null;
    case "browser_focus":
      asked.push({ focus: args.id });
      return null;
    case "ipc_request": {
      const req = (args as { request: Request }).request;
      return req.method === "updateSettings" ? { method: req.method, settings: req.settings } : { method: req.method };
    }
    case "browser_go":
      asked.push({ go: args.action });
      return null;
    case "open_url":
      asked.push({ open: args.url });
      return null;
    case "browser_open":
    case "browser_navigate":
      asked.push({ navigate: args.url });
      return null;
    case "browser_site_count":
      return 0;
    case "browser_history":
    case "browser_downloads":
    case "browser_sites":
    case "browser_import_sources":
      return [];
    default:
      return null;
  }
});

const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [
  ...root.querySelectorAll<T>(selector),
];
const one = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const button = (name: string, root: ParentNode = document) =>
  all<HTMLButtonElement>("button", root).find(
    (element) =>
      element.getAttribute("aria-label") === name || text(element.querySelector(".sr-only")) === name || text(element) === name,
  );
const box = (element: Element | null | undefined) => {
  const rect = element?.getBoundingClientRect();
  return rect ? { width: Math.round(rect.width), height: Math.round(rect.height) } : null;
};
const field = () => one<HTMLInputElement>('input[aria-label="Address"]');
const address = () => {
  const input = field();
  return { value: input?.value, align: input ? getComputedStyle(input).textAlign : null };
};
const siteInfo = () => {
  const trigger = one('[data-slot="browser-site-info"]');
  return trigger
    ? { security: trigger.dataset.security, label: text(trigger.querySelector(".sr-only")), shown: text(trigger.querySelector("span[aria-hidden]")) }
    : null;
};
const popover = () => text(one('[data-slot="popover-content"]'));
const menu = () => one('[data-slot="browser-options"]');
const openMenu = () => {
  const trigger = button("Browser options");
  trigger?.focus();
  trigger?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
};
/** The ⋯ menu's rows as read top to bottom: "|" for a separator. */
const menuRows = () =>
  [...(menu()?.children ?? [])].map((row) => {
    const element = row as HTMLElement;
    if (element.getAttribute("role") === "separator") return "|";
    if (element.getAttribute("role") === "group") {
      return {
        label: element.getAttribute("aria-label"),
        shows: text(element.querySelector('[data-slot="browser-zoom"]')),
        disabled: all<HTMLButtonElement>("button", element).filter((inner) => inner.disabled).map((inner) => inner.getAttribute("aria-label")),
      };
    }
    return { label: text(element), disabled: element.hasAttribute("data-disabled") };
  });
const pickMenu = (name: string) =>
  all('[data-slot="browser-options"] [role="menuitem"]').find((item) => text(item) === name)?.click();
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (check: () => boolean) => {
  for (let tries = 0; tries < 60 && !check(); tries++) await wait(50);
};

const press = (code: string, options: KeyboardEventInit = {}, target: EventTarget = window) =>
  target.dispatchEvent(new KeyboardEvent("keydown", { code, bubbles: true, cancelable: true, metaKey: true, ...options }));
const zoomShown = () => text(one('[data-slot="browser-zoom"]'));
const findBar = () => one('[data-slot="browser-find"]');
const findField = () => one<HTMLInputElement>('input[aria-label="Find in page"]');
const findCount = () => text(one('[data-slot="browser-find-count"]'));
/** Types into the find field as React hears it. */
const typeFind = (value: string) => {
  const input = findField();
  if (!input) return;
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
};

/** Types into the address field as React hears it. */
const typeField = (value: string) => {
  const input = field();
  if (!input) return;
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
};

/** What the ⋯ menu's zoom stepper shows, the menu opened and closed again. */
const menuZoom = async () => {
  openMenu();
  await until(() => menu() !== null);
  const shows = zoomShown();
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await until(() => menu() === null);
  return shows;
};

/** The app's main area: the session's Browser tab in front, or the Settings page it opened. */
function Main() {
  const selection = useApp((s) => s.selection);
  const front = useFront((s) => s.id);
  if (selection.type === "settings") return selection.page === "browserImport" ? <BrowserImportPage /> : <BrowserPage />;
  return <BrowserTab key={front} conversationId={front} active />;
}

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (!params.has("drive")) return;
    let stopped = false;
    void (async () => {
      await until(() => field() !== null);
      await wait(200);
      const seen: Record<string, unknown> = {};
      const toolbar = one('[data-slot="browser-toolbar"]');
      const navigation = one('[data-slot="browser-navigation"]');
      seen.measure = {
        toolbar: box(toolbar),
        toolbarBorder: toolbar ? parseFloat(getComputedStyle(toolbar).borderBottomWidth) : null,
        navigation: box(navigation),
        navigationPadding: navigation ? parseFloat(getComputedStyle(navigation).paddingTop) : null,
        divider: box(navigation?.querySelector("span[aria-hidden]")),
        back: box(button("Back")),
        reload: box(button("Reload")),
        backIcon: box(button("Back")?.querySelector("svg")),
        address: box(one('[data-slot="browser-address"]')),
        addressRing: getComputedStyle(one('[data-slot="browser-address"]') ?? document.body).boxShadow,
        external: box(button("Open in external browser")),
        options: box(button("Browser options")),
        screenshot: box(button("Take a screenshot")),
        font: field() ? parseFloat(getComputedStyle(field()!).fontSize) : null,
        fade: field() ? getComputedStyle(field()!).maskImage : null,
      };

      seen.idle = address();
      field()?.focus();
      await wait(150);
      seen.focused = { ...address(), siteInfo: siteInfo(), external: button("Open in external browser") !== undefined };
      field()?.blur();
      await wait(100);
      seen.blurred = address();

      // https: a lock, faded in while the pointer is over the field.
      seen.secure = { ...siteInfo(), opacity: getComputedStyle(one('[data-slot="browser-site-info"]')!).opacity };
      one<HTMLButtonElement>('[data-slot="browser-site-info"]')?.click();
      await until(() => popover() !== "");
      seen.securePopover = popover();
      one<HTMLButtonElement>('[data-slot="browser-site-info"]')?.click();
      await until(() => popover() === "");

      setPage("http://example.com/");
      await wait(100);
      seen.insecure = siteInfo();
      seen.insecureAddress = address();
      one<HTMLButtonElement>('[data-slot="browser-site-info"]')?.click();
      await until(() => popover() !== "");
      seen.insecurePopover = popover();
      one<HTMLButtonElement>('[data-slot="browser-site-info"]')?.click();
      await until(() => popover() === "");

      setPage("http://localhost:5173/checkout");
      await wait(100);
      seen.local = siteInfo();

      // Loading: the spinner stands in for Reload and stops the load.
      setPage(PAGE, true);
      await wait(100);
      const stop = button("Stop loading");
      seen.loading = { busy: stop?.getAttribute("aria-busy"), spinner: stop?.querySelector("svg.animate-spin") !== null };
      stop?.click();
      await wait(50);
      setPage(PAGE);
      await wait(100);
      button("Open in external browser")?.click();
      await wait(50);
      seen.asked = asked.slice();

      // Enter in the address: the form's default button (its first submit button) is clicked, as
      // a browser does, then the form submits. Only the tab goes there; nothing opens outside.
      const entered = asked.length;
      field()?.focus();
      typeField("example.org/next");
      await wait(50);
      const form = field()?.form;
      const submitters = [...(form?.elements ?? [])].filter((element) => (element as HTMLButtonElement).type === "submit");
      (submitters[0] as HTMLButtonElement | undefined)?.click();
      form?.requestSubmit();
      await wait(100);
      seen.addressEnter = { submitters: submitters.length, asked: asked.slice(entered) };
      setPage(PAGE);
      await wait(100);

      openMenu();
      await until(() => menu() !== null);
      seen.menu = menuRows();
      seen.menuWidth = box(menu())?.width;
      seen.menuItemHeight = box(menu()?.querySelector('[role="menuitem"]'))?.height;
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await until(() => menu() === null);

      // Each Settings row of the menu, from the tab, and back to the tab.
      const destinations: Record<string, unknown> = {};
      for (const row of ["Downloads", "History", "Clear browsing data", "Import Browser Data…", "Browser settings"]) {
        openMenu();
        await until(() => menu() !== null);
        pickMenu(row);
        await until(() => useApp.getState().selection.type === "settings");
        await wait(200);
        destinations[row] = {
          selection: useApp.getState().selection,
          title: text(one("main h1")),
          dialog: text(one('[role="dialog"] h2')),
        };
        useBrowserSettingsView.setState({ opened: null, clearing: null });
        useApp.setState({ selection: { type: "conversation", id: ID } });
        await until(() => field() !== null && one('[role="dialog"]') === null);
        await wait(100);
      }
      // The site information popover's Site settings.
      one<HTMLButtonElement>('[data-slot="browser-site-info"]')?.click();
      await until(() => popover() !== "");
      button("Site settings", one('[data-slot="popover-content"]') ?? document)?.click();
      await until(() => useApp.getState().selection.type === "settings");
      await wait(200);
      destinations["Site settings"] = { selection: useApp.getState().selection, title: text(one("main h1")), dialog: text(one('[role="dialog"] h2')) };
      useBrowserSettingsView.setState({ opened: null, clearing: null });
      useApp.setState({ selection: { type: "conversation", id: ID } });
      await until(() => field() !== null);
      seen.destinations = destinations;

      // The page's native actions from the ⋯ menu: Zoom steps 10% and resets, Print and the
      // screenshot wait for the page to show again, the screenshot says where it went.
      const native = asked.length;
      openMenu();
      await until(() => menu() !== null);
      button("Zoom in", menu() ?? document)?.click();
      await until(() => zoomShown() === "110%");
      button("Zoom in", menu() ?? document)?.click();
      await until(() => zoomShown() === "120%");
      button("Zoom out", menu() ?? document)?.click();
      await until(() => zoomShown() === "110%");
      seen.zoomStepped = { shows: zoomShown(), group: menuRows().find((row) => typeof row === "object" && row.label === "Zoom") };
      button("Reset zoom", menu() ?? document)?.click();
      await until(() => zoomShown() === "100%");
      seen.zoomReset = zoomShown();
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await until(() => menu() === null);
      button("Take a screenshot")?.click();
      await until(() => useToasts.getState().toasts.length > 0);
      seen.screenshotToast = useToasts.getState().toasts.map((toast) => toast.text);
      openMenu();
      await until(() => menu() !== null);
      pickMenu("Print");
      await until(() => asked.some((entry) => (entry as { print?: string }).print === ID));
      seen.menuAsked = asked.slice(native);

      // Keys: ⌘= / ⌘- / ⌘0 zoom; with the page holding the keyboard they arrive at the tab's
      // pane from the Browser menu, as ⌘R, ⇧⌘R, ⌘← and ⌘→ do. A field keeps its ⌘←.
      const keys = asked.length;
      const pane = one('[data-pane="browser"]') ?? window;
      press("Equal");
      await until(() => useBrowsers.getState().pages[ID]?.zoom === 1.1);
      press("Minus", {}, pane);
      await until(() => useBrowsers.getState().pages[ID]?.zoom === 1);
      press("Equal", { shiftKey: true });
      await until(() => useBrowsers.getState().pages[ID]?.zoom === 1.1);
      press("Digit0", {}, pane);
      await until(() => useBrowsers.getState().pages[ID]?.zoom === 1);
      press("KeyR", {}, pane);
      press("KeyR", { shiftKey: true }, pane);
      press("ArrowLeft", {}, pane);
      press("ArrowRight", {}, pane);
      field()?.focus();
      press("ArrowLeft", {}, field() ?? window);
      field()?.blur();
      // A native page holds the keyboard: ⌘← and ⌘→ leave a field of the page its caret, and ⌘L
      // takes the keyboard back to the app before the address field gets it.
      Object.defineProperty(document, "hasFocus", { configurable: true, value: () => false });
      press("ArrowLeft", {}, pane);
      press("ArrowRight", {}, pane);
      press("KeyL", {}, pane);
      await until(() => document.activeElement === field());
      seen.addressFromPage = document.activeElement === field();
      Reflect.deleteProperty(document, "hasFocus");
      field()?.blur();
      await wait(100);
      seen.keysAsked = asked.slice(keys);

      // The find bar: ⌘F opens it with the keyboard in its field; typing finds, Enter and ⌘G go
      // on, ⇧Enter back; no match says so.
      const finds = asked.length;
      press("KeyF", {}, pane);
      await until(() => findBar() !== null);
      await wait(100);
      seen.findOpen = { focused: document.activeElement === findField(), placeholder: findField()?.placeholder, count: findCount() };
      const bar = findBar()?.getBoundingClientRect();
      const findRow = one('[data-slot="browser-find-row"]')?.getBoundingClientRect();
      const toolbarBox = one('[data-slot="browser-toolbar"]')?.getBoundingClientRect();
      seen.findMeasure = {
        width: bar ? Math.round(bar.width) : null,
        height: bar ? Math.round(bar.height) : null,
        radius: parseFloat(getComputedStyle(findBar() ?? document.body).borderTopLeftRadius),
        endGap: bar && findRow ? Math.round(findRow.right - bar.right) : null,
        underToolbar: findRow && toolbarBox ? Math.round(findRow.top - toolbarBox.bottom) : null,
        pageBelow: findRow ? Math.round((one('[data-slot="browser-page"]')?.getBoundingClientRect().top ?? 0) - findRow.bottom) : null,
        font: findField() ? parseFloat(getComputedStyle(findField()!).fontSize) : null,
      };
      typeFind("foo");
      await until(() => findCount() === "1 of 3");
      seen.typed = findCount();
      findField()?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
      await until(() => findCount() === "2 of 3");
      seen.enter = findCount();
      press("KeyG", {}, pane);
      await until(() => findCount() === "3 of 3");
      seen.commandG = findCount();
      findField()?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", shiftKey: true, bubbles: true, cancelable: true }));
      await until(() => findCount() === "2 of 3");
      seen.shiftEnter = findCount();
      press("KeyG", { shiftKey: true }, pane);
      await until(() => findCount() === "1 of 3");
      seen.commandShiftG = findCount();
      typeFind("fooz");
      await until(() => findCount() === "No results");
      seen.noResults = { count: findCount(), previous: button("Previous match")?.disabled, next: button("Next match")?.disabled };
      typeFind("foo");
      await until(() => findCount() === "1 of 3");
      press("Equal", {}, pane);
      await until(() => useBrowsers.getState().pages[ID]?.zoom === 1.1);

      // Kept per tab: another tab has none, and coming back finds it as it was.
      useBrowsers.setState(({ pages }) => ({ pages: { ...pages, [OTHER]: { url: "https://other.test/", title: "Other", loading: false, ready: true, blocked: null } } }));
      useFront.setState({ id: OTHER });
      await until(() => one('[data-pane="browser"]') !== null && findBar() === null);
      await wait(100);
      seen.otherTab = { bar: findBar() !== null, zoom: await menuZoom() };
      useFront.setState({ id: ID });
      await until(() => findBar() !== null);
      await wait(100);
      seen.backTab = { query: findField()?.value, count: findCount(), zoom: await menuZoom() };

      // Esc closes it, lets go of the match and gives the page the keyboard back.
      findField()?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
      await until(() => findBar() === null);
      await wait(50);
      seen.closed = { bar: findBar() !== null, kept: useBrowserFind.getState().tabs[ID]?.query };
      seen.findAsked = asked.slice(finds);

      // The New tab page: no site, so no site information or outside button.
      useBrowsers.setState({ pages: {} });
      await wait(100);
      field()?.blur();
      await wait(100);
      seen.newTab = { ...address(), placeholder: field()?.placeholder, siteInfo: siteInfo(), external: button("Open in external browser") !== undefined };
      seen.newTabBack = { disabled: button("Back")?.disabled, opacity: getComputedStyle(button("Back")!).opacity };
      // Its ⋯ menu: nothing to find, print, zoom or picture; ⌘F does nothing.
      openMenu();
      await until(() => menu() !== null);
      seen.newTabMenu = menuRows().filter((row) => row !== "|").slice(0, 3);
      seen.newTabScreenshot = button("Take a screenshot")?.disabled;
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await until(() => menu() === null);
      press("KeyF");
      await wait(100);
      seen.newTabFind = findBar() !== null;

      if (!stopped) setResult(JSON.stringify(seen));
    })();
    return () => {
      stopped = true;
    };
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen">
        <main className="flex h-full min-w-0 flex-1 flex-col">
          <Main />
        </main>
      </div>
      {result && <pre id="browser-tab-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
