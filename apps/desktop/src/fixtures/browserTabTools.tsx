/**
 * The Browser tab's address suggestions, its page's question for the camera and microphone, and
 * its Downloads popover, over a faked shell. ?drive=1 drives them and writes what it saw to
 * #browser-tab-tools-result (BrowserTabToolsRender.test.ts reads it).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { BrowserTab } from "@/app/conversation/BrowserTab";
import { modalOpen } from "@/app/conversation/browserLayer";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { BrowserDownload, BrowserVisit } from "@/ipc/generated";
import { type BrowserAsking, useBrowsers } from "@/state/browsers";
import { CHAT_TAB, useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
const ID = "browser:self";
const OTHER = "browser:guide";

if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}

useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  settings: { ...state.settings, browserEnabled: true, browserFullUrl: false },
  selection: { type: "conversation", id: "session" },
}));
const page = (url: string, title: string) => ({ url, title, loading: false, ready: true, blocked: null });
useBrowsers.setState({
  pages: { [ID]: page("https://www.example.com/", "Example"), [OTHER]: page("https://docs.example.org/guide", "The guide") },
});
useSessionTabs.setState({
  sessions: {
    session: {
      tabs: [
        { kind: "browser", id: ID, opener: CHAT_TAB, url: "https://www.example.com/", title: "" },
        { kind: "browser", id: OTHER, opener: CHAT_TAB, url: "https://docs.example.org/guide", title: "" },
      ],
      active: ID,
    },
  },
});

let visits: BrowserVisit[] = [
  { id: 1, url: "https://docs.example.org/guide", title: "The guide", atMs: 3, agent: false },
  { id: 2, url: "https://news.test/docs", title: "Docs news", atMs: 2, agent: false },
  { id: 3, url: "https://old.test/docs-archive", title: "", atMs: 1, agent: false },
];
const downloads: BrowserDownload[] = [
  { id: 11, url: "https://files.test/report.pdf", path: "/Users/me/Downloads/report.pdf", atMs: 2, state: "done" },
  { id: 12, url: "https://files.test/big.zip", path: "/Users/me/Downloads/big.zip", atMs: 1, state: "failed" },
];

/** What the tab asked the shell for that matters here. */
const asked: unknown[] = [];
mockIPC(async (command, payload) => {
  const args = (payload ?? {}) as Record<string, unknown>;
  switch (command) {
    case "browser_history": {
      const query = String(args.query).toLowerCase();
      asked.push({ history: args.query });
      return visits.filter((visit) => `${visit.url} ${visit.title}`.toLowerCase().includes(query));
    }
    case "browser_history_remove":
      asked.push({ removeHistory: args.ids });
      visits = visits.filter((visit) => !(args.ids as number[]).includes(visit.id));
      return null;
    case "browser_downloads":
      return downloads;
    case "browser_download_open":
      asked.push({ openDownload: args.id });
      return null;
    case "reveal_path":
      asked.push({ reveal: args.path });
      return null;
    case "browser_permission":
      asked.push({ permission: args.request, answer: args.answer });
      return null;
    case "browser_open":
    case "browser_navigate":
      asked.push({ navigate: args.url });
      return null;
    case "browser_site_count":
      return 0;
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
const field = () => one<HTMLInputElement>('input[aria-label="Address"]');
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (check: () => boolean) => {
  for (let tries = 0; tries < 60 && !check(); tries++) await wait(50);
};
/** Types into the address field as React hears it. */
const typeField = (value: string) => {
  const input = field();
  if (!input) return;
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
};
const key = (name: string, target: EventTarget | null = field()) =>
  target?.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true }));

const suggestions = () => one('[data-slot="browser-suggestions"]');
/** The suggestion rows: their kind, what they say, and whether the arrows are on them. */
const rows = () =>
  all('[role="option"]', suggestions() ?? document).map((row) => ({
    kind: row.dataset.kind,
    text: text(row).replace(/\s+/g, " "),
    selected: row.getAttribute("aria-selected") === "true",
  }));
const prompt = () => one('[data-slot="browser-permission"]');
const ask = (asking: BrowserAsking) =>
  useBrowsers.setState(({ pages }) => ({ pages: { ...pages, [ID]: { ...pages[ID]!, asking } } }));
