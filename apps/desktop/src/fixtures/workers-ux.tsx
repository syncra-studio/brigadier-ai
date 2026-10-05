/** Production chat/panel fixture with stored worker events. No live workers or daemon. */
import { leadTranscript } from "@/fixtures/flow";
import type { Message, ProviderEvent, RawEntry, Report, Task } from "@/ipc/generated";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const done = query.get("view") === "done";
const now = Date.now();
const id = "flow-session";
const board = useBoard.getState().board!;
const base = Object.values(board.tasks)[0]!;
const report: Report = {
  summary: "Completed a read-only inspection of all **43 files** in the project. Read every file completely and verified the Git objects. No inaccessible files or symlinks were found. Final Git status is clean on `main`.\n\nThe project is a collection of independent Python functions, a JavaScript assignment, and a text greeting.\n\n### Project files\n\n| File | Size | Summary |\n| --- | --- | --- |\n| `m1.py` | 30 bytes | Returns 1. |\n| `m2.py` | 30 bytes | Returns 2. |\n| `src/b.js` | 4 bytes | Assigns 1 to x. |",
  changes: ["No source files changed."], decisions: ["Inspected the Git metadata as requested."],
  verification: ["Read all 43 files and checked Git status."], doneWhen: ["[met] Every file has a summary."],
  openQuestions: [], risks: ["There is no test suite in the project."], needsUser: ["Choose a license before publishing the project."],
  verdict: null, checks: null, artifacts: [], submittedAtMs: now - 60000,
};
function worker(taskId: string, number: number, title: string): Task {
  return { ...base, id: taskId, number, title, conversationId: id, state: done ? "done" : "running",
    kind: "scout", role: null, phase: null, run: null, gateLink: null, plan: null, subject: null,
    requestId: "r2", position: 12 + number, spec: "Inspect every project file, including Git metadata. Read only; do not change any files. Summarize the contents and verify Git status.",
    createdAtMs: now - (number === 1 ? 75000 : 64000), updatedAtMs: now - 60000,
    route: { ...base.route, choice: { provider: "codex", model: "gpt-6.1-sol", effort: "high" } },
    attempts: [], workspace: null, candidate: null, kept: null, error: null, blockedReason: null,
    quotaWait: null, report: done ? report : null, outputs: done ? [{ id: "summary-file", title: "Project file summary", kind: "file", mime: "text/markdown", bytes: 2048, fileName: "summary.md" }] : [],
  };
}
const tasks = { t1: worker("t1", 1, "File summary"), t2: worker("t2", 2, "Readme draft") };
if (query.get("waiting") === "1") { tasks.t2.state = "blocked"; tasks.t2.blockedReason = "Waiting for plan review"; }
useApp.setState((s) => ({ providers: { ...s.providers, view: { providers: [{ provider: "codex", status: null, quota: null, usage: null, error: null, checkedAtMs: now,
  models: { provider: "codex", fetchedAtMs: now, cliVersion: "fixture", models: [{ id: "gpt-6.1-sol", displayName: "GPT-6.1 Sol", description: "", resolved: null, efforts: ["high"], defaultEffort: "high", isDefault: true, inputModalities: ["text"], fast: null, legacy: false }] } }], sessions: [], fixtures: [] } } }));
const events: ProviderEvent[] = done ? [
  ...["I’ll inspect the project files and its Git metadata.", "I’ll read the Python functions first.", "Next I’ll inspect the JavaScript and text files.", "I’ll include the Git metadata in the inventory.", "The inspection remains read only."].map((text, index) => ({ type: "message" as const, itemId: `early-${index}`, role: "assistant" as const, text })),
  ...["ls -la", "cat m1.py", "cat m2.py", "git status", "git log --oneline"].map((command, index) => ({ type: "command" as const, itemId: `cmd-${index}`, command, cwd: null, status: "completed" as const, exitCode: 0, output: "Clean", durationMs: null })),
  { type: "toolCall", itemId: "report", name: "mcp__brigadier__submit_report", input: "{}", status: "completed", output: null },
  { type: "command", itemId: "recent", command: "python3 -B - <<'PY'\nfrom pathlib import Path\nprint(list(Path('.').rglob('*')))\nPY", cwd: null, status: "completed", exitCode: 0, output: "43 files", durationMs: null },
] : [{ type: "command", itemId: "listing", command: "ls .", cwd: null, status: "inProgress", exitCode: null, output: null, durationMs: null }];
const entries: RawEntry[] = events.map((event, index) => ({ streamSeq: index + 1, atMs: now - 60000 + index * 100, event }));
leadTranscript.splice(0, leadTranscript.length, ...entries);
useBoard.setState({ board: { ...board, tasks, plans: {}, approvals: {}, waiting: {}, questions: {}, decisions: [], thinking: [], machineSteps: [], orchestratorSteps: [],
  workerSteps: [
    { taskId: "t1", requestId: "r2", kind: "started", position: 13, atMs: tasks.t1.createdAtMs },
    { taskId: "t2", requestId: "r2", kind: "started", position: 14, atMs: tasks.t2.createdAtMs },
    ...(done ? [
      { taskId: "t2", requestId: "r2", kind: "finished" as const, position: 18, atMs: now - 75000 },
      { taskId: "t1", requestId: "r2", kind: "finished" as const, position: 22, atMs: now - 60000 },
    ] : []),
  ],
  transcripts: { t1: { entries, loading: false, hasMore: false }, t2: { entries, loading: false, hasMore: false } },
  activity: {}, summaries: {}, requests: { r2: { ...board.requests.r2!, startedAtMs: now - 259000, endedAtMs: done ? now : null, state: done ? { type: "done" } : { type: "working" } } },
  run: done ? "idle" : "running", runRequest: done ? null : "r2",
} });
const message = (seq: number, role: Message["role"], text: string): Message => ({
  ...useApp.getState().threads[id]!.items[0]!, id: `fixture-${seq}`, seq, role, text,
  requestId: role === "user" ? null : "r2", createdAtMs: now - 80000 + seq * 500, parentId: null,
});
useApp.setState((s) => ({ threads: { ...s.threads, [id]: { ...s.threads[id]!, items: [
  { ...message(10, "user", "Read every project file and draft a README. Use two workers in parallel."), id: "r2" },
  message(11, "assistant", "I’ll run both workers in parallel, then summarize their results."),
  ...(done ? [message(19, "assistant", "The README draft is ready."), message(23, "assistant", "Both workers have completed their inspections."),
    message(30, "assistant", "The files have been summarized and the README draft is ready. Both workers verified the project without changing any source files.")] : []),
] } }, conversations: { ...s.conversations, [id]: { ...s.conversations[id]!, title: "Summarize probe project" } } }));
