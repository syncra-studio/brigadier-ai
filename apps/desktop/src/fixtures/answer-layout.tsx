/**
 * A standalone Vite entry, excluded from the app entry and production build: a finished answer
 * in the app's own conversation view, laid out in its optional parts when it has them (Q86).
 * `a=` picks the answer (answerLayoutTexts.ts): `trading` (every part), `fact`, `howto`, `free`
 * (no part) or `coding`. `kind=session` shows it in a session (else a Chat); `changed=1` has the
 * request change code. `open=Why,Taxes` opens the folds whose titles start so; `top=1` holds the
 * thread at the question, as a reader first meets it.
 *
 * `drive=1` reads what shows, reads it aloud, clicks the first Ask next question and writes what
 * it saw to `<pre id="answer-layout-result">` (AnswerLayoutRender.test.ts reads it). Without it
 * the page holds the answer still, for a screenshot.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/ConversationView";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, ConversationView as View, Message, Page, Request, UserRequest } from "@/ipc/generated";
import { openConversation } from "@/state/actions";
import { useApp } from "@/state/store";

import {
  CODING,
  CODING_QUESTION,
  FACT,
  FACT_QUESTION,
  FREE,
  FREE_QUESTION,
  HOWTO,
  HOWTO_QUESTION,
  TRADING,
  TRADING_QUESTION,
} from "@/fixtures/answerLayoutTexts";

const query = new URLSearchParams(location.search);
const session = query.get("kind") === "session";
const changed = query.get("changed") === "1";
const ID = "answer-layout";
const PAGE = "5".repeat(32);
const begun = Date.now() - 600_000;

const ANSWERS: Record<string, [string, string, string]> = {
  trading: [TRADING_QUESTION, TRADING, "How can i start trading"],
  fact: [FACT_QUESTION, FACT, "When to declare investment income"],
  howto: [HOWTO_QUESTION, HOWTO, "Open the practice account"],
  free: [FREE_QUESTION, FREE, "ETF or mutual fund"],
  coding: [CODING_QUESTION, CODING, "Fix the login timeout"],
};
const [ASKED, RAW, TITLE] = ANSWERS[query.get("a") ?? "trading"]!;
const ANSWER = RAW.replace("PAGE", PAGE);

const conversation = {
  id: ID, kind: session ? "session" : "chat", projectId: null, title: TITLE, pinnedAtMs: null,
  createdAtMs: begun, updatedAtMs: begun, lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
  setup: session
    ? {
        type: "session", repo: "/tmp/answer-layout-fixture", environment: { type: "local" }, permission: "fullAccess", planMode: false,
        workersSeeUncommitted: null, orchestrator: { provider: "claude", model: null, effort: null },
      }
    : { type: "chat", model: { provider: "claude", model: null, effort: null, fast: false } },
} as unknown as Conversation;

const messages: Message[] = [
  {
    id: "r1", conversationId: ID, seq: 1, role: "user", text: ASKED, blob: null,
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
    ? { changes: { stat: { files: [{ path: "auth/session.ts", insertions: 3, deletions: 1, binary: false }], insertions: 3, deletions: 1 }, settled: true } }
    : {}),
} as UserRequest;

const page: Page = {
  id: PAGE, revision: "rev-1", createdMs: begun, modifiedMs: begun,
  content: { title: "Investing from Moldova", emoji: null, body: "## Brokers\n\nThe full findings." },
} as unknown as Page;

const sent: string[] = [];
const spoken: string[] = [];
speechSynthesis.speak = (utterance) => void spoken.push(utterance.text);
speechSynthesis.cancel = () => {};

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

let sequence = 10;
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getConversation":
      return { method: req.method, view: view() };
    case "pageRead":
      return { method: req.method, page };
    case "listFiles":
      return { method: req.method, files: [], truncated: false };
    case "sendMessage": {
      sent.push(req.text);
      const message = {
        id: `sent-${sequence}`, conversationId: req.conversationId, seq: sequence++, role: "user", text: req.text, blob: null,
        attachments: req.attachments, mentions: req.mentions, createdAtMs: Date.now(), model: null,
        requestId: `sent-request-${sequence}`, parentId: null,
      };
      return { method: req.method, outcome: { type: "sent", message } };
    }
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
const text = (element: Element | null | undefined) => element?.textContent?.replace(/[\s⁠]+/g, " ").trim() ?? null;

async function until(found: () => boolean, ms = 10_000): Promise<void> {
  for (const end = performance.now() + ms; !found() && performance.now() < end; ) await wait(50);
}

const opened = (query.get("open") ?? "").split(",").filter(Boolean);
async function openFolds(): Promise<void> {
  await until(() => !!document.querySelector("[data-answer] p, [data-answer] li"));
  await wait(300);
  for (const fold of all<HTMLDetailsElement>("[data-answer] details")) {
    const title = text(fold.querySelector("summary")) ?? "";
    if (opened.some((name) => title.toLowerCase().startsWith(name.toLowerCase()))) fold.open = true;
  }
}

async function drive(): Promise<object> {
  await openFolds();
  const answer = document.querySelector<HTMLElement>("[data-answer]")!;
  const shown = {
    layout: !!answer.querySelector("[data-slot=answer-layout]"),
    bottomLine: text(answer.querySelector("[data-slot=answer-bottom-line]")),
    steps: all("[data-slot=answer-steps] > li", answer).map((step) => text(step)),
    picks: all("[data-slot=answer-pick]", answer).map((badge) => ({ badge: text(badge), row: text(badge.closest("tr")?.querySelector("td")), tinted: getComputedStyle(badge.closest("tr")!).backgroundColor })),
    avoid: all("[data-slot=answer-avoid] li", answer).length,
    avoidTitle: text(answer.querySelector("[data-slot=answer-avoid] > div")),
    chance: text(answer.querySelector("[data-slot=answer-chance] figcaption")),
    folds: all<HTMLDetailsElement>("[data-answer] details", answer).map((fold) => ({
      slot: fold.dataset.slot ?? null,
      title: text(fold.querySelector("summary")),
      open: fold.open,
      // The chevron leads the row, and shows at rest.
      chevronFirst: fold.querySelector("summary")?.firstElementChild?.tagName.toLowerCase() === "svg",
      chevronShown: Number(getComputedStyle(fold.querySelector("summary svg")!).opacity) > 0,
    })),
    askNext: all("[data-slot=answer-ask-next] button", answer).map((chip) => text(chip)),
    stars: (answer.textContent ?? "").includes("★"),
    // Today's Markdown: what the answer is made of, outside its folds.
    markdown: all(".aui-md > *", answer)
      .filter((element) => !element.closest("details") && !element.closest("[data-slot^=answer-]"))
      .map((element) => element.tagName.toLowerCase()),
    citations: all("[data-slot=citation]", answer).filter((marker) => marker.checkVisibility()).length,
  };
  all<HTMLButtonElement>("[data-slot=answer-actions] button").find((button) => text(button) === "Read aloud")?.click();
  await wait(100);
  const chip = all<HTMLButtonElement>("[data-slot=answer-ask-next] button", answer)[0];
  chip?.click();
  await until(() => sent.length > 0, chip ? 3000 : 0);
  return { ...shown, spoken: spoken.at(-1) ?? null, sent };
}

if (query.has("drive")) {
  void drive().then((seen) => {
    const result = document.createElement("pre");
    result.id = "answer-layout-result";
    result.textContent = JSON.stringify(seen);
    document.body.append(result);
  });
} else {
  void openFolds();
}

// `top=1`: the thread at the top of the question, as a reader first meets it.
function toTop(): void {
  const viewport = document.querySelector<HTMLElement>('[data-slot="aui_thread-viewport"]');
  if (viewport) viewport.scrollTop = 0;
}
if (query.get("top") === "1") {
  const timer = setInterval(toTop, 200);
  setTimeout(() => clearInterval(timer), 8_000);
}
