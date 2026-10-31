/**
 * A running subagent's panel: its transcript comes only from its session's files, so the panel
 * reads them while it works; once it is over, the session's thread (read again at its turn's end)
 * brings its closing words.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { WorkerThread } from "@/app/side-panel/WorkerThread";
import { TooltipProvider } from "@/components/overlays";
import type { FileEntry, Request, ThreadView, WorkerRow } from "@/ipc/generated";
import { useCliThread } from "@/state/cliThread";
import type { NativeWorker } from "@/state/nativeWorkers";
import { useApp } from "@/state/store";

const id = "subagent-panel-fixture";
const said = (itemId: string, text: string): FileEntry => ({ event: { type: "message", itemId, role: "assistant", text }, atMs: 1000, parent: "call1" } as FileEntry);
const brief: FileEntry = { event: { type: "message", itemId: "b", role: "user", text: "Count the lines." }, atMs: 1000, parent: "call1" } as FileEntry;
/** What the session's files hold of the subagent so far. */
const file: FileEntry[] = [brief, said("s1", "Starting the count.")];
const thread = (entries: FileEntry[]): ThreadView => ({ entries, turns: [] }) as unknown as ThreadView;
useCliThread.setState({ views: { [id]: thread([...file]) } });
mockIPC((command, args) => {
  if (command !== "ipc_request") return null;
  const req = (args as { request: Request }).request;
  return req.method === "threadView" ? { method: req.method, view: thread([...file]) } : { method: req.method };
});
useApp.setState({ info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 } });

const row = (working: boolean): NativeWorker => ({
  id: `native:${id}:call1`, number: 1, title: "Count the lines", agent: "claude", model: null, startedAtMs: 1000,
  working, lastEnd: working ? null : "done", gone: false, lastEventAtMs: Date.now(), limit: null, freshContext: false, waiting: [], mcp: [],
  branch: null, base: null, diff: null, ahead: 0, merged: false,
  native: { conversationId: id, callId: "call1", backgroundId: null, prompt: "Count the lines." },
} as WorkerRow as NativeWorker);

function until(check: () => boolean, timeoutMs = 6000): Promise<void> {
  const start = performance.now();
  return new Promise((resolve, reject) => {
    const tick = () => (check() ? resolve() : performance.now() - start > timeoutMs ? reject(new Error("timed out")) : setTimeout(tick, 20));
    tick();
  });
}
const shows = (text: string) => document.body.textContent?.includes(text) ?? false;

function Fixture() {
  const [working, setWorking] = useState(true);
  const [result, setResult] = useState("");
  useEffect(() => {
    const seen: Record<string, unknown> = {};
    void (async () => {
      await until(() => shows("Starting the count."));
      // Its file grows while it works; live news carries none of it.
      file.push(said("s2", "Halfway there."));
      await until(() => shows("Halfway there."));
      seen.whileWorking = true;
      // Its call ends; its closing words come with the session's thread read at the turn's end.
      setWorking(false);
      useCliThread.setState({ views: { [id]: thread([...file, said("s3", "There are 42 lines.")]) } });
      await until(() => shows("There are 42 lines.") && shows("Halfway there."));
      seen.closing = true;
      setResult(JSON.stringify(seen));
    })().catch((error: unknown) => setResult(JSON.stringify({ error: String(error), seen, body: document.body.textContent?.slice(0, 1000) })));
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground w-96"><WorkerThread conversationId={id} row={row(working)} /></div>
      {result && <pre id="subagent-panel-result">{result}</pre>}
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
