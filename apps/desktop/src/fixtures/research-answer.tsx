/**
 * A standalone Vite entry, excluded from the app entry and production build: a finished research
 * answer in the app's own conversation view (Q87). The answer links its sources where it uses them,
 * links its full report (a page `brigadier report save` made) and ends with a `### Details` section.
 * `kind=session` shows it in a session (else a Chat); `changed=1` has the request change code.
 * `bold=1` writes its breakdown in bold-led lines, as models still sometimes do (#319), with one
 * bold run mid-sentence; `details=0` drops its Details. `old=1` is an answer written before Q87:
 * numbered markers and a trailing `### Sources` list, which now read as plain Markdown.
 * `preview=1` has the answer name the one local page left running (a preview an earlier turn
 * started, still up); `preview=2` names two.
 * `late=1` adds a late note under the answer; `local=system` sends local pages to the default
 * browser (Settings › Browser).
 *
 * `drive=1` reads what shows, opens the report, and writes what it saw to
 * `<pre id="research-answer-result">` (ResearchAnswerRender.test.ts reads it). Without it the
 * page holds the answer still, for a screenshot.
 */
import "@/fixtures/cliView";
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, ConversationView as View, Message, Page, Preview, Request, UserRequest } from "@/ipc/generated";
import { openConversation } from "@/state/actions";
import { sessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const session = query.get("kind") === "session";
const changed = query.get("changed") === "1";
const bold = query.get("bold") === "1";
const detailed = query.get("details") !== "0";
const old = query.get("old") === "1";
const previews = Number(query.get("preview") ?? 0);
const late = query.get("late") === "1";
const ID = "research-answer";
const PAGE = "5".repeat(32);
const begun = Date.now() - 600_000;

const ANSWER = old
  ? [
      "Start with Meshy's free plan: it exports print-ready STL [1]. For exact, measured parts use Zoo instead [2, 3].",
      "",
      "- Figurines and gifts: Meshy, or Rodin for the finest detail [3].",
      "- Functional parts: Zoo writes editable CAD [2].",
      "",
      "### Sources",
      "1. [Meshy pricing](https://meshy.ai/pricing)",
      "2. [Zoo Text-to-CAD](https://zoo.dev/text-to-cad)",
      "3. [Rodin Gen-2.5](https://hyper3d.ai/)",
    ].join("\n")
  : [
      "Start with Meshy's free plan: it exports print-ready STL ([pricing](https://meshy.ai/pricing)). For exact, measured parts use [Zoo](https://zoo.dev/text-to-cad) instead.",
      "",
      ...(bold
        ? [
            "- **Figurines and gifts:** Meshy, or Rodin for the finest detail.",
            "- **Zoo** writes editable CAD for functional parts.",
            "",
            "**Print monitoring**",
            "Obico watches the camera and **stops a failed print** on its own.",
          ]
        : [
            "- Figurines and gifts: Meshy, or [Rodin](https://hyper3d.ai/) for the finest detail.",
            "- Functional parts: Zoo writes editable CAD.",
          ]),
      "",
      `The full comparison is in [Full report](brigadier-page:${PAGE}).`,
      ...(previews ? ["", previews > 1 ? "The page runs at http://localhost:5173/ and its API at http://localhost:3000/." : "Try it at http://localhost:5173/ (still running)."] : []),
      ...(detailed ? ["", "### Details", "Checks: none run. Review: none. Not checked: nothing."] : []),
    ].join("\n");

const conversation = {
  id: ID, kind: session ? "session" : "chat", projectId: null, title: "Which AI models for 3D printing", pinnedAtMs: null,
  createdAtMs: begun, updatedAtMs: begun, lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null,
  setup: session
    ? {
        type: "session", repo: "/tmp/research-answer-fixture", environment: { type: "local" }, permission: "full", planMode: false,
        workersSeeUncommitted: null, orchestrator: { provider: "claude", model: null, effort: null },
      }
    : { type: "chat", model: { provider: "claude", model: null, effort: null, fast: false }, permission: "acceptEdits" },
} as unknown as Conversation;

const messages: Message[] = [
  {
    id: "r1", conversationId: ID, seq: 1, role: "user", text: "Which AI models are best for 3D printing? Do some research.", blob: null,
    createdAtMs: begun, attachments: [], mentions: [], model: null, requestId: "r1", parentId: null,
  },
  {
    id: "a1", conversationId: ID, seq: 2, role: "assistant", text: ANSWER, blob: null, createdAtMs: begun + 60_000,
    attachments: [], mentions: [], model: null, requestId: "r1", parentId: "r1",
  },
  ...(late
    ? [{
        id: "a2", conversationId: ID, seq: 3, role: "assistant", text: "Update: CI passed on c9cb8b3.", blob: null, createdAtMs: begun + 90_000,
        attachments: [], mentions: [], model: null, requestId: "r1", parentId: "a1",
      } as Message]
    : []),
];

const request = {
  id: "r1", conversationId: ID, preview: "", state: { type: "done" }, startedAtMs: begun, endedAtMs: begun + 60_000,
  steeredInto: null, steeredAfter: null, undo: null, worked: [{ fromMs: begun, toMs: begun + 60_000 }], quotaWait: false,
  ...(changed
    ? { changes: { stat: { files: [{ path: "src/print.ts", insertions: 3, deletions: 1, binary: false }], insertions: 3, deletions: 1 }, settled: true } }
    : {}),
} as UserRequest;

const page: Page = {
  id: PAGE, revision: "rev-1", createdMs: begun, modifiedMs: begun,
  content: { title: "AI models for 3D printing", emoji: null, body: "## Figurines\n\nMeshy, Tripo and Rodin compared in full." },
} as unknown as Page;

// The dev server the answer's page runs on, started before this turn and still running.
const server = (port: number): Preview => ({
  id: `preview-${port}`, conversationId: ID, name: "npm run dev", command: "npm run dev", workdir: "/w", workspace: "/w", pid: 1,
  state: { type: "running" }, startedAtMs: begun - 60_000, endedAtMs: null, log: null, url: `http://localhost:${port}/`, kind: "web", building: false,
});

const read: string[] = [];
const system: string[] = [];

function view(): View {
  return {
    conversation, context: null,
    messages: { messages, hasMore: false },
    tasks: [], approvals: [], questions: [], requests: [request], workerSteps: [],
    orchestratorSteps: [], machineSteps: [], thinking: [], compactions: [], decisions: [],
    queue: { items: [], paused: false },
    run: "idle", runRequest: null, head: "a1", ratings: {}, streaming: null,
    notices: [], previews: previews ? [server(5173), ...(previews > 1 ? [server(3000)] : [])] : [],
  } as unknown as View;
}

mockIPC((command, payload) => {
  if (command === "open_url") system.push((payload as { url: string }).url);
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getConversation":
      return { method: req.method, view: view() };
    case "pageRead":
      read.push(req.id);
      return { method: req.method, page };
    case "listFiles":
      return { method: req.method, files: [], truncated: false };
    default:
      return { method: req.method };
  }
});

