/** Settings › Browser over a faked shell and daemon: the rows, Import…, Clear, the sub-views, off. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { BrowserPage } from "@/app/settings/BrowserPage";
import { TooltipProvider } from "@/components/ui/tooltip";
import type {
  BrowserClear,
  BrowserDownload,
  BrowserSite,
  BrowserVisit,
  ImportProfile,
  ImportResult,
  Request,
} from "@/ipc/generated";
import { useApp } from "@/state/store";

/*
 * ?drive=1 reads the page, imports from a profile (after trying one whose browser is open and
 * one macOS keeps locked), clears browsing data, filters the history, opens the downloads and
 * the site settings, turns the browser off, and writes what it saw to #browser-result
 * (BrowserPageRender.test.ts reads it). ?view=import|clear|history|downloads|sites|off opens
 * that state and stays, for screenshots.
 */
const params = new URLSearchParams(location.search);
document.documentElement.dataset.density = params.get("density") === "compact" ? "compact" : "normal";
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}

useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  settings: { ...state.settings, browserEnabled: params.get("view") !== "off" },
}));

function profile(id: string, browser: string, fields: Partial<ImportProfile> = {}): ImportProfile {
  return {
    id,
    browser,
    name: "Default",
    family: "chromium",
    hasCookies: true,
    hasHistory: true,
    readable: true,
    isDefault: true,
    running: false,
    ...fields,
  };
}

const PROFILES: ImportProfile[] = [
  profile("dia:Default", "Dia", { running: true }),
  profile("chrome:Default", "Google Chrome", { name: "Person 1" }),
  profile("firefox:default-release", "Firefox", { family: "gecko", name: "default-release" }),
  profile("safari", "Safari", { family: "safari", readable: false }),
];

const IMPORTED: ImportResult = {
  cookies: { imported: 1204, rejected: 3, skipped: 41, error: null },
  history: { imported: 5321, error: null },
};

const HOUR = 3_600_000;
const now = Date.now();
const VISITS: BrowserVisit[] = [
  { id: 3, url: "http://localhost:5173/checkout", title: "Checkout preview", atMs: now - HOUR, agent: true },
  { id: 2, url: "https://www.rust-lang.org/learn", title: "Learn Rust", atMs: now - 2 * HOUR, agent: false },
  { id: 1, url: "https://developer.apple.com/documentation/webkit", title: "", atMs: now - 30 * HOUR, agent: false },
];

const DOWNLOADS: BrowserDownload[] = [
  { id: 2, url: "https://example.com/report.pdf", path: "/Users/me/Downloads/report.pdf", atMs: now - HOUR, state: "done" },
  { id: 1, url: "https://example.com/big.zip", path: "/Users/me/Downloads/big.zip", atMs: now - 2 * HOUR, state: "failed" },
];

const SITES: BrowserSite[] = [
  { origin: "https://meet.example.com", camera: "allow", microphone: "allow", askedMs: now - HOUR },
  { origin: "http://localhost:3000", camera: "ask", microphone: "block", askedMs: now - 2 * HOUR },
];

/** What the page asked the shell for, in order: the commands and the arguments that matter. */
const asked: unknown[] = [];
mockIPC(async (command, payload) => {
  const args = (payload ?? {}) as Record<string, unknown>;
  switch (command) {
    case "ipc_request": {
      const req = (args as { request: Request }).request;
      if (req.method === "updateSettings") {
        const { browserEnabled, webLinksIn, localLinksIn, browserFullUrl } = req.settings;
        asked.push({ updateSettings: { browserEnabled, webLinksIn, localLinksIn, browserFullUrl } });
        return { method: req.method, settings: req.settings };
      }
      return { method: req.method };
    }
    case "browser_import_profiles":
      return PROFILES;
    case "browser_import":
      asked.push({ import: { id: args.id, cookies: args.cookies, history: args.history } });
      return { cookies: args.cookies ? IMPORTED.cookies : null, history: args.history ? IMPORTED.history : null };
    case "browser_site_count":
      return 87;
    case "browser_clear":
      asked.push({ clear: args.clear as BrowserClear });
      return null;
    case "browser_history":
      asked.push({ history: args.filter });
      return VISITS.filter((visit) => args.filter === "all" || visit.agent === (args.filter === "agent"));
    case "browser_downloads":
      return DOWNLOADS;
    case "browser_sites":
      return SITES;
    default:
      return null;
  }
});

const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [
  ...root.querySelectorAll<T>(selector),
];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const button = (name: string, root: ParentNode = document) =>
  all<HTMLButtonElement>("button", root).find(
    (element) => text(element) === name || element.getAttribute("aria-label") === name,
  );
const openMenu = (label: string): "menu" => {
  const trigger = button(label);
  trigger?.focus();
  trigger?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  return "menu";
};
const menuItems = () => all('[role="menu"][data-state="open"] [role^="menuitem"]').map(text);
const pickMenu = (name: string) =>
  (
    all('[role="menu"][data-state="open"] [role^="menuitem"]').find((item) => text(item).startsWith(name)) as
      | HTMLElement
      | undefined
  )?.click();
const dialog = () => document.querySelector<HTMLElement>('[role="dialog"]');
const dialogTexts = () => all("p", dialog() ?? document.createElement("div")).map(text);
const boxes = () =>
  all<HTMLInputElement>('input[type="checkbox"]', dialog() ?? document.createElement("div")).map((box) => ({
    label: text(box.closest("label")?.querySelector(".text-label")),
    checked: box.checked,
    disabled: box.disabled,
  }));
