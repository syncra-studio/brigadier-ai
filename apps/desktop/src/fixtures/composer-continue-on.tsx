/**
 * A standalone Vite entry, excluded from the app entry and production build: a started session's
 * composer on Claude Code, with Codex installed too, IPC mocked (S38c). Browsing Codex in the
 * model picker says a pick continues there, and picking a Codex model asks the daemon to move the
 * session there on that model; Claude Code's own list says nothing of the kind. Then the picker's
 * keys: arrows between its columns, typing to search, Escape. What it saw goes as JSON into `<pre id="composer-continue-on-result">`.
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
const picker = () => document.querySelector<HTMLElement>("[data-slot=model-picker-popover]");
const agent = (label: string) =>
  [...document.querySelectorAll<HTMLElement>("[data-slot=model-picker-agent]")].find((row) => row.textContent === label);
const models = () => [...document.querySelectorAll<HTMLElement>("[data-slot=model-picker-model]")];
const line = () => document.querySelector("[data-slot=model-picker-continue]")?.textContent ?? null;
const focused = () => {
  const active = document.activeElement as HTMLElement | null;
  return active?.matches("input") ? `search:${(active as HTMLInputElement).value}` : (active?.textContent ?? null);
};

async function open(): Promise<void> {
  const opener = await until("the picker's trigger", trigger);
  opener.focus();
  key(opener, "Enter");
  await until("the picker", picker);
  await until("the search focused", () => document.activeElement?.matches("[data-slot=model-picker] input"));
}

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    // Its own agent's list has only models, and no line.
    await open();
    seen.claude = { line: line(), models: models().map((row) => row.textContent) };

    // The other agent's list says a pick continues there; a pick moves the session on that model.
    agent("Codex")!.click();
    await until("Codex's models", () => models().some((row) => row.textContent === "GPT-5.5"));
    seen.codex = { line: line(), models: models().map((row) => row.textContent) };
    models()[0]!.click();
    await until("the move asked for", () => asked.length === 1);
    await until("the picker closed", () => picker() === null);
    seen.asked = asked;

    // The keyboard: ↓ from the search to the picked model, ← to its agent, ↓ to the next agent,
    // → into its models, typing searches, Escape clears the search, then closes.
    await open();
    const keys: [string, string | null][] = [];
    const press = async (name: string) => {
      key(document.activeElement!, name);
      await sleep(50);
      keys.push([name, focused()]);
    };
    await press("ArrowDown");
    await press("ArrowLeft");
    await press("ArrowDown");
    seen.browsingLine = line();
    await press("ArrowRight");
    await press("g");
    seen.searched = models().map((row) => row.textContent);
    await press("Escape");
    seen.openAfterEscape = picker() !== null;
    key(document.activeElement!, "Escape");
    await until("the picker closed", () => picker() === null);
    seen.keys = keys;
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
