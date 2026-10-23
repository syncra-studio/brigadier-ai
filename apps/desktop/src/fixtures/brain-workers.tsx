/**
 * The Inspector's Brain tab, opened in Settings from a session with no Brain picked before, over
 * the IPC mock. No daemon.
 *
 * ?from=session (the default) | chat | other: Settings opened from a session of the project, a
 * chat, or a session of another project. ?pick=<project>: a Brain picked before (calc or
 * other).
 * ?drive=1 records what the tab shows, sends task updates, reads the tasks again twice as the
 * bridge's resync does (first bringing back an update the feed missed, at the same moment as one
 * it delivered; then with an update arriving during the read; then two overlapping reads, the
 * older finishing last), and writes what it saw to
 * #brain-workers-result (BrainWorkersRender.test.ts reads it).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { BrainTab } from "@/app/inspector/brain/BrainTab";
import { TooltipProvider } from "@/components/overlays";
import type { BrainOverview, BrainUse, Conversation, EventEnvelope, Project, Request, Task } from "@/ipc/generated";
import { openSettings } from "@/state/actions";
import { applyBrainEvents, reloadSessionTasks, selectBrain } from "@/state/brain";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
const from = params.get("from") ?? "session";

function project(id: string, name: string): Project {
  return { id, name, createdAtMs: 0, repos: [{ path: `/tmp/${name}`, name }], trust: [] } as unknown as Project;
}

function conversation(id: string, projectId: string | null): Conversation {
  return { id, kind: projectId ? "session" : "chat", projectId, title: id, pinnedAtMs: null, createdAtMs: 0,
    updatedAtMs: 0, lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
    setup: null } as unknown as Conversation;
}

function task(id: string, number: number, title: string, brain: BrainUse | undefined, updatedAtMs: number): Task {
  return { id, conversationId: "session", number, title, brain, createdAtMs: 0, updatedAtMs } as unknown as Task;
}

const overview: BrainOverview = {
  projectId: "calc",
  stats: { nodes: 10, edges: 2, stale: 0, updatedMs: null, unembedded: 0, kinds: [], transcriptEntries: 0, bytesOnDisk: 1024 },
  index: null,
  embedder: { model: "fixture-model", modelBytes: 0, dimensions: 512, state: { type: "loaded" } },
};

useApp.setState({
  projects: { calc: project("calc", "calc"), other: project("other", "other") },
  conversations: {
    session: conversation("session", "calc"),
    chat: conversation("chat", null),
    elsewhere: conversation("elsewhere", "other"),
  },
  selection: { type: "conversation", id: from === "chat" ? "chat" : from === "other" ? "elsewhere" : "session" },
});
const pick = params.get("pick");
if (pick) selectBrain(pick);
// Opening Settings remembers the session it was opened from (no Brain picked before).
openSettings("inspector");

const calls: string[] = [];
const send = (tasks: Task[]) =>
  applyBrainEvents(
    tasks.map(
      (t, i) => ({ seq: 100 + i, stream: `conversation:${t.conversationId}`, streamSeq: i, atMs: 0, event: { type: "taskUpdated", task: t } }) as EventEnvelope,
    ),
  );
let reads = 0;
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getBrain":
      calls.push(`getBrain ${req.projectId}`);
      return { method: req.method, overview: { ...overview, projectId: req.projectId } };
    case "getConversation": {
      calls.push(`getConversation ${req.id}`);
      const read = reads++;
      // The first read; then one after the feed missed an update at 20, the moment of the one it
      // delivered; then one taken before an update at 30 that arrives while it is read.
      const t1 =
        read === 0
          ? task("t1", 1, "Add mul", { queries: 3, factsIn: 2, factsOut: 1, calls: 40 }, 10)
          : read === 1
            ? task("t1", 1, "Add mul", { queries: 5, factsIn: 2, factsOut: 2, calls: 50 }, 20)
            : read === 2
              ? task("t1", 1, "Add mul", { queries: 5, factsIn: 2, factsOut: 2, calls: 50 }, 30)
              : read === 3
                ? task("t1", 1, "Add mul", { queries: 7, factsIn: 2, factsOut: 3, calls: 60 }, 40)
                : task("t1", 1, "Add mul", { queries: 8, factsIn: 2, factsOut: 3, calls: 65 }, 50);
      if (read === 2) send([task("t1", 1, "Add mul", { queries: 6, factsIn: 2, factsOut: 3, calls: 55 }, 30)]);
      const response = {
        method: req.method,
        view: {
          tasks: [
            t1,
            task("t2", 2, "Long refactor", { queries: 0, factsIn: 0, factsOut: 0, calls: 80 }, 10),
            task("t3", 3, "Before counters", undefined, 10),
          ],
        },
      };
      // The fourth read is overtaken by the fifth: it answers last.
      return read === 3 ? new Promise((resolve) => setTimeout(() => resolve(response), 300)) : response;
    }
    default:
      return { method: req.method };
  }
});

const picker = () => document.querySelector('button[aria-label="Brain"]')?.textContent?.trim();
const section = () =>
  [...document.querySelectorAll("section")].find((s) => s.querySelector("h2")?.textContent === "Workers in this session");
const rows = () =>
  [...(section()?.querySelectorAll("li") ?? [])].map((li) => ({
    name: li.querySelector("span")?.textContent,
    low: !!li.querySelector('[data-slot="badge"]'),
    use: li.querySelector("p")?.textContent,
  }));

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    const at = (ms: number, run: () => void) => setTimeout(run, ms);
    const timers = [
      at(600, () => {
        seen.first = { picker: picker(), rows: rows() };
        const updated = task("t1", 1, "Add mul", { queries: 4, factsIn: 2, factsOut: 2, calls: 45 }, 20);
        const stale = task("t2", 2, "Long refactor", { queries: 9, factsIn: 9, factsOut: 9, calls: 9 }, 5);
        const elsewhere = { ...task("t9", 9, "Other session", { queries: 1, factsIn: 0, factsOut: 0, calls: 1 }, 20), conversationId: "chat" };
        send([updated, stale, elsewhere]);
      }),
      at(900, () => {
        seen.updated = rows();
        void reloadSessionTasks();
      }),
      at(1300, () => {
        seen.reloaded = rows();
        void reloadSessionTasks();
      }),
      at(1700, () => {
        seen.raced = rows();
        void reloadSessionTasks();
        setTimeout(() => void reloadSessionTasks(), 50);
      }),
      at(2300, () => {
        seen.overtaken = rows();
        seen.calls = calls;
        setResult(JSON.stringify(seen));
      }),
    ];
    return () => timers.forEach(clearTimeout);
  }, []);
  return (
    <TooltipProvider>
      <main className="bg-background text-foreground flex h-screen flex-col overflow-hidden">
        <BrainTab />
      </main>
      {result && <pre id="brain-workers-result">{result}</pre>}
    </TooltipProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Fixture />);
