/**
 * The Browser tab's toolbar over a faked shell: its measurements, the address idle and focused,
 * the site information button and popover, Reload while loading, the ⋯ menu, and where each of
 * the menu's Settings rows leads (Settings › Browser and Import Browser Data render beside it as
 * they do in the app's main area).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { BrowserTab } from "@/app/conversation/BrowserTab";
import { BrowserImportPage } from "@/app/settings/BrowserImportPage";
import { BrowserPage } from "@/app/settings/BrowserPage";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Request } from "@/ipc/generated";
import { useBrowserSettingsView } from "@/state/browserSettings";
import { useBrowsers } from "@/state/browsers";
import { useApp } from "@/state/store";

/*
 * ?drive=1 measures and drives the toolbar and writes what it saw to #browser-tab-result
 * (BrowserTabRender.test.ts reads it); ?full=1 turns Show full URL on. Without ?drive the tab
 * stays on a loaded page, for screenshots.
 */
const params = new URLSearchParams(location.search);
const full = params.has("full");
const ID = "c-browser";
const PAGE = "https://www.example.com/docs/intro?q=1#top";

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
  useBrowsers.setState({ pages: { [ID]: { url, title: "Example Domain", loading, ready: true, blocked: null } } });
setPage(PAGE);

/** What the tab asked the shell for that matters here: Back/Reload/Stop, and outside opens. */
const asked: unknown[] = [];
mockIPC(async (command, payload) => {
  const args = (payload ?? {}) as Record<string, unknown>;
  switch (command) {
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
    return {
      label: element.getAttribute("role") === "group" ? element.getAttribute("aria-label") : text(element),
      disabled: element.hasAttribute("data-disabled") || all("button", element).some((inner) => (inner as HTMLButtonElement).disabled),
      native: element.dataset.native ?? null,
    };
  });
const pickMenu = (name: string) =>
  all('[data-slot="browser-options"] [role="menuitem"]').find((item) => text(item) === name)?.click();
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (check: () => boolean) => {
  for (let tries = 0; tries < 60 && !check(); tries++) await wait(50);
};

/** The app's main area: the session's Browser tab, or the Settings page it opened. */
function Main() {
  const selection = useApp((s) => s.selection);
  if (selection.type === "settings") return selection.page === "browserImport" ? <BrowserImportPage /> : <BrowserPage />;
  return <BrowserTab conversationId={ID} active />;
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

      // The New tab page: no site, so no site information or outside button.
      useBrowsers.setState({ pages: {} });
      await wait(100);
      field()?.blur();
      await wait(100);
      seen.newTab = { ...address(), placeholder: field()?.placeholder, siteInfo: siteInfo(), external: button("Open in external browser") !== undefined };
      seen.newTabBack = { disabled: button("Back")?.disabled, opacity: getComputedStyle(button("Back")!).opacity };

      seen.asked = asked;
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
