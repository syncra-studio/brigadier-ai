/**
 * A standalone Vite entry, excluded from the app entry and production build: a session's thread
 * with a compaction in it, read back as the daemon gives it, through the real ConversationView.
 * `?case=between`: Brigadier's own `/compact` between two answers, in the order the CLI writes it
 * (the compaction, then the `/compact` it echoes), as the freelogo thread was at 11:08; with
 * `&last=1` the `/compact` turn is the thread's last, read as cut off (a thread noted before the
 * fix). `?case=midturn`: the CLI compacting on its own in the middle of a turn. IPC is mocked, no
 * daemon. What the thread shows goes to `<pre id="compaction-divider-result">`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, ConversationView as View, FileEntry, ProviderEvent, Request, ThreadTurn, ThreadView } from "@/ipc/generated";
import { openConversation } from "@/state/actions";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
const MIDTURN = params.get("case") === "midturn";
const LAST = params.has("last");

const ID = "compaction-divider";
const conversation = {
  id: ID, kind: "session", projectId: null, title: "X", pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, lifecycle: "active",
  forkedFrom: null, sideOf: null, quotaWait: null,
  setup: {
    type: "session", repo: "/tmp/projX", environment: { type: "local" }, permission: "full", planMode: false,
    workersSeeUncommitted: null, orchestrator: { provider: "claude", model: "claude-opus-5-5", effort: "low" },
  },
} as unknown as Conversation;

const T = 1_793_005_682_000;
const at = (seconds: number, event: ProviderEvent): FileEntry => ({ event, atMs: T + seconds * 1000 });
const user = (itemId: string, text: string): ProviderEvent => ({ type: "message", itemId, role: "user", text });
const reply = (itemId: string, text: string): ProviderEvent => ({ type: "message", itemId, role: "assistant", text });
const ran = (itemId: string, seconds: number): FileEntry =>
  at(seconds, { type: "command", itemId, command: "git status", cwd: null, status: "completed", exitCode: 0, output: "clean", durationMs: 100 } as ProviderEvent);
const compacted = (automatic: boolean, seconds: number): FileEntry =>
  at(seconds, { type: "compactionEnded", automatic, tokensBefore: 900_000, tokensAfter: 40_000, error: null } as ProviderEvent);
const turn = (userMessage: string, from: number, to: number, end: ThreadTurn["end"], requests: string[]): ThreadTurn => ({
  userMessage, startedAtMs: T + from * 1000, endedAtMs: T + to * 1000, end, error: null, requests,
});

const thread: ThreadView = MIDTURN
  ? {
      entries: [
        at(0, user("u1", "Tidy the release notes.")),
        ran("t1", 2),
        at(3, reply("a1", "Reading the notes first.")),
        compacted(true, 40),
        ran("t2", 42),
        at(44, reply("a2", "The release notes are tidy.")),
      ],
      turns: [turn("u1", 0, 45, "done", ["r1"])],
    }
  : {
      entries: [
        at(0, user("u1", "Is the token rotated?")),
        ran("t1", 2),
        at(4, reply("a1", "Half done: the revoke worked.")),
        // The CLI writes the compaction before the `/compact` that asked for it.
        compacted(false, 73),
        at(5, user("c1", "/compact Besides your usual summary, keep these exactly")),
        ...(LAST ? [] : [at(200, user("u3", "Here is the terminal.")), ran("t3", 202), at(204, reply("a3", "Now it is saved."))]),
      ],
      turns: [
        turn("u1", 0, 5, "done", ["r1"]),
        LAST ? { ...turn("c1", 5, 5, "interrupted", []), endedAtMs: T + 5000 } : turn("c1", 5, 74, "done", []),
        ...(LAST ? [] : [turn("u3", 200, 205, "done", ["r3"])]),
      ],
    };

/** What the daemon says of the session when it opens: nothing beyond its CLI thread. */
const view = {
  conversation, context: null, messages: { messages: [], hasMore: false },
  tasks: [], approvals: [], questions: [], requests: [], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [], compactions: [], decisions: [],
  queue: { items: [], paused: false }, run: "idle", runRequest: null, head: null, ratings: {}, streaming: null, notices: [], previews: [],
} as unknown as View;

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getConversation": return { method: req.method, view };
    case "threadView": return { method: req.method, view: thread };
    case "listFiles": return { method: req.method, files: [], truncated: false };
    default: return { method: req.method };
  }
});

useApp.setState({
  conversations: { [ID]: conversation },
  connection: { status: "connected", daemon: null, reason: null },
  summaryShown: { [ID]: false },
});
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
const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, " ").trim() ?? null;
const blocks = () => [...document.querySelectorAll("[data-role=assistant]")];

for (let tries = 0; tries < 100 && blocks().length === 0; tries++) await wait(50);
await wait(300);
const dividers = [...document.querySelectorAll("[data-slot=compaction]")];
const result = document.createElement("pre");
result.id = "compaction-divider-result";
result.textContent = JSON.stringify({
  failed: text(document.querySelector("[data-slot=turn-render-error]")),
  blocks: blocks().length,
  headers: [...document.querySelectorAll("[data-slot=request-work-label]")].map(text),
  dividers: dividers.map(text),
  // Each divider's hairlines, and whether it sits in a turn's folded work or as a row of it.
  rules: dividers.map((divider) => divider.querySelectorAll(":scope > [data-rule]").length),
  inWork: dividers.filter((divider) => divider.closest("[data-slot=request-work]")).length,
  answers: blocks().map((block) => text(block.querySelector("[data-slot=aui_assistant-message-content][data-answer]"))),
  bubbles: [...document.querySelectorAll("[data-role=user]")].map(text),
  // The thread top to bottom: the user's messages, the answers and the dividers.
  order: [...document.querySelectorAll("[data-role=user], [data-slot=compaction], [data-slot=aui_assistant-message-content][data-answer]")].map((element) =>
    element.matches("[data-role=user]") ? "user" : element.matches("[data-slot=compaction]") ? "divider" : "answer",
  ),
  interrupted: /Interrupted|Brigadier quit/.test(document.querySelector("main")?.textContent ?? ""),
  page: text(document.querySelector("main"))?.slice(0, 600),
});
document.body.append(result);
