/** Production approval cards with synthetic requests and IPC; no daemon or credentials. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ApprovalAction } from "@/app/conversation/ActionCards";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { ApprovalRequest, Request } from "@/ipc/generated";
import { emptyBoard, type ShownApproval, useBoard } from "@/state/board";

const conversationId = "approval-fixture";
const variants: [string, Partial<ApprovalRequest>][] = [
  ["command", { command: "git push origin main", grant: "git push", escalation: true }],
  ["network", { kind: "tool", tool: "SandboxNetworkAccess", command: null, grant: "example.com" }],
  ["files", { kind: "fileChange", tool: "Edit", command: null, paths: ["/work/session/src/main.rs"], grant: "/work/session" }],
  ["outside", { kind: "fileChange", tool: "Edit", command: null, paths: ["/elsewhere/file.txt"], grant: null }],
  ["permissions", { kind: "permissions", tool: "permissions", command: null, grant: null }],
  ["tool", { kind: "tool", tool: "Unknown", command: null, grant: null }],
  ["unscoped-command", { command: "echo done > /outside/file", grant: null }],
  ["narrow", { command: "git push origin main", grant: "git push" }],
  ["long", { command: "./a-very-long-program-name run", grant: "a-very-long-program-name-that-must-truncate-without-hiding-the-other-buttons run" }],
];
const approvals = Object.fromEntries(variants.map(([id, overrides]): [string, ShownApproval] => [id, {
  id, conversationId, taskId: null, requestId: null, position: 0, state: { type: "pending" }, createdAtMs: 0, resolvedAtMs: null,
  subject: { type: "cli", request: {
    id, kind: "command", tool: "Bash", command: null, cwd: null, paths: [], reason: null,
    escalation: false, input: null, grant: null, ...overrides,
  } },
}]));
useBoard.setState({ board: { ...emptyBoard(conversationId), loaded: true, approvals } });
const calls: { id: string; decision: string }[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "answerCard") calls.push({ id: req.cardId, decision: req.decision.type });
  return { method: req.method };
});

function Fixture() {
  const [keyboardOnly, setKeyboardOnly] = useState(false);
  useEffect(() => {
    if (!new URLSearchParams(location.search).has("drive")) return;
    const seen: Record<string, unknown> = {};
    const timers: ReturnType<typeof setTimeout>[] = [];
    const at = (ms: number, action: () => void) => timers.push(setTimeout(action, ms));
    const card = (id: string) => document.querySelector(`[data-fixture="${id}"]`)!;
    const buttons = (id: string) => [...card(id).querySelectorAll<HTMLButtonElement>("button")];
    const session = (id: string) => buttons(id).find((button) => button.textContent?.includes("for this session"));
    at(1000, () => {
      seen.buttons = Object.fromEntries(variants.map(([id]) => [id, buttons(id).map((button) => button.textContent?.replaceAll("\u00a0", " "))]));
      seen.narrow = buttons("narrow").map((button) => {
        const rect = button.getBoundingClientRect();
        return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
      });
      seen.overflow = variants.filter(([id]) => card(id).scrollWidth > card(id).clientWidth).map(([id]) => id);
      session("command")?.click();
    });
    at(1200, () => setKeyboardOnly(true));
    at(1300, () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true })));
    at(1600, () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true })));
    at(1800, () => setKeyboardOnly(false));
    at(1900, () => session("network")?.click());
    at(2200, () => session("files")?.click());
    at(2500, () => {
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
        {variants.filter(([id]) => !keyboardOnly || id === "command").map(([id]) => (
          <div key={id} data-fixture={id} style={{ width: id === "narrow" || id === "long" ? 320 : 680 }} className="bg-composer rounded-xl">
            <ApprovalAction id={id} />
          </div>
        ))}
      </div>
    </main>
  );
}
createRoot(document.getElementById("root")!).render(<TooltipProvider><Fixture /></TooltipProvider>);
