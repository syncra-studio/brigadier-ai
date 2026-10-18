/**
 * A standalone Vite entry, excluded from the app entry and production build: a finished research
 * answer in the app's own conversation view (Q79). The answer has numbered markers, one trailing
 * `### Sources` list, a link to its full report (a page `save_report` made) and the `### Details`
 * a thread writes. `kind=session` shows it in a session (else a Chat); `changed=1` has the request
 * change code.
 *
 * `drive=1` reads what shows, opens the sources and the report, and writes what it saw to
 * `<pre id="research-answer-result">` (ResearchAnswerRender.test.ts reads it). Without it the
 * page holds the answer still, for a screenshot.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/ConversationView";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, ConversationView as View, Message, Page, Request, UserRequest } from "@/ipc/generated";
import { openConversation } from "@/state/actions";
import { sessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const session = query.get("kind") === "session";
const changed = query.get("changed") === "1";
const ID = "research-answer";
const PAGE = "5".repeat(32);
const begun = Date.now() - 600_000;

const ANSWER = [
  "Start with Meshy's free plan: it exports print-ready STL and is what most guides recommend [1]. For exact, measured parts use Zoo instead [2, 3].",
  "",
  "- Figurines and gifts: Meshy, or Rodin for the finest detail [3].",
  "- Functional parts: Zoo writes editable CAD [2].",
  "",
  `The full comparison is in [Full report](brigadier-page:${PAGE}).`,
  "",
  "### Sources",
  "1. [Meshy pricing](https://meshy.ai/pricing)",
  "2. [Zoo Text-to-CAD](https://zoo.dev/text-to-cad)",
  "3. [Rodin Gen-2.5](https://hyper3d.ai/)",
  "",
  "### Details",
  "Checks: none run. Review: none. Not checked: nothing.",
].join("\n");

const conversation = {
  id: ID, kind: session ? "session" : "chat", projectId: null, title: "Which AI models for 3D printing", pinnedAtMs: null,
  createdAtMs: begun, updatedAtMs: begun, lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
  setup: session
    ? {
        type: "session", repo: "/tmp/research-answer-fixture", environment: { type: "local" }, permission: "fullAccess", planMode: false,
        workersSeeUncommitted: null, orchestrator: { provider: "claude", model: null, effort: null },
      }
    : { type: "chat", model: { provider: "claude", model: null, effort: null, fast: false } },
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

const read: string[] = [];

function view(): View {
  return {
    conversation, context: null,
    messages: { messages, hasMore: false },
    tasks: [], approvals: [], questions: [], plans: [], requests: [request], workerSteps: [],
    orchestratorSteps: [], machineSteps: [], thinking: [], compactions: [], decisions: [], waiting: [],
    queue: { items: [], paused: false },
    run: "idle", runRequest: null, head: "a1", ratings: {}, streaming: null,
    notices: [], memories: [], overnight: [], reviews: [], previews: [],
  } as unknown as View;
}

mockIPC((command, payload) => {
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

useApp.setState({
  conversations: { [ID]: conversation },
  connection: { status: "connected", daemon: null, reason: null },
});
openConversation(ID);

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
  await until(() => all("[data-slot=citation]").length > 0);
  await wait(200);
  const answer = all("[data-answer]")[0];
  const sources = all<HTMLDetailsElement>("[data-slot=answer-sources]");
  const shown = {
    // What reads with everything folded: a closed fold's list doesn't.
    first: answer?.innerText.split("\n")[0] ?? null,
    citations: all<HTMLAnchorElement>("[data-slot=citation]").map((marker) => `${text(marker)} ${marker.getAttribute("href")}`),
    sources: sources.map((fold) => ({ summary: text(fold.querySelector("summary")), open: fold.open })),
    // No source shows before the fold opens; the old wall of links is gone from the answer.
    linksShown: all<HTMLAnchorElement>("[data-answer] a[href^=http]").filter((link) => link.checkVisibility() && link.dataset.slot !== "citation").length,
    details: all<HTMLDetailsElement>("[data-slot=report-details]").map((fold) => text(fold.querySelector("summary"))),
    report: all("[data-slot=report-link]").map((link) => text(link)),
  };
  sources[0]?.querySelector("summary")?.click();
  await wait(300);
  const listed = all("[data-slot=answer-source-list] li").map((item) => text(item)?.replace(/\u2060/g, "") ?? null);
  all<HTMLAnchorElement>("[data-slot=report-link]")[0]?.click();
  await until(() => !!document.querySelector("[data-slot=report-dialog]") || sessionTabs(ID).tabs.some((tab) => tab.kind === "page"), 3000);
  await wait(200);
  const tab = sessionTabs(ID).tabs.find((each) => each.kind === "page");
  return {
    ...shown,
    listed,
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
