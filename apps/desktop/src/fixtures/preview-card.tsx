/**
 * A standalone Vite entry, excluded from the app entry and production build: the Preview card in
 * each of its states, as the user's design has them (`K53xQ`). IPC is mocked, no daemon.
 *
 * - `running`: a web preview running at localhost:3000 with two captioned screenshots; Open, its
 *   ▾ and the fold chevron.
 * - `collapsed`: the same, folded: the header counts the screenshots.
 * - `paused`: the page paused: Start again, the screenshots kept.
 * - `stopped`: the page stopped: no Open, the screenshots kept.
 * - `no-proofs`: running, no screenshots: the header line alone, no chevron.
 * - `no-record`: a page with no preview record: the address alone, no state.
 * - `proofs-only`: proofs with no page left running: "Screenshots and recordings".
 *
 * `?case=<id>` shows one case alone (for a screenshot); `?drive=1` writes what each case shows,
 * then clicks through it, to `<pre id="preview-card-result">` (PreviewCardRender.test.ts).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";
import type { ReactNode } from "react";

import { PreviewCard } from "@/app/conversation/activity/PreviewCard";
import type { BlockOrchestratorStep } from "@/app/conversation/blocks";
import { ViewContext } from "@/app/conversation/viewContext";
import { driveFold } from "@/fixtures/cardFold";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, Preview, PreviewState, Request } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { sessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const CHAT = "preview-card";

// A tall page, so a thumbnail shows its top, not the whole page shrunk.
const page = (title: string) =>
  btoa(
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 1600"><rect width="640" height="1600" fill="white"/>` +
      `<text x="32" y="56" font-family="sans-serif" font-size="28" font-weight="700" fill="black">${title}</text>` +
      Array.from({ length: 30 }, (_, row) => `<rect x="32" y="${96 + row * 48}" width="576" height="36" rx="6" fill="${row % 2 ? "whitesmoke" : "lavender"}"/>`).join("") +
      `</svg>`,
  );

const calls: string[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "readThreadImage") {
    const video = req.itemId.includes("video");
    return { method: req.method, data: video ? btoa("not a real recording") : page(req.itemId), mime: video ? "video/webm" : "image/svg+xml" };
  }
  calls.push(req.method);
  return { method: req.method };
});

const preview = (id: string, port: number, state: PreviewState): Preview => ({
  id, conversationId: CHAT, name: "npm run dev", command: "npm run dev", workdir: "/w", workspace: "/w", pid: 1, state,
  startedAtMs: 0, endedAtMs: null, log: null, url: `http://localhost:${port}/`, kind: "web", building: false,
});
useBoard.setState({
  board: {
    ...emptyBoard(CHAT),
    loaded: true,
    previews: Object.fromEntries(
      [preview("preview-1", 3000, { type: "running" }), preview("preview-2", 3002, { type: "paused" }), preview("preview-3", 3003, { type: "stopped", reason: "stopped" })].map((each) => [each.id, each]),
    ),
  },
});
useApp.setState({ selection: { type: "conversation", id: CHAT } });

const at22 = new Date(2026, 9, 19, 22, 11).getTime();
const shot = (id: string, caption: string | undefined, minute: number): BlockOrchestratorStep => ({
  position: minute, atMs: at22 + minute * 60_000, kind: { type: "proof", id, name: `${id}.png`, mime: "image/png", ...(caption ? { caption } : {}) },
});
const SHOTS = [shot("todo-added", "Added a todo with a due date", 0), shot("todo-checked", "Checked off an overdue todo", 1)];
const MIXED = [shot("readme", "The install section, fixed", 0), { position: 2, atMs: at22 + 120_000, kind: { type: "proof", id: "install-video", name: "install.webm", mime: "video/webm" } } as BlockOrchestratorStep];

const session = { id: CHAT, kind: "session", title: "Due dates", setup: { type: "session" } } as unknown as Conversation;

const CASES: { id: string; node: ReactNode }[] = [
  { id: "running", node: <PreviewCard url="http://localhost:3000/" steps={SHOTS} session /> },
  { id: "collapsed", node: <PreviewCard url="http://localhost:3000/" steps={SHOTS} session /> },
  { id: "paused", node: <PreviewCard url="http://localhost:3002/" steps={SHOTS} session /> },
  { id: "stopped", node: <PreviewCard url="http://localhost:3003/" steps={SHOTS} session /> },
  { id: "no-proofs", node: <PreviewCard url="http://localhost:3000/" steps={[]} session /> },
  { id: "no-record", node: <PreviewCard url="http://localhost:5173/" steps={[]} session /> },
  { id: "proofs-only", node: <PreviewCard url={null} steps={MIXED} session /> },
];

const only = query.get("case");
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <ViewContext.Provider value={{ selection: { type: "conversation", id: CHAT }, conversation: session, embedded: false }}>
      <div className="flex w-190 flex-col gap-6 p-6">
        {CASES.filter((each) => !only || each.id === only).map((each) => (
          <div key={each.id} data-case={each.id}>{each.node}</div>
        ))}
      </div>
    </ViewContext.Provider>
  </TooltipProvider>,
);

const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
const wait = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));
async function until(check: () => boolean, ms = 8000) {
  for (const end = performance.now() + ms; performance.now() < end && !check(); ) await frame();
  await wait(50);
}
const words = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, " ").trim() ?? null;
const at = (id: string) => document.querySelector<HTMLElement>(`[data-case="${id}"]`);

function look(id: string) {
  const card = at(id)?.querySelector<HTMLElement>("[data-slot=preview-card]");
  const thumb = card?.querySelector("[data-slot=proof] > *")?.getBoundingClientRect();
  return {
    state: card?.dataset.state ?? null,
    title: words(card?.querySelector("[data-slot=preview-title]")),
    standing: words(card?.querySelector("[data-slot=preview-state]")),
    target: words(card?.querySelector("[data-slot=preview-target]")),
    count: words(card?.querySelector("[data-slot=preview-count]")),
    buttons: [...(card?.querySelectorAll("button") ?? [])].filter((each) => !each.closest("[data-slot=proof]")).map((each) => each.getAttribute("aria-label") ?? words(each)),
    heading: words(card?.querySelector("[data-slot=preview-proofs-count]")),
    captions: [...(card?.querySelectorAll("[data-slot=proof] figcaption") ?? [])].map((each) => [...each.children].map((part) => words(part)).join(" ")),
    thumb: thumb ? Math.round(thumb.height) : null,
    // Cropped to the top of the page: the image fills its frame, its top at the frame's.
    fit: card?.querySelector("[data-slot=proof] img") ? getComputedStyle(card.querySelector("[data-slot=proof] img")!).objectPosition : null,
  };
}

/** The states that need a click to reach: folded. */
async function reach() {
  await until(() => document.querySelectorAll("[data-slot=proof] img").length >= (only ? 0 : 6));
  at("collapsed")?.querySelector<HTMLButtonElement>("[data-slot=preview-fold]")?.click();
  // The folded card's body closes as it animates.
  await until(() => !at("collapsed") || look("collapsed").captions.length === 0);
  document.body.dataset.reached = "1";
}

