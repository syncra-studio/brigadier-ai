/** Standalone rendering-test entry with synthetic board data; no daemon or real app data. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { BackgroundWorkers } from "@/app/conversation/BackgroundWorkers";
import { TaskRow } from "@/app/conversation/TaskRow";
import { TooltipProvider } from "@/components/ui/tooltip";
import night from "@/fixtures/boards/overnight-2026-10-03.json";
import type { Gate, Task } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";

const base = Object.values(night.tasks)[0] as unknown as Task;
const conversationId = "worker-activity-fixture";
const task = (id: string, state: Task["state"], patch: Partial<Task> = {}): Task => ({
  ...base,
  id,
  title: id,
  conversationId,
  number: 1,
  kind: "implement",
  state,
  createdAtMs: 8000,
  updatedAtMs: 8000,
  attempts: [],
  run: null,
  requestId: null,
  gate: null,
  gateLink: null,
  candidate: null,
  report: null,
  quotaWait: null,
  blockedReason: null,
  fixRounds: 0,
  ...patch,
});
const gate: Gate = {
  round: 1, commit: null, outcome: null, relanding: false, retry: false, overridden: false, findings: [],
  members: ["review", "verify", "review"].map((role, index) => ({
    taskId: `checker-${index}`, role: role as "review" | "verify", result: null, avoid: [],
  })),
};
const tasks = {
  active: task("active", "running"),
  blocked: task("blocked", "blocked", { blockedReason: "Waiting for a free worker" }),
  quota: task("quota", "paused", {
    quotaWait: {
      reason: "Codex weekly quota resets tomorrow", sinceMs: 160000, resetsAtMs: null, rule: null, ranking: null,
    },
  }),
  queued: task("queued", "queued", { blockedReason: "waiting for step 1" }),
  gate: task("gate", "reviewing", { gate }),
  completed: task("completed", "done"),
  ...Object.fromEntries(gate.members.map((member) => [member.taskId, task(member.taskId, "running", {
    kind: member.role === "review" ? "review" : "verify",
    gateLink: { owner: { type: "task", taskId: "gate" }, round: 1, role: member.role },
    route: { ...base.route, choice: { ...base.route.choice, provider: "codex" } },
  })])),
};
useBoard.setState({
  board: {
    ...emptyBoard(conversationId), loaded: true, tasks,
    activity: {
      active: "Editing apps/desktop/src/composer/Paste.tsx",
      blocked: "Writing…", quota: "Writing…", queued: "Writing…",
      "checker-0": "Thinking…", "checker-1": "$ cargo test -p core", "checker-2": "Thinking…",
    },
    diffs: { active: { files: [], insertions: 209, deletions: 102 } },
  },
});
mockIPC(() => null);

function snapshot(element: Element) {
  return {
    text: element.textContent,
    state: element.querySelector("[aria-live]")?.textContent ?? "",
    subrows: element.querySelectorAll('[data-slot="task-activity"]').length,
    lines: [...element.querySelectorAll('[data-slot="task-activity"] > div')].map((line) => line.textContent),
  };
}

function WorkerActivityFixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    document.querySelector<HTMLButtonElement>('[data-slot="background-workers"] button[aria-expanded="false"]')
      ?.click();
    const timer = setTimeout(() => {
      setResult(JSON.stringify({
        thread: Object.fromEntries([...document.querySelectorAll("[data-task]")].map((element) => [
          element.getAttribute("data-task"), snapshot(element),
        ])),
        strip: Object.fromEntries([...document.querySelectorAll('[data-slot="worker-strip-row"]')].map((element) => [
          element.querySelector("button")?.textContent, snapshot(element),
        ])),
      }));
    }, 200);
    return () => clearTimeout(timer);
  }, []);
  return (
    <TooltipProvider>
      <main className="mx-auto flex max-w-2xl flex-col gap-4 p-6">
        {["active", "blocked", "quota", "queued", "gate", "completed"].map((id) => (
          <section key={id} data-task={id}><TaskRow taskId={id} /></section>
        ))}
        <BackgroundWorkers conversationId={conversationId} />
        <pre id="worker-activity-result">{result}</pre>
      </main>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<WorkerActivityFixture />);