const page = () => ({
  title: text(document.querySelector("main h1")),
  sections: all("main section h2").map(text),
  rows: all('[data-slot="settings-row"]').map((element) => element.dataset.setting ?? text(element.querySelector(".text-label, .text-sm"))),
});
const settings = () => useApp.getState().settings;

/** The step that opens a `?view=` state. */
const VIEWS: Record<string, () => void> = {
  import: () => button("Import…")?.click(),
  clear: () => button("Clear browsing data")?.click(),
  history: () => all<HTMLButtonElement>('[data-setting="Browsing history"] button')[0]?.click(),
  downloads: () => all<HTMLButtonElement>('[data-setting="Download history"] button')[0]?.click(),
  sites: () => all<HTMLButtonElement>('[data-setting="Site settings"] button')[0]?.click(),
  off: () => {},
};

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    const view = params.get("view");
    if (view && !params.has("drive")) {
      const timer = setTimeout(() => VIEWS[view]?.(), 200);
      return () => clearTimeout(timer);
    }
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    const steps: (() => void | "menu" | Promise<void>)[] = [
      () => {
        seen.on = page();
        seen.importButton = button("Import…") !== undefined;
        seen.links = [text(button("Open web links in")), text(button("Open local dev URLs in"))];
        return openMenu("Open web links in");
      },
      () => {
        seen.linkChoices = menuItems();
        pickMenu("Brigadier's browser");
      },
      () => {
        seen.webLinksIn = settings().webLinksIn;
        button("Import…")?.click();
      },
      // The profiles load: an open browser isn't picked first.
      () => {
        seen.importOpen = { title: text(dialog()?.querySelector("h2")), from: text(button("Import from")), boxes: boxes() };
        seen.importNotes = dialogTexts();
        return openMenu("Import from");
      },
      () => {
        seen.profiles = menuItems();
        pickMenu("Dia");
      },
      () => {
        seen.running = { notes: dialogTexts(), importable: !button("Import", dialog() ?? document)?.disabled };
        return openMenu("Import from");
      },
      () => pickMenu("Safari"),
      () => {
        seen.locked = {
          notes: dialogTexts(),
          settings: button("Open System Settings") !== undefined,
          importable: !button("Import", dialog() ?? document)?.disabled,
        };
        return openMenu("Import from");
      },
      () => pickMenu("Google Chrome"),
      () => {
        seen.chromium = { notes: dialogTexts(), importable: !button("Import", dialog() ?? document)?.disabled };
        return openMenu("Import from");
      },
      () => pickMenu("Firefox"),
      () => {
        seen.gecko = dialogTexts();
        // History only.
        all<HTMLInputElement>('input[type="checkbox"]', dialog() ?? document)[0]?.click();
      },
      () => button("Import", dialog() ?? document)?.click(),
      () => {
        seen.imported = dialogTexts();
        button("Done")?.click();
      },
      () => button("Clear browsing data")?.click(),
      () => {
        seen.clearKinds = boxes();
        seen.clearNotes = all(".text-xs", dialog() ?? document).map(text);
        // All time, and the cache kept.
        button("All time")?.click();
        all<HTMLInputElement>('input[type="checkbox"]', dialog() ?? document)[2]?.click();
      },
      () => button("Delete data")?.click(),
      () => {
        seen.clearClosed = dialog() === null;
        VIEWS.history?.();
      },
      () => {
        seen.history = { ...page(), visits: all("main li button[title]").map((element) => element.title) };
        return openMenu("Filter browsing history");
      },
      () => {
        seen.filters = menuItems();
        pickMenu("Agent");
      },
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 200));
        seen.agentVisits = all("main li button[title]").map((element) => element.title);
        // Browser picked again in the sidebar: the page itself, not the history.
        useApp.setState({ selection: { type: "settings", page: "browser" } });
      },
      () => {
        seen.repicked = text(document.querySelector("main h1"));
      },
      () => VIEWS.downloads?.(),
      () => {
        seen.downloads = { title: text(document.querySelector("main h1")), files: all("main li [title]").map((element) => element.title) };
        button("Browser")?.click();
      },
      () => VIEWS.sites?.(),
      () => {
        seen.sites = { title: text(document.querySelector("main h1")), choices: [text(button("Camera for http://localhost:3000")), text(button("Microphone for http://localhost:3000"))] };
        button("Browser")?.click();
      },
      () => button("Browser", document.querySelector('[data-slot="settings-row"]') ?? document)?.click(),
      () => {
        seen.off = { ...page(), enabled: settings().browserEnabled, note: all("main p").map(text) };
        seen.asked = asked;
        setResult(JSON.stringify(seen));
      },
    ];
    let stopped = false;
    const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
    const menuOpen = () => document.querySelector('[role="menu"][data-state="open"]') !== null;
    void (async () => {
      await wait(400);
      for (const step of steps) {
        if (stopped) return;
        const done = step();
        if (done instanceof Promise) await done;
        await wait(150);
        if (done === "menu") for (let tries = 0; tries < 40 && !menuOpen(); tries++) await wait(50);
      }
    })();
    return () => {
      stopped = true;
    };
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen">
        <main className="flex h-full min-w-0 flex-1">
          <div className="min-w-0 flex-1">
            <BrowserPage />
          </div>
        </main>
      </div>
      {result && <pre id="browser-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
