/**
 * Settings › Import Browser Data over a faked shell: one row per browser state, the profile
 * picker, importing, Full Disk Access, a blocked import, the results per profile with a failure
 * and its Try again, the last import's record, the browser off, and no browsers or an error.
 */
import { mockIPC } from "@/ipc/nativeMocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { BrowserImportPage } from "@/app/settings/BrowserImportPage";
import { Toaster, TooltipProvider } from "@/components/overlays";
import type { ImportProgress, ImportRecord, Request, Settings } from "@/ipc/generated";
import type { ImportedOutcome, ImportedRecord, ListedSource } from "@/state/browserImport";
import { allProfiles } from "@/state/browserProfiles";
import { useApp } from "@/state/store";

/*
 * ?drive=1 reads every row, picks profiles for Dia and imports them (trying to leave the
 * importing dialog halfway), reads the results and tries a failed part again, grants Safari Full
 * Disk Access (still off the first time), meets Edge's Keychain refusal, quits Helium when asked,
 * fails to read Arc, rechecks Chrome, then
 * lists no browsers, an error and the browser off, and writes what it saw to
 * #browser-import-result (BrowserImportPageRender.test.ts reads it).
 * ?view=choose|importing|results|fda|fda-still-off|blocked-keychain|blocked-running|off|empty|error|loading
 * opens that state and stays, for screenshots.
 */
const params = new URLSearchParams(location.search);
const view = params.get("view");
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}

useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  settings: { ...state.settings, browserEnabled: view !== "off" },
  selection: { type: "settings", page: "browserImport" },
}));