const LAYERS = "[role=dialog], [role=alertdialog], [data-radix-popper-content-wrapper]";

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (!params.has("drive")) return;
    let stopped = false;
    void (async () => {
      await until(() => field() !== null);
      await wait(200);
      const seen: Record<string, unknown> = {};

      // Typing suggests: search the web first, the session's other tab to switch to, then the
      // history (the open tab's page not again). The field keeps the keyboard.
      field()?.focus();
      typeField("docs");
      await until(() => rows().length === 4);
      seen.typed = {
        rows: rows(),
        expanded: field()?.getAttribute("aria-expanded"),
        focused: document.activeElement === field(),
        width: Math.round(suggestions()?.getBoundingClientRect().width ?? 0),
        address: Math.round(one('[data-slot="browser-address"]')?.getBoundingClientRect().width ?? 0),
      };
      // ↓ walks the rows, ↑ from the first goes back to the field.
      key("ArrowDown");
      await wait(50);
      key("ArrowDown");
      await wait(50);
      seen.arrows = {
        selected: rows().findIndex((row) => row.selected),
        active: field()?.getAttribute("aria-activedescendant"),
      };
      // Enter on "Switch to tab" brings that tab to the front, the address untouched.
      const switched = asked.length;
      key("Enter");
      await until(() => useSessionTabs.getState().sessions.session?.active === OTHER);
      await wait(50);
      seen.switched = {
        active: useSessionTabs.getState().sessions.session?.active,
        suggestions: suggestions() !== null,
        asked: asked.slice(switched).filter((entry) => !("history" in (entry as object))),
      };
      useSessionTabs.setState(({ sessions }) => ({ sessions: { ...sessions, session: { ...sessions.session!, active: ID } } }));

      // × takes a page out of the history; its row goes, the keyboard stays in the field.
      field()?.focus();
      typeField("docs");
      await until(() => rows().length === 4);
      const removed = asked.length;
      button("Remove Docs news from history", suggestions() ?? document)?.click();
      await until(() => rows().length === 3);
      seen.removed = {
        rows: rows().map((row) => row.kind),
        focused: document.activeElement === field(),
        asked: asked.slice(removed).filter((entry) => !("history" in (entry as object))),
      };

      // Esc closes the suggestions and keeps what was typed; a second Esc puts the page back.
      key("Escape");
      await until(() => suggestions() === null);
      seen.escape = { suggestions: suggestions() !== null, value: field()?.value };
      key("Escape");
      await wait(50);
      seen.escapeAgain = field()?.value;

      // A click on the first row searches the web.
      typeField("cats");
      await until(() => rows().length === 1);
      const searched = asked.length;
      all<HTMLElement>('[role="option"]', suggestions() ?? document)[0]?.click();
      await until(() => asked.length > searched + 1);
      seen.search = asked.slice(searched).filter((entry) => !("history" in (entry as object)));
      field()?.blur();
      await wait(100);

      // The page asks for the camera and the microphone: the question hangs from the address,
      // above the page, which stays live around it.
      ask({ request: 7, origin: "https://meet.example", camera: true, microphone: true });
      await until(() => prompt() !== null);
      await wait(100);
      const box = prompt()?.getBoundingClientRect();
      const addressBox = one('[data-slot="browser-address"]')?.getBoundingClientRect();
      seen.prompt = {
        text: text(prompt()).replace(/\s+/g, " "),
        above: prompt()?.hasAttribute("data-above-page"),
        modal: modalOpen(document.querySelectorAll(LAYERS)),
        startAligned: box && addressBox ? Math.round(box.left - addressBox.left) : null,
        below: box && addressBox ? box.top >= addressBox.bottom : null,
        buttons: all("button", prompt() ?? document).map((element) => text(element.querySelector(".sr-only")) || text(element)),
      };
      // A click elsewhere in the app leaves the question.
      document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
      await wait(100);
      seen.promptStays = prompt() !== null;
      const answers = asked.length;
      button("Allow while visiting the site", prompt() ?? document)?.click();
      await until(() => prompt() === null);
      for (const [request, answer] of [[8, "Allow this time"], [9, "Never allow"], [10, "Close permission request"]] as const) {
        ask({ request, origin: "https://meet.example", camera: request !== 9, microphone: request === 9 });
        await until(() => prompt() !== null);
        await wait(50);
        if (request === 9) seen.microphoneOnly = text(prompt()).replace(/\s+/g, " ");
        button(answer, prompt() ?? document)?.click();
        await until(() => prompt() === null);
      }
      ask({ request: 11, origin: "https://meet.example", camera: true, microphone: false });
      await until(() => prompt() !== null);
      key("Escape", document);
      await until(() => prompt() === null);
      seen.answers = asked.slice(answers);

      // Downloads: the newest, each finished one to open or show in Finder, and all of them.
      const downloadsAsked = asked.length;
      button("Downloads")?.click();
      await until(() => one('[data-slot="browser-downloads"]') !== null);
      await wait(50);
      const popover = one('[data-slot="browser-downloads"]') ?? document;
      seen.downloads = all("li", popover).map((row) => text(row).replace(/\s+/g, " "));
      button("Open", popover)?.click();
      button("Show in Finder", popover)?.click();
      await wait(50);
      button("All downloads", popover)?.click();
      await until(() => useApp.getState().selection.type === "settings");
      seen.downloadsAsked = asked.slice(downloadsAsked);
      seen.allDownloads = useApp.getState().selection;

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
          <BrowserTab conversationId={ID} active />
        </main>
      </div>
      {result && <pre id="browser-tab-tools-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
