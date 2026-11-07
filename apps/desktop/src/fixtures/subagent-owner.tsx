/**
 * A worker's own subagent, its panel open (its Details page too, where the worker's thread
 * isn't drawn): the subagent is found in that worker's thread, so the thread stays through a
 * resync, and one not held when the panel opens is read (#760).
 */
import { mockIPC } from "@/ipc/nativeMocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import type { FileEntry, Request, ThreadView } from "@/ipc/generated";
import { resyncThreadViews, useSubagentRow, useWorkerThreads } from "@/state/cliThread";
import { subagentPanel } from "@/state/nativeWorkers";

const session = "subagent-owner-fixture";
const call = (itemId: string, description: string): FileEntry => ({
  event: { type: "toolCall", itemId, name: "Agent", input: JSON.stringify({ description, prompt: "Look" }), status: "inProgress", output: null },
  atMs: 1000,
} as FileEntry);
const thread = (entries: FileEntry[]): ThreadView => ({ entries, turns: [] }) as unknown as ThreadView;
/** What the daemon answers for each worker's thread. */
const threads: Record<string, ThreadView> = { held: thread([call("c1", "Count the lines")]), unread: thread([call("c2", "Read the parser")]) };
useWorkerThreads.setState({ views: { held: threads["held"]! } });
mockIPC((command, args) => {
  if (command !== "ipc_request") return null;
  const req = (args as { request: Request }).request;
  if (req.method === "workerThread") return { method: req.method, view: threads[req.taskId] };
  return req.method === "threadView" ? { method: req.method, view: thread([]) } : { method: req.method };
});

function until(check: () => boolean, timeoutMs = 6000): Promise<void> {
  const start = performance.now();
  return new Promise((resolve, reject) => {
    const tick = () => (check() ? resolve() : performance.now() - start > timeoutMs ? reject(new Error("timed out")) : setTimeout(tick, 20));
    tick();
  });
}
const shows = (text: string) => document.body.textContent?.includes(text) ?? false;

/** What the Workers tab finds for the panel open, its Details page or not. */
function Panel({ panel }: { panel: string }) {
  const row = useSubagentRow(session, panel);
  return <p>{row ? row.title : "No subagent"}</p>;
}

function Fixture() {
  const [panel, setPanel] = useState(subagentPanel("held", "c1"));
  const [result, setResult] = useState("");
  useEffect(() => {
    const seen: Record<string, unknown> = {};
    void (async () => {
      await until(() => shows("Count the lines"));
      // A reconnect drops the workers' threads not on screen: its panel's worker stays.
      await resyncThreadViews([]);
      await new Promise((resolve) => setTimeout(resolve, 100));
      seen.keptThroughResync = shows("Count the lines") && !!useWorkerThreads.getState().views["held"];
      // A panel whose worker's thread isn't held reads it.
      setPanel(subagentPanel("unread", "c2"));
      await until(() => shows("Read the parser"));
      seen.readWhenMissing = true;
      setResult(JSON.stringify(seen));
    })().catch((error: unknown) => setResult(JSON.stringify({ error: String(error), seen, body: document.body.textContent?.slice(0, 1000) })));
  }, []);
  return (
    <div>
      <Panel panel={panel} />
      {result && <pre id="subagent-owner-result">{result}</pre>}
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
