/**
 * A running subagent's panel while a command of its waits on the user's approval (#668): the
 * session's pending approval names the subagent as its asker (`ApprovalRequest.agent`). What the
 * panel says while the approval waits, and once it is allowed, goes into `#subagent-approval-result`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { WorkerThread } from "@/app/side-panel/WorkerThread";
import { TooltipProvider } from "@/components/overlays";
import type { FileEntry, Request, ThreadView, WorkerRow } from "@/ipc/generated";
import { emptyBoard, type ShownApproval, useBoard } from "@/state/board";
import { useCliThread } from "@/state/cliThread";
import type { NativeWorker } from "@/state/nativeWorkers";
import { useApp } from "@/state/store";

const id = "subagent-approval-fixture";
const command = "rm -rf build && pnpm build";
const entries: FileEntry[] = [
  { event: { type: "message", itemId: "b", role: "user", text: "Rebuild the app." }, atMs: 1000, parent: "call1" },
  { event: { type: "message", itemId: "s1", role: "assistant", text: "Cleaning the build first." }, atMs: 1100, parent: "call1" },
  {
    event: { type: "toolCall", itemId: "bash1", name: "Bash", input: JSON.stringify({ command }), status: "inProgress", output: null },
    atMs: 1200, parent: "call1",
  },
] as FileEntry[];
const thread = { entries, turns: [] } as unknown as ThreadView;
useCliThread.setState({ views: { [id]: thread } });
mockIPC((cmd, args) => {
  if (cmd !== "ipc_request") return null;
  const req = (args as { request: Request }).request;
  return req.method === "threadView" ? { method: req.method, view: thread } : { method: req.method };
});
useApp.setState({ info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 } });

const approval = (state: ShownApproval["state"]): ShownApproval => ({
  id: "approval-1", conversationId: id, taskId: null, requestId: "r1", position: 0, createdAtMs: 1200, resolvedAtMs: null, state,
  subject: {
    type: "cli",
    request: { id: "perm-1", kind: "command", tool: "Bash", command, cwd: null, paths: [], reason: null, escalation: false, input: null, grant: null, agent: "call1" },
  },
});
const setApproval = (state: ShownApproval["state"]) =>
  useBoard.setState({ board: { ...emptyBoard(id), loaded: true, approvals: { "approval-1": approval(state) } } });
setApproval({ type: "pending" });

const row = {
  id: `native:${id}:call1`, number: 1, title: "Rebuild the app", agent: "claude", model: null, startedAtMs: 1000,
  working: true, lastEnd: null, gone: false, lastEventAtMs: Date.now(), limit: null, freshContext: false, waiting: [], mcp: [],
  branch: null, base: null, diff: null, ahead: 0, merged: false,
  native: { conversationId: id, owner: id, callId: "call1", backgroundId: null, prompt: "Rebuild the app." },
} as WorkerRow as NativeWorker;

createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <div className="bg-background text-foreground w-96"><WorkerThread conversationId={id} row={row} /></div>
  </TooltipProvider>,
);

function until(check: () => boolean, timeoutMs = 6000): Promise<void> {
  const start = performance.now();
  return new Promise((resolve, reject) => {
    const tick = () => (check() ? resolve() : performance.now() - start > timeoutMs ? reject(new Error("timed out")) : setTimeout(tick, 20));
    tick();
  });
}
const panel = () => document.querySelector<HTMLElement>('[data-slot="worker-thread"]');
const text = () => panel()?.textContent ?? "";

const seen: Record<string, unknown> = {};
try {
  await until(() => text().includes("Cleaning the build first."));
  // Its live words settle (a live label stands a second before the next).
  await new Promise((resolve) => setTimeout(resolve, 2500));
  seen.waiting = { awaiting: text().includes("Awaiting approval"), running: /Running/.test(text()) };
  setApproval({ type: "allowed", by: "user", similar: false });
  await new Promise((resolve) => setTimeout(resolve, 2500));
  seen.allowed = { awaiting: text().includes("Awaiting approval"), running: /Running/.test(text()) };
} catch (error) {
  seen.error = String(error);
  seen.text = text().slice(0, 600);
}
const result = document.createElement("pre");
result.id = "subagent-approval-result";
result.textContent = JSON.stringify(seen);
document.body.append(result);
