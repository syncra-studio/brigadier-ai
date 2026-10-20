/**
 * The search palette over a faked app: a project with three sessions, and a chat. ?drive=1 reads
 * the groups and rows, moves down with ↓, opens a session with ↵, then types "feed" and opens
 * Send feedback… from it, writing what it saw to #palette-result (SearchPaletteRender.test.ts
 * reads it). ?look=1 sets the palette's card, selected row, key hints and hairline beside the
 * app's shared parts and writes their looks to #palette-look. Without either, the palette stays
 * open on the first session, for screenshots.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { FeedbackDialog } from "@/app/feedback/FeedbackDialog";
import { GlobalSearch, openSearch } from "@/app/SearchDialog";
import { TooltipProvider } from "@/components/overlays/tooltip";
import { Canon, canon, look, type LookPairs } from "@/fixtures/dialogLook";
import { useFeedbackUi } from "@/state/feedback";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
// Driven, nothing animates (see the archived chats fixture).
if (params.has("drive") || params.has("look")) {
  const still = document.createElement("style");
  still.textContent = "*, *::before, *::after { animation: none !important; transition: none !important; }";
  document.head.append(still);
}

mockIPC((command, payload) => {
  if (command === "ipc_request") return { method: (payload as { request: { method: string } }).request.method };
  return null;
});

const conversation = (id: string, kind: "session" | "chat", title: string, updatedAtMs: number) =>
  ({ id, kind, title, lifecycle: "active", projectId: kind === "session" ? "demo" : null, updatedAtMs }) as never;
useApp.setState({
  catalogLoaded: true,
  connection: { status: "connected", daemon: null, reason: null },
  projects: { demo: { id: "demo", name: "brigadier-demo", repos: [{ path: "/w/brigadier-demo" }], createdAtMs: 1 } as never },
  conversations: {
    s1: conversation("s1", "session", "am nevoie sa ma ghidezi in domeniul 3d printing, care sunt pasii", 40),
    s2: conversation("s2", "session", "use computer use and go to makerworld website for 3d models", 30),
    s3: conversation("s3", "session", "hi", 20),
    c1: conversation("c1", "chat", "hi", 10),
  },
});

const q = <E extends Element = HTMLElement>(selector: string, root: ParentNode = document) =>
  root.querySelector<E & HTMLElement>(selector);
const all = <E extends Element = HTMLElement>(selector: string, root: ParentNode = document) =>
  [...root.querySelectorAll<E & HTMLElement>(selector)];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const field = () => q<HTMLInputElement>('[data-slot="search-palette"] input');
const key = (name: string) =>
  field()?.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true }));
const type = (value: string) => {
  const input = field();
  if (!input) return;
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
};
/** Each group: its label, then each row's words, part by part, a selected row marked. */
const groups = () =>
  all('[data-slot="search-palette"] [role="group"]').map((group) => [
    group.getAttribute("aria-label"),
    ...all('[role="option"]', group).map((row) => [
      ...[...row.children].map(text).filter(Boolean),
      ...(row.getAttribute("aria-selected") === "true" ? ["(selected)"] : []),
    ]),
  ]);

function Fixture() {
  const [result, setResult] = useState("");
  const [measuring, setMeasuring] = useState(false);
  const [looks, setLooks] = useState("");
  useEffect(() => {
    openSearch();
    let stopped = false;
    void (async () => {
      for (let tries = 0; tries < 100 && !field(); tries++) await wait(50);
      await wait(100);
      if (params.has("look")) {
        setMeasuring(true);
        await wait(200);
        const palette = '[data-slot="search-palette"]';
        const pairs: LookPairs = {
          card: [look(q(palette), "surface"), canon("dialog", "surface")],
          selected: [look(q(`${palette} [aria-selected="true"]`), "fill"), canon("chosen", "fill")],
          selectedInk: [look(q(`${palette} [aria-selected="true"]`), "ink"), canon("chosen", "ink")],
          key: [look(q(`${palette} kbd`), "button"), canon("kbd", "button")],
          keyType: [look(q(`${palette} kbd`), "text"), canon("kbd", "text")],
          hairline: [look(field()?.parentElement, "edgeBottom"), canon("divider", "edgeBottom")],
        };
        if (!stopped) setLooks(JSON.stringify(pairs));
        return;
      }
      if (!params.has("drive")) {
        key("ArrowDown");
        return;
      }
      const seen: Record<string, unknown> = {};
      seen.placeholder = field()?.placeholder;
      seen.focused = document.activeElement === field();
      seen.keys = all('[data-slot="search-palette"] kbd').map(text);
      seen.groups = groups();
      key("ArrowDown");
      await wait(50);
      seen.moved = groups()[1];
      key("Enter");
      await wait(200);
      seen.opened = { selection: useApp.getState().selection, closed: !q('[data-slot="search-palette"]') };
      openSearch();
      for (let tries = 0; tries < 100 && !field(); tries++) await wait(50);
      type("feed");
      await wait(100);
      seen.feedbackGroups = groups();
      key("Enter");
      await wait(400);
      seen.feedback = {
        open: useFeedbackUi.getState().open,
        focused: document.activeElement === q('[data-slot="feedback"] textarea'),
      };
      if (!stopped) setResult(JSON.stringify(seen));
    })();
    return () => {
      stopped = true;
    };
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background h-screen" />
      <GlobalSearch />
      <FeedbackDialog />
      {result && <pre id="palette-result">{result}</pre>}
      {measuring && <Canon />}
      {looks && <pre id="palette-look">{looks}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
