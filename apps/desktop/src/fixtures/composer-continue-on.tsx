/**
 * A standalone Vite entry, excluded from the app entry and production build: a started session's
 * composer on Claude Code, with Codex installed too, IPC mocked (S38c). The model picker's Codex
 * list opens with "Continue on Codex", which asks the daemon to move the session there on the
 * model the rules allow; picking a Codex model asks the same with that model; Claude Code's own
 * list has no such item. What it saw goes as JSON into `<pre id="composer-continue-on-result">`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { model, overview } from "@/fixtures/agents";
import type { Conversation, ConversationView as View, Request } from "@/ipc/generated";
import { select } from "@/state/actions";
import { useApp } from "@/state/store";

const id = "composer-continue-on";
const start = Date.now() - 20 * 60_000;
const conversation = {
  id, kind: "session", projectId: null, title: "Tax helpers", pinnedAtMs: null, createdAtMs: start, updatedAtMs: start,
  lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null,
  setup: {
    type: "session", orchestrator: { provider: "claude", model: "opus", effort: "high" }, permission: "acceptEdits", planMode: false,
    workersSeeUncommitted: null, environment: { type: "localCheckout", repoPath: "/repo", branch: null },
  },
} as unknown as Conversation;

const asked: unknown[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getConversation":
      return {
        method: req.method,
        view: {
          conversation, context: null, messages: { messages: [], hasMore: false }, tasks: [], approvals: [], questions: [],
          requests: [], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [],
          compactions: [], decisions: [], queue: { items: [], paused: false }, run: "idle",
          runRequest: null, head: null, ratings: {}, streaming: null, notices: [], previews: [],
        } as unknown as View,
      };
    case "continueOn":
      asked.push({ provider: req.provider, model: req.model });
      return { method: req.method, when: "now" };
    default:
      return { method: req.method };
  }
});
const efforts = { efforts: ["low", "medium", "high"], defaultEffort: "medium" };
const state = useApp.getState();
useApp.setState({
  conversations: { [id]: conversation },
  connection: { status: "connected", daemon: null, reason: null },
  providers: {
    ...state.providers,
    view: {
      providers: [
        overview("claude", [model("opus", "Opus 5.5", { isDefault: true, ...efforts })]),
        overview("codex", [model("gpt-5.5", "GPT-5.5", { isDefault: true, ...efforts })]),
      ],
      sessions: [],
      fixtures: [],
    },
  },
});

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
async function until<T>(what: string, find: () => T | null | undefined | false, ms = 10_000): Promise<T> {
  const end = performance.now() + ms;
  for (;;) {
    const found = find();
    if (found) return found;
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(16);
  }
}
const key = (target: Element, name: string) =>
  target.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true }));
const trigger = () => document.querySelector<HTMLElement>("[data-slot=model-selector-trigger]");
const menu = () => document.querySelector<HTMLElement>("[data-slot=model-selector]");
const provider = (label: string) =>
  [...document.querySelectorAll<HTMLElement>("[data-slot=model-selector-provider]")].find((row) => row.textContent?.includes(label));
const sub = () => document.querySelector<HTMLElement>("[data-radix-menu-content][data-side]:not([data-slot=model-selector])");

/** Opens the picker, then `label`'s list; answers that list's rows. */
async function openList(label: string): Promise<HTMLElement[]> {
  const opener = await until("the picker's trigger", trigger);
  opener.focus();
  key(opener, "Enter");
  await until("the picker", menu);
  const row = await until(`the ${label} row`, () => provider(label));
  row.focus();
  key(row, "ArrowRight");
  const list = await until(`the ${label} list`, sub);
  return [...list.querySelectorAll<HTMLElement>("[role=menuitem], [role=menuitemradio]")];
}

async function closeMenus() {
  key(menu() ?? document.body, "Escape");
  await until("the picker closed", () => menu() === null);
}

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    const claude = await openList("Claude Code");
    seen.claude = claude.map((row) => row.textContent);
    await closeMenus();

    const codex = await openList("Codex");
    seen.codex = codex.map((row) => [row.dataset.slot, row.textContent]);
    codex[0]!.click();
    await until("the move asked for", () => asked.length === 1);
    await until("the picker closed", () => menu() === null);

    const again = await openList("Codex");
    again.find((row) => row.textContent?.includes("GPT-5.5"))!.click();
    await until("the move on the picked model", () => asked.length === 2);
    seen.asked = asked;
  } catch (error) {
    seen.error = String(error);
  }
  const out = document.createElement("pre");
  out.id = "composer-continue-on-result";
  out.textContent = JSON.stringify(seen);
  document.body.append(out);
}

select({ type: "conversation", id });
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
      <main className="relative flex min-h-0 w-full flex-1 flex-col">
        <ConversationView key={id} selection={{ type: "conversation", id }} />
      </main>
    </SidebarProvider>
  </TooltipProvider>,
);
void drive();
