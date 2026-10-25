/** Production approval cards with synthetic requests and IPC; no daemon or credentials. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ApprovalAction } from "@/app/conversation/ActionCards";
import { TooltipProvider } from "@/components/overlays";
import type { ApprovalRequest, Request } from "@/ipc/generated";
import { emptyBoard, type ShownApproval, useBoard } from "@/state/board";

const conversationId = "approval-fixture";
const variants: [string, Partial<ApprovalRequest>][] = [
  ["command", { command: "git push origin main", grant: "git push", escalation: true }],
  ["network", { kind: "tool", tool: "SandboxNetworkAccess", command: null, grant: "example.com", input: '{"host":"example.com"}' }],
  ["files", { kind: "fileChange", tool: "Edit", command: null, paths: ["/work/session/src/main.rs"], grant: "/work/session" }],
  ["outside", { kind: "fileChange", tool: "Edit", command: null, paths: ["/elsewhere/file.txt"], grant: null }],
  ["permissions", { kind: "permissions", tool: "permissions", command: null, grant: null }],
  ["tool", { kind: "tool", tool: "Unknown", command: null, grant: null }],
  ["unscoped-command", { command: "echo done > /outside/file", grant: null }],
  ["narrow", { command: "git push origin main", grant: "git push" }],
  ["long", { command: "./a-very-long-program-name run", grant: "a-very-long-program-name-that-must-truncate-without-hiding-the-other-buttons run" }],
  // `terminal_run` below Full access, or in a terminal of the user's (Q90): each command asks.
  ["terminal", { tool: "terminal_run", command: "npm run dev", cwd: "/work/session", reason: "Run this in a terminal tab of its own? It runs outside the sandbox.", escalation: true, grant: null }],
  // `close_tab` on a tab of the user's, one with unsaved edits or a terminal (Q90): asks first.
  ["close-tab", { kind: "tool", tool: "close_tab", command: null, reason: "Close the tab “npm run dev”? It's a tab you opened. A command is running in it: closing stops it.", grant: null }],
];
const approvals = Object.fromEntries(variants.map(([id, overrides]): [string, ShownApproval] => [id, {
  id, conversationId, taskId: null, requestId: null, position: 0, state: { type: "pending" }, createdAtMs: 0, resolvedAtMs: null,
  subject: { type: "cli", request: {
    id, kind: "command", tool: "Bash", command: null, cwd: null, paths: [], reason: null,
    escalation: false, input: null, grant: null, ...overrides,
  } },
}]));
// A message to another session waiting for the user's OK: Send or Don't send.
approvals["peer"] = {
  id: "peer", conversationId, taskId: null, requestId: null, position: 0, state: { type: "pending" }, createdAtMs: 0, resolvedAtMs: null,
  subject: { type: "peerMessage", messageId: "m-1", to: "c-2", title: "Math helper", project: "Alpha", text: "What is 9 + 10?" },
};
// An agent's browser action that asks first (Q67): what it does, never a secret it types.
approvals["browser"] = {
  id: "browser", conversationId, taskId: null, requestId: null, position: 0, state: { type: "pending" }, createdAtMs: 0, resolvedAtMs: null,
  subject: {
    type: "browser", tab: "browser:agent-1", url: "https://shop.example/checkout", generation: 3, sensitive: "pay",
    question: "Pay on shop.example?", details: "Clicks button “Pay $5”.\nOn https://shop.example/checkout", fingerprint: "click\u001fe4",
  },
};
// The plan the CLI proposed from its own plan mode (harness D7): Build it, or reply; no Deny.
const plan = (id: string, text: string): ShownApproval => ({
  id, conversationId, taskId: null, requestId: null, position: 0, state: { type: "pending" }, createdAtMs: 0, resolvedAtMs: null,
  subject: { type: "plan", plan: text },
});
approvals["plan"] = plan("plan", "# Add a.txt\n\n1. Create `a.txt` holding `hello`.\n2. Check it reads back.");
approvals["plan-bare"] = plan("plan-bare", "## Plan\n\nWrite the file.");
const replied: string[] = [];
const shown = [...variants.map(([id]) => id), "peer", "browser", "plan", "plan-bare"];
useBoard.setState({ board: { ...emptyBoard(conversationId), loaded: true, approvals } });
const calls: { id: string; decision: string }[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "answerCard") calls.push({ id: req.cardId, decision: req.decision.type });
  return { method: req.method };
});

function Fixture() {
  const [only, setOnly] = useState<string | null>(null);
  useEffect(() => {
    if (!new URLSearchParams(location.search).has("drive")) return;
    const seen: Record<string, unknown> = {};
    const timers: ReturnType<typeof setTimeout>[] = [];
    const at = (ms: number, action: () => void) => timers.push(setTimeout(action, ms));
    const card = (id: string) => document.querySelector(`[data-fixture="${id}"]`)!;
    const buttons = (id: string) => [...card(id).querySelectorAll<HTMLButtonElement>("button")];
    const session = (id: string) => buttons(id).find((button) => button.textContent?.includes("for this session"));
    at(1000, () => {
      seen.buttons = Object.fromEntries(shown.map((id) => [id, buttons(id).map((button) => button.textContent?.replaceAll("\u00a0", " "))]));
      seen.narrow = buttons("narrow").map((button) => {
        const rect = button.getBoundingClientRect();
        return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
      });
      // The network card names the host it asks to reach.
      seen.network = card("network").querySelector("[role=alert]")?.textContent ?? null;
      seen.networkBody = card("network").querySelector("p")?.textContent ?? null;
      seen.overflow = variants.filter(([id]) => card(id).scrollWidth > card(id).clientWidth).map(([id]) => id);
      session("command")?.click();
    });
    at(1200, () => setOnly("command"));
    at(1300, () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true })));
    at(1600, () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true })));
    at(1800, () => setOnly(null));
    at(1900, () => session("network")?.click());
    at(2200, () => session("files")?.click());
    // The message to another session took focus on mount: a window-wide Enter sends nothing, and
    // Enter on the focused card sends it. Its ⏎ shows only while it has focus.
    const peer = () => card("peer").querySelector<HTMLElement>("[data-slot=action-card]")!;
    const hint = () => getComputedStyle([...peer().querySelectorAll("kbd")].at(-1)!.parentElement!).display;
    at(2300, () => setOnly("peer"));
    at(2400, () => {
      seen.peerFocused = document.activeElement === peer();
      seen.peerHint = hint();
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    });
    at(2500, () => {
      peer().blur();
      seen.peerHintBlurred = hint();
      peer().focus();
      peer().dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    });
    at(2600, () => setOnly(null));
    at(2700, () => {
      seen.browser = {
        title: card("browser").querySelector("[role=alert]")?.textContent ?? null,
        body: card("browser").querySelector(".whitespace-pre-wrap")?.textContent ?? null,
      };
      buttons("browser").find((button) => button.textContent === "Go ahead")?.click();
    });
    // A plan: its title from its own heading, and Esc refuses nothing; Build it allows it.
    at(2800, () => setOnly("plan"));
    at(2900, () => {
      seen.plan = {
        title: card("plan").querySelector("[role=alert]")?.textContent ?? null,
        body: card("plan").querySelector("[data-slot=plan-text] ol")?.textContent?.trim() ?? null,
        bare: null,
      };
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
      card("plan").querySelector<HTMLElement>("[data-slot=action-card]")?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    });
    at(3000, () => setOnly(null));
    at(3100, () => {
      (seen.plan as Record<string, unknown>).bare = card("plan-bare").querySelector("[role=alert]")?.textContent ?? null;
      buttons("plan").find((button) => button.textContent === "Reply…")?.click();
      buttons("plan").find((button) => button.textContent === "Build it")?.click();
    });
    at(3300, () => {
      seen.replied = replied;
      seen.calls = calls;
      const result = document.createElement("pre");
      result.id = "approvals-result";
      result.textContent = JSON.stringify(seen);
      document.body.append(result);
    });
    return () => timers.forEach(clearTimeout);
  }, []);
  return (
    <main className="bg-background text-foreground min-h-screen p-8">
      <h1 className="mb-6 text-xl font-semibold">Session approval scopes</h1>
      <div className="flex flex-wrap items-start gap-6">
        {shown.filter((id) => only === null || id === only).map((id) => (
          <div key={id} data-fixture={id} style={{ width: id === "narrow" || id === "long" ? 320 : 680 }} className="bg-composer rounded-xl">
            <ApprovalAction id={id} {...(id.startsWith("plan") ? { onReply: () => void replied.push(id) } : {})} />
          </div>
        ))}
      </div>
    </main>
  );
}
createRoot(document.getElementById("root")!).render(<TooltipProvider><Fixture /></TooltipProvider>);