async function drive() {
  const out: Record<string, unknown> = {};
  out.cases = Object.fromEntries(CASES.map((each) => [each.id, look(each.id)]));
  // Open opens the page in a Browser tab of the session (Settings › Browser's default).
  at("running")?.querySelector<HTMLButtonElement>("[data-slot=preview-open] > button")?.click();
  await wait(100);
  const tabs = sessionTabs(CHAT);
  const tab = tabs.tabs.find((each) => each.id === tabs.active);
  out.opened = tab?.kind === "browser" ? tab.url : null;
  // Again: the tab already on the page comes to the front, no second one.
  at("running")?.querySelector<HTMLButtonElement>("[data-slot=preview-open] > button")?.click();
  await wait(100);
  out.browserTabs = sessionTabs(CHAT).tabs.filter((each) => each.kind === "browser").length;
  // Its ▾ picks where.
  const trigger = at("running")?.querySelector<HTMLButtonElement>("[aria-label='Open in']");
  trigger?.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0, pointerType: "mouse" }));
  await until(() => document.querySelectorAll("[role=menuitem]").length > 0, 2000);
  out.menu = [...document.querySelectorAll("[role=menuitem]")].map((each) => words(each));
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  // Paused: Start again resumes it.
  at("paused")?.querySelector<HTMLButtonElement>("[data-slot=preview-start]")?.click();
  await until(() => calls.includes("resumePreview"), 2000);
  // The folded card opens to its proofs again.
  at("collapsed")?.querySelector<HTMLButtonElement>("[data-slot=preview-fold]")?.click();
  await wait(50);
  out.unfolded = look("collapsed").captions.length;
  out.calls = calls;
  return out;
}

void reach()
  .then(() =>
    query.get("drive") === "1"
      ? drive()
      : query.get("drive") === "fold"
        ? driveFold(
            at("running")!.querySelector("[data-slot=preview-card]")!,
            at("running")!.querySelector<HTMLElement>("[data-slot=preview-open] > button")!,
            () => look("running").captions.length,
          )
        : new Promise<never>(() => {}),
  )
  .catch((error: unknown) => ({ error: String(error) }))
  .then((result) => {
    const out = document.createElement("pre");
    out.id = "preview-card-result";
    out.textContent = JSON.stringify(result);
    document.body.append(out);
  });