/** A stand-in app icon: a rounded tile, darker or lighter per browser. */
const icon = (shade: number) =>
  `data:image/svg+xml;base64,${btoa(
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect x="4" y="4" width="56" height="56" rx="14" fill="currentColor" fill-opacity="${shade / 255}"/><circle cx="32" cy="32" r="13" fill="white" fill-opacity="0.85"/></svg>`,
  )}`;

const DAY = 86_400_000;
const ARC_RECORD: ImportRecord = {
  atMs: Date.now() - 3 * DAY,
  profiles: ["Arc"],
  imported: 887,
  skipped: 26,
  skippedDomains: ["accounts.google.com", "github.com", "x.com", "news.example"],
  skippedDomainCount: 4,
  history: 0,
};

const source = (key: string, name: string, fields: Partial<ListedSource> = {}): ListedSource => ({
  key,
  name,
  family: "chromium",
  icon: null,
  state: "ready",
  profiles: [],
  record: null,
  emptyProfiles: [],
  signIns: true,
  passwords: true,
  imports: {},
  firstImport: true,
  ...fields,
});
const one = (key: string, cookies: number) => [{ id: `${key}/Default`, name: "Default", cookies, isDefault: true }];

let sources: ListedSource[] = [
  source("chrome", "Google Chrome", { state: "running", icon: icon(215) }),
  source("chromium", "Chromium", { profiles: one("chromium", 12), signIns: false, passwords: false }),
  source("helium", "Helium", { profiles: one("helium", 40), icon: icon(255), passwords: false }),
  source("edge", "Microsoft Edge", { state: "needsKeychainAccess", profiles: one("edge", 310) }),
  source("brave", "Brave", { state: "noKeychainKey", icon: icon(15) }),
  source("vivaldi", "Vivaldi", { state: "unavailable" }),
  source("arc", "Arc", { profiles: one("arc", 912), record: ARC_RECORD }),
  source("dia", "Dia", {
    icon: icon(42),
    emptyProfiles: ["Guest"],
    profiles: [
      { id: "dia/Default", name: "Personal", cookies: 2841, isDefault: true },
      { id: "dia/Profile 1", name: "Work", cookies: 1204, isDefault: false },
    ],
  }),
  source("firefox", "Firefox", { family: "gecko", profiles: one("firefox", 0) }),
  source("safari", "Safari", { family: "safari", state: "needsFullDiskAccess", icon: icon(200) }),
];

/** What listing does next: the browsers, none, or an error. */
let mode: "sources" | "empty" | "error" = view === "empty" ? "empty" : view === "error" ? "error" : "sources";
/** Ends a running Dia import; set while one waits. */
let release: (() => void) | null = null;
const patch = (key: string, fields: Partial<ListedSource>) => {
  sources = sources.map((item) => (item.key === key ? { ...item, ...fields } : item));
};

const failed = (reason: Extract<ImportedOutcome, { type: "failed" }>["reason"], message: string): ImportedOutcome => ({
  type: "failed",
  reason,
  message,
});

/** What the page asked the shell for, in order. */
const asked: unknown[] = [];
let listed = 0;
mockIPC(async (command, payload) => {
  const args = (payload ?? {}) as Record<string, unknown>;
  switch (command) {
    case "browser_import_sources":
      listed += 1;
      if (view === "loading") return new Promise(() => {});
      if (mode === "error") throw new Error("Could not check the installed browsers.");
      return mode === "empty" ? [] : sources;
    case "ipc_request": {
      const req = (args as { request: Request }).request;
      return req.method === "updateSettings" ? { method: req.method, settings: req.settings as Settings } : { method: req.method };
    }
    case "browser_import_open_access":
      asked.push("openAccess");
      return null;
    case "browser_import_read": {
      const key = args.key as string;
      if (key === "edge") {
        patch("edge", { state: "needsKeychainAccess" });
        return failed("needsKeychainAccess", "macOS didn't let Brigadier use Microsoft Edge's cookie key.");
      }
      if (key === "arc") return failed("readFailed", "Arc's cookies couldn't be read (database disk image is malformed).");
      if (key === "helium" && sources.find((item) => item.key === "helium")?.state === "ready" && !asked.includes("heliumQuit")) {
        asked.push("heliumQuit");
        patch("helium", { state: "running" });
        return failed("running", "Quit Helium first, then import again.");
      }
      return { type: "ready", token: `${key}|${args.id as string}` };
    }
    case "browser_import_write": {
      const [key = "", id = ""] = (args.token as string).split("|");
      const ids = [id];
      const options = args.what as { signIns: boolean; history: boolean; bookmarks: boolean; passwords: boolean };
      const { signIns, history, bookmarks, passwords } = options;
      asked.push({ import: { key, ids, history, signIns, bookmarks, passwords, replace: args.replace } });
      const progress = args.progress as { onmessage: (message: ImportProgress) => void };
      const done = (record: Partial<ImportedRecord>, more: Partial<Extract<ImportedOutcome, { type: "done" }>> = {}): ImportedOutcome => {
        const full: ImportedRecord = { ...ARC_RECORD, atMs: Date.now(), skipped: 0, skippedDomains: [], skippedDomainCount: 0, history: 0, ...record };
        // A retried part leaves the browser's record as the import left it.
        // The row sums up each source profile's last import, as main does.
        if (options.signIns) {
          const before = sources.find((each) => each.key === key)?.record;
          patch(key, { record: before ? { ...full, profiles: [...before.profiles, ...full.profiles], imported: before.imported + full.imported, skipped: before.skipped + full.skipped, skippedDomains: [...before.skippedDomains, ...full.skippedDomains], skippedDomainCount: before.skippedDomainCount + full.skippedDomainCount } : full });
        }
        return { type: "done", record: full, historyError: null, ...more };
      };
      switch (key) {
        case "dia": {
          if (ids[0] === "dia/Default") {
            progress.onmessage({ type: "reading" });
            progress.onmessage({ type: "cookies", done: 1200, total: 4045 });
            await new Promise<void>((resolve) => {
              release = resolve;
            });
            return done({
              profiles: ["Personal"],
              imported: 2800,
              skipped: 51,
              skippedDomains: ["accounts.google.com", "mail.google.com"],
              skippedDomainCount: 2,
              appBoundDomains: ["accounts.google.com"],
              history: 900,
              bookmarks: 40,
              passwords: 21,
              googleLeftOut: 2,
            });
          }
          // Work's bookmarks can't be read once, and come on Try again.
          if (options.signIns) {
            return done({ profiles: ["Work"], imported: 1194, history: 300, bookmarks: 0, passwords: 0, passwordsKept: true }, { bookmarksError: "Dia's bookmarks couldn't be read" });
          }
          return done({ profiles: ["Work"], imported: 0, bookmarks: 12 });
        }
        case "safari":
          return done({ profiles: ["Default"], imported: 12, history: 340, bookmarks: 5 }, { passwordsError: "Safari keeps its passwords in iCloud Keychain, which other apps can't read." });
        case "firefox":
          return done({ profiles: ["Default"], imported: 5, history: 8, bookmarks: 2 }, { passwordsError: "Firefox's passwords are locked with a Primary Password." });
        case "helium":
          return done({ profiles: ["Default"], imported: 0 });
        default:
          return failed("gone", "That browser or profile is no longer here.");
      }
    }
    default:
      return null;
  }
});

const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [
  ...root.querySelectorAll<T>(selector),
];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const button = (name: string, root: ParentNode = document) =>
  all<HTMLButtonElement>("button", root).find((element) => text(element) === name);
const row = (key: string) => document.querySelector<HTMLElement>(`[data-slot="import-row"][data-browser="${key}"]`);
const rowButton = (key: string) => row(key)?.querySelector<HTMLButtonElement>("button");
const dialog = () => document.querySelector<HTMLElement>('[data-slot="import-dialog"]');
const readDialog = () => {
  const open = dialog();
  if (!open) return null;
  return {
    step: open.dataset.step,
    title: text(open.querySelector('[data-slot="dialog-title"]')),
    description: text(open.querySelector('[data-slot="dialog-description"]')),
    notes: all("p", open).map(text),
    // The dialog's own buttons (not its checkboxes), then whether it has a close button.
    buttons: all<HTMLButtonElement>('button:not([role="checkbox"]):not([data-slot="dialog-close"])', open).map((element) =>
      element.disabled ? `${text(element)} (off)` : text(element),
    ),
    closable: open.querySelector('[data-slot="dialog-close"]') !== null,
  };
};
const rows = () =>
  all('[data-slot="import-row"]').map((element) => ({
    name: text(element.querySelector(".font-medium")),
    badge: text(element.querySelector('[data-slot="import-badge"]')) || null,
    status: text(element.querySelector('[data-slot="import-status"]')),
    details: all('[data-slot="import-detail"]', element).map(text),
    button: element.querySelector("button") ? text(element.querySelector("button")) : null,
    enabled: element.querySelector<HTMLButtonElement>("button")?.disabled === false,
    icon: element.querySelector("img") ? "app" : "globe",
  }));
const profiles = () =>
  all('[data-slot="import-profile"]').map((element) => ({
    name: text(element.querySelector(".text-label")),
    line: text(element.querySelector(".text-xs")),
    into: text(element.querySelector("button:not([role=checkbox])")),
    checked: element.querySelector('[role="checkbox"]')?.getAttribute("data-state") === "checked",
  }));
const results = () =>
  all('[data-slot="import-result"]').map((element) => ({
    name: text(element.querySelector(".font-medium")),
    badge: text(element.querySelector('[data-slot="badge"]')),
    parts: all('[data-slot="import-part"]', element).map((part) => [part.dataset.mark, text(part)]),
    sites: all('[data-slot="import-site"]', element).map(text),
    google: text(element.querySelector('[data-slot="import-google"]')),
  }));
const bring = () => all('[role="checkbox"]', dialog() ?? document).slice(-4).map((box) => box.getAttribute("data-state"));
const tickProfile = (name: string) =>
  all('[data-slot="import-profile"]')
    .find((element) => text(element.querySelector(".text-label")) === name)
    ?.querySelector<HTMLButtonElement>('[role="checkbox"]')
    ?.click();
const escape = () =>
  document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));

/** The step that opens a `?view=` state. */
const VIEWS: Record<string, () => void> = {
  choose: () => rowButton("dia")?.click(),
  importing: () => {
    rowButton("dia")?.click();
    setTimeout(() => button("Import 2 profiles")?.click(), 200);
  },
  results: () => {
    rowButton("safari")?.click();
    patch("safari", { state: "ready", profiles: one("safari", 12) });
    setTimeout(() => button("I've turned it on")?.click(), 200);
    setTimeout(() => button("Import 1 profile")?.click(), 500);
  },
  fda: () => rowButton("safari")?.click(),
  "fda-still-off": () => {
    rowButton("safari")?.click();
    setTimeout(() => button("I've turned it on")?.click(), 200);
  },
  "blocked-keychain": () => {
    rowButton("edge")?.click();
    setTimeout(() => button("Import 1 profile")?.click(), 200);
  },
  "blocked-running": () => {
    rowButton("helium")?.click();
    setTimeout(() => button("Import 1 profile")?.click(), 200);
  },
};

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (view && !params.has("drive")) {
      const timer = setTimeout(() => VIEWS[view]?.(), 300);
      return () => clearTimeout(timer);
    }
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    const steps: (() => void | Promise<void>)[] = [
      () => {
        seen.page = {
          title: text(document.querySelector("main h1")),
          section: text(document.querySelector("main section h2")),
          description: text(document.querySelector("main section h2 + p")),
          refresh: button("Refresh") !== undefined,
        };
        seen.rows = rows();
        rowButton("dia")?.click();
      },
      // The picker: what isn't here yet ticked as a new profile, and every part.
      () => {
        seen.choose = { ...readDialog(), profiles: profiles() };
        seen.bring = bring();
        tickProfile("Work");
      },
      () => {
        seen.personalOnly = readDialog()?.buttons.slice(-2);
        tickProfile("Personal");
      },
      () => {
        seen.noneTicked = readDialog()?.buttons.slice(-2);
        tickProfile("Personal");
        tickProfile("Work");
      },
      () => {
        // Bookmarks off, then on again: the import asks for them.
        all<HTMLButtonElement>('[role="checkbox"]', dialog() ?? document).at(-2)?.click();
      },
      () => {
        seen.bookmarksOff = bring()[2];
        all<HTMLButtonElement>('[role="checkbox"]', dialog() ?? document).at(-2)?.click();
      },
      () => button("Import 2 profiles")?.click(),
      // Importing: no close button, and neither Esc nor a click outside ends it.
      () => {
        seen.importing = { ...readDialog(), rowsEnabled: rows().filter((item) => item.enabled).map((item) => item.name) };
        escape();
        // A whole press: the dialog weighs an outside press when it is released.
        const overlay = document.querySelector<HTMLElement>('[data-slot="dialog-overlay"]');
        overlay?.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, pointerType: "mouse" }));
        overlay?.dispatchEvent(new PointerEvent("pointerup", { bubbles: true, pointerType: "mouse" }));
        overlay?.click();
      },
      () => {
        seen.stillImporting = dialog()?.dataset.step ?? null;
        release?.();
      },
      // The results, part by part; a part that failed tries again alone.
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 200));
        seen.results = { ...readDialog(), profiles: results() };
        button("Try again")?.click();
      },
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 200));
        seen.retried = results()[1]?.parts.find((part) => part[1]?.startsWith("Bookmarks"));
        button("Done")?.click();
      },
      () => {
        seen.imported = { dialog: readDialog(), dia: rows().find((item) => item.name === "Dia") };
        rowButton("safari")?.click();
      },
      // Full Disk Access: still off the first time.
      () => {
        seen.fda = readDialog();
        button("Open System Settings")?.click();
        button("I've turned it on")?.click();
      },
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 200));
        seen.fdaStillOff = readDialog();
        patch("safari", { state: "ready", profiles: one("safari", 12) });
        button("I've turned it on")?.click();
      },
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 300));
        seen.fdaOn = readDialog()?.step;
        button("Import 1 profile")?.click();
      },
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 200));
        seen.safari = { description: readDialog()?.description, buttons: readDialog()?.buttons, profiles: results() };
        button("Done")?.click();
        patch("firefox", { profiles: one("firefox", 5) });
        button("Refresh")?.click();
      },
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 200));
        rowButton("firefox")?.click();
      },
      () => button("Import 1 profile")?.click(),
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 200));
        seen.firefox = { description: readDialog()?.description, buttons: readDialog()?.buttons, passwords: results()[0]?.parts.at(-1) };
        button("Done")?.click();
      },
      () => rowButton("edge")?.click(),
      () => button("Import 1 profile")?.click(),
      // Edge's Keychain key refused: the blocked dialog.
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 200));
        seen.keychain = readDialog();
        button("Close", dialog() ?? document)?.click();
      },
      () => {
        seen.keychainClosed = dialog() === null;
        rowButton("helium")?.click();
      },
      () => {
        seen.heliumParts = all('[data-slot="import-dialog"] [role="checkbox"]').length;
        seen.heliumNote = readDialog()?.notes[1];
        button("Import 1 profile")?.click();
      },
      // Helium is open: "I've quit it" looks again and asks again.
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 200));
        seen.running = readDialog();
        patch("helium", { state: "ready" });
        button("I've quit it")?.click();
      },
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 300));
        button("Import 1 profile")?.click();
      },
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 200));
        seen.quit = readDialog()?.description;
        button("Done")?.click();
      },
      () => rowButton("arc")?.click(),
      () => button("Import 1 profile")?.click(),
      // A read that fails shows in the results.
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 200));
        seen.readFailed = { description: readDialog()?.description, buttons: readDialog()?.buttons, profiles: results() };
        // A browser that can't be read makes no profile.
        seen.madeForFailedRead = allProfiles().some((each) => each.name === "Arc: Default");
        button("Done")?.click();
      },
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 200));
        seen.listedBefore = listed;
        rowButton("chrome")?.click();
      },
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 200));
        seen.rechecked = listed > (seen.listedBefore as number);
        useApp.setState((state) => ({ settings: { ...state.settings, browserEnabled: false } }));
      },
      () => {
        seen.off = {
          note: text(document.querySelector('[data-slot="browser-off"]')),
          enabled: rows().filter((item) => item.enabled).map((item) => item.name),
        };
        mode = "empty";
        button("Refresh")?.click();
      },
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 200));
        seen.empty = all("main section p").map(text);
        mode = "error";
        button("Refresh")?.click();
      },
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 200));
        seen.error = all("main section p").map(text);
        seen.asked = asked.filter((entry) => entry !== "heliumQuit");
        setResult(JSON.stringify(seen));
      },
    ];
    let stopped = false;
    const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
    void (async () => {
      await wait(400);
      for (const step of steps) {
        if (stopped) return;
        const done = step();
        if (done instanceof Promise) await done;
        await wait(150);
      }
    })();
    return () => {
      stopped = true;
    };
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground relative flex h-screen">
        <Toaster className="top-2" />
        <main className="flex h-full min-w-0 flex-1">
          <div className="min-w-0 flex-1">
            <BrowserImportPage />
          </div>
        </main>
      </div>
      {result && <pre id="browser-import-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