useApp.setState((s) => ({
  settings: { ...s.settings, localLinksIn: query.get("local") === "system" ? "system" : s.settings.localLinksIn },
  conversations: { [ID]: conversation },
  connection: { status: "connected", daemon: null, reason: null },
}));
await openConversation(ID);

function Page() {
  const selection = useApp((s) => s.selection);
  return (
    <TooltipProvider>
      <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
        <main className="relative flex min-h-0 w-full flex-1 flex-col">
          <ConversationView selection={selection} />
        </main>
      </SidebarProvider>
    </TooltipProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Page />);

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [...root.querySelectorAll<T>(selector)];
const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, " ").trim() ?? null;

async function until(found: () => boolean, ms = 10_000): Promise<void> {
  for (const end = performance.now() + ms; !found() && performance.now() < end; ) await wait(50);
}

async function drive(): Promise<object> {
  await until(() => all("[data-answer] li").length > 0);
  await wait(200);
  const answer = all("[data-answer]")[0];
  const shown = {
    // What reads with everything folded: a closed fold's text doesn't.
    first: answer?.innerText.split("\n")[0] ?? null,
    // Sources are the answer's own links, each where it is used.
    links: all<HTMLAnchorElement>("[data-answer] a[href^=http]").filter((link) => link.checkVisibility()).map((link) => `${text(link)} ${link.getAttribute("href")}`),
    folds: all("[data-answer] details > summary").map((summary) => text(summary)),
    markers: all("[data-slot=citation], [data-slot=answer-pick]").length,
    report: all("[data-slot=report-link]").map((link) => text(link)),
    bold: all("[data-answer] strong").map((run) => text(run)),
    items: all("[data-answer] li").map((item) => text(item)),
    preview: text(document.querySelector("[data-slot=preview-card]")),
  };
  // The web preview opens in the session's browser.
  document.querySelector<HTMLButtonElement>("[data-slot=preview-open] > button")?.click();
  await wait(100);
  const browser = sessionTabs(ID).tabs.find((each) => each.kind === "browser");
  const browsed = browser?.kind === "browser" ? browser.url : null;
  all<HTMLAnchorElement>("[data-slot=report-link]")[0]?.click();
  await until(() => !!document.querySelector("[data-slot=report-dialog]") || sessionTabs(ID).tabs.some((tab) => tab.kind === "page"), 3000);
  await wait(200);
  const tab = sessionTabs(ID).tabs.find((each) => each.kind === "page");
  return {
    ...shown,
    browsed,
    system,
    opened: {
      tab: tab?.kind === "page" ? { pageId: tab.pageId, active: sessionTabs(ID).active === tab.id } : null,
      dialog: text(document.querySelector("[data-slot=report-dialog]")),
      read,
    },
  };
}

if (query.has("drive")) {
  void drive().then((seen) => {
    const result = document.createElement("pre");
    result.id = "research-answer-result";
    result.textContent = JSON.stringify(seen);
    document.body.append(result);
  });
}
