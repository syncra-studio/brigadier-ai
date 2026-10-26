/** Production history/output with synthetic daemon responses, including restart/clear evidence. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { HistoryTab } from "@/app/side-panel/HistoryTab";
import { OutputTab } from "@/app/side-panel/OutputTab";
import { TooltipProvider } from "@/components/overlays";
import { SidebarProvider } from "@/components/layout";
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@/components/conversation";
import type { Request } from "@/ipc/generated";
import type { BackgroundHistoryRecord } from "@/state/backgroundHistory";
import { sessionTabs, useSessionTabsOf } from "@/state/sessionTabs";

const id = "history-fixture";
const now = Date.now();
const params = new URLSearchParams(location.search);
const make = (key: string, purpose: string, status: string, patch: Partial<BackgroundHistoryRecord> = {}): BackgroundHistoryRecord => ({
  id: key, conversationId: id, workerId: null, provider: "claude", nativeSessionId: "native-before-restart", generation: "generation-1", taskId: key, callId: key,
  kind: "shell", purpose, command: "cargo test -p brigadier-core", cwd: "/work/session", status, startedAtMs: now - 180_000, endedAtMs: status === "running" ? null : now - 90_000,
  exitCode: null, stopActor: null, summary: null, outputAvailable: true, outputTruncated: false, outputBytes: 36, ...patch,
});
const records = [
  make("running", "Watch the pull request's CI", "running", { command: "gh pr checks 273 --watch", startedAtMs: now - 20_000, workerId: "worker-parser" }),
  make("failed", "Run core tests", "failed", { exitCode: 1, outputTruncated: true, outputBytes: 2097152 }),
  make("stopped", "Wait for the review", "stopped", { command: "gh pr checks 273 --watch", stopActor: "user", summary: "Stopped before the CLI closed" }),
  make("unknown", "Build the desktop app", "unknown", { endedAtMs: null, command: "pnpm build", outputAvailable: false, summary: "Daemon restarted; end was not observed" }),
  make("preview", "Preview cleared from Background", "completed", { kind: "preview", command: "pnpm dev", exitCode: 0 }),
  make("older", "Install dependencies", "completed", { command: "pnpm install", startedAtMs: now - 600_000, endedAtMs: now - 500_000, exitCode: 0 }),
];
let failedOnce = params.has("error");
const calls: string[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  calls.push(req.method);
  if (req.method === "getBackgroundHistory") {
    if (failedOnce) { failedOnce = false; throw new Error("Daemon unavailable"); }
    return { method: req.method, history: { records: params.has("empty") ? [] : req.before ? records.slice(5) : records.slice(0, 5), next: params.has("empty") || req.before ? null : "preview", total: params.has("empty") ? 0 : records.length } };
  }
  if (req.method === "readBackgroundHistoryOutput") {
    const record = records.find((item) => item.id === req.recordId)!;
    return { method: req.method, output: { record, text: record.outputAvailable ? `${record.purpose}\n${record.exitCode === 1 ? "test result: FAILED" : "Recorded output survives restart"}` : null, unavailableReason: record.outputAvailable ? null : "No output snapshot was available" } };
  }
  return { method: req.method };
});

const frames = () => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
async function waitFor(find: () => boolean) { for (const end = Date.now() + 10_000; !find() && Date.now() < end;) await frames(); }
const click = (text: string) => [...document.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent === text)?.click();

function Fixture() {
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  const tabs = useSessionTabsOf(id);
  const [result, setResult] = useState("");
  const panel = tabs.tabs.find((tab) => tab.id === tabs.active);
  useEffect(() => {
    if (!params.has("drive")) return;
    void (async () => {
      await waitFor(() => document.querySelectorAll('[data-slot="timeline"] li').length === 5);
      click("Run core tests");
      await waitFor(() => !!document.querySelector('[data-slot="shell-output"]')?.textContent?.includes("FAILED"));
      const expanded = document.querySelector('[data-slot="background-history"]')?.textContent ?? "";
      click("Build the desktop app");
      await waitFor(() => !!document.body.textContent?.includes("No output snapshot was available"));
      const unknown = document.getElementById("background-unknown")?.textContent ?? "";
      click("Load older activity");
      await waitFor(() => document.querySelectorAll('[data-slot="timeline"] li').length === 6);
      const count = document.querySelectorAll('[data-slot="timeline"] li').length;
      [...(document.getElementById("background-failed")?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find((button) => button.textContent === "Open output in a tab")?.click();
      await waitFor(() => !!document.querySelector('[data-slot="output-tab"]')?.textContent?.includes("FAILED"));
      setResult(JSON.stringify({ count, expanded, unknown, active: sessionTabs(id).active, output: document.querySelector('[data-slot="output-tab"]')?.textContent, calls }));
    })();
  }, []);
  return <AssistantRuntimeProvider runtime={runtime}><main className="bg-background text-foreground flex h-screen w-full min-w-0 flex-1 flex-col">{panel?.kind === "panel" && panel.view.type === "output" ? <OutputTab conversationId={id} view={panel.view} /> : <HistoryTab conversationId={id} />}{result && <pre id="history-result" className="hidden">{result}</pre>}</main></AssistantRuntimeProvider>;
}
createRoot(document.getElementById("root")!).render(<TooltipProvider><SidebarProvider defaultOpen={false}><Fixture /></SidebarProvider></TooltipProvider>);
