/* oxlint-disable brigadier/no-raw-design-values -- Fixed widths exercise user-resized panels. */
/** Production chat/panel fixture with stored worker events. No live workers or daemon. */
import { leadTranscript, workerPatch } from "@/fixtures/flow";
import type { Message, ProviderEvent, RawEntry, Report, Task } from "@/ipc/generated";
import { useBoard } from "@/state/board";
import { openWorker, useOpenWorker } from "@/state/openWorker";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const done = query.get("view") === "done";
const stress = query.has("panel-test");
// The worker view's header at the panel's own width in a 1280-wide window.
const headerTest = query.has("header-test");
// A done worker's diff, opened from its `task-1.diff` chip.
const diffTest = query.has("diff-test");
const previousWidth = localStorage.getItem("brigadier.rightSidebarWidth");
if (stress) {
  localStorage.setItem("brigadier.rightSidebarWidth", "600");
  // React reads the starting width when the fixture mounts; do not alter later pages.
  window.addEventListener("pagehide", () => {
    if (previousWidth === null) localStorage.removeItem("brigadier.rightSidebarWidth");
    else localStorage.setItem("brigadier.rightSidebarWidth", previousWidth);
  }, { once: true });
}
const longPath = `/workspace/${"longdirectoryname".repeat(18)}/worker.tsx`;
const now = Date.now();
const id = "flow-session";
const board = useBoard.getState().board!;
const base = Object.values(board.tasks)[0]!;
const report: Report = {
  summary: "Completed a read-only inspection of all **43 files** in the project. Read every file completely and verified the Git objects. No inaccessible files or symlinks were found. Final Git status is clean on `main`.\n\nThe project is a collection of independent Python functions, a JavaScript assignment, and a text greeting.\n\n### Project files\n\n| File | Size | Summary |\n| --- | --- | --- |\n| `m1.py` | 30 bytes | Returns 1. |\n| `m2.py` | 30 bytes | Returns 2. |\n| `src/b.js` | 4 bytes | Assigns 1 to x. |",
  changes: ["No source files changed."], decisions: ["Inspected the Git metadata as requested."], learned: [],
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
    quotaWait: null, report: done || diffTest ? { ...report, artifacts: diffTest ? [{ id: "worker-diff", title: `Diff of task-${number}`, kind: "diff", mime: "text/x-diff", bytes: workerPatch.length, fileName: `task-${number}.diff` }] : [] } : null, outputs: done ? [{ id: "summary-file", title: "Project file summary", kind: "file", mime: "text/markdown", bytes: 2048, fileName: "summary.md" }] : [],
  };
}
const tasks = { t1: worker("t1", 1, headerTest ? "Add the project file summary" : "File summary"), t2: worker("t2", 2, "Readme draft") };
if (stress) {
  tasks.t1.spec = `Brigadier desktop app (Tauri + React, apps/desktop/src). Fix the worker panel and verify it in the running app. Read ${longPath}. ${"Keep the brief readable at every width. ".repeat(8)}`;
}
if (query.get("waiting") === "1") { tasks.t2.state = "blocked"; tasks.t2.blockedReason = "Waiting for plan review"; }
useApp.setState((s) => ({ providers: { ...s.providers, view: { providers: [{ provider: "codex", status: null, quota: null, usage: null, error: null, checkedAtMs: now,
  models: { provider: "codex", fetchedAtMs: now, cliVersion: "fixture", models: [{ id: "gpt-6.1-sol", displayName: "GPT-6.1 Sol", description: "", resolved: null, efforts: ["high"], defaultEffort: "high", isDefault: true, inputModalities: ["text"], fast: null, legacy: false }] } }], sessions: [], fixtures: [] } } }));
const events: ProviderEvent[] = done ? [
  ...["I’ll inspect the project files and its Git metadata.", "I’ll read the Python functions first.", "Next I’ll inspect the JavaScript and text files.", "I’ll include the Git metadata in the inventory.", "The inspection remains read only."].map((text, index) => ({ type: "message" as const, itemId: `early-${index}`, role: "assistant" as const, text })),
  ...["ls -la", "cat m1.py", "cat m2.py", "git status", "git log --oneline"].map((command, index) => ({ type: "command" as const, itemId: `cmd-${index}`, command, cwd: null, status: "completed" as const, exitCode: 0, output: "Clean", durationMs: null })),
  { type: "toolCall", itemId: "report", name: "mcp__brigadier__submit_report", input: "{}", status: "completed", output: null },
  { type: "command", itemId: "recent", command: "python3 -B - <<'PY'\nfrom pathlib import Path\nprint(list(Path('.').rglob('*')))\nPY", cwd: null, status: "completed", exitCode: 0, output: "43 files", durationMs: null },
] : [{ type: "command", itemId: "listing", command: "ls .", cwd: null, status: "inProgress", exitCode: null, output: null, durationMs: null }];
if (stress) events.push(
  { type: "message", itemId: "wide-reply", role: "assistant", text: `Inspect ${longPath}.\n\n\`\`\`text\n${"long code line ".repeat(40)}\n\`\`\`` },
  { type: "command", itemId: "wide-command", command: `cat ${longPath}`, cwd: null, status: "completed", exitCode: 0, output: "Read file", durationMs: null },
);
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

// A worker's row in the sidebar asks for it: the open session's Workers panel shows it.
const workerThread = () => document.querySelector('[data-slot="worker-thread"]');
if (query.has("open-worker")) {
  const result = document.body.appendChild(document.createElement("pre"));
  result.id = "workers-result";
  void (async () => {
    openWorker(id, "t2");
    for (let i = 0; i < 100 && !workerThread(); i++) await new Promise((resolve) => setTimeout(resolve, 50));
    const view = workerThread()?.closest('[role="region"], section');
    result.textContent = JSON.stringify({ label: view?.getAttribute("aria-label") ?? null, taken: useOpenWorker.getState().request });
  })();
}

// The render test uses the real sidebar and chat. Manual mode leaves it ready for screenshots.
const wait = (ms = 150) => new Promise((resolve) => setTimeout(resolve, ms));
const find = (selector: string) => document.querySelector<HTMLElement>(selector);
if (stress && !query.has("manual")) {
  const result = document.body.appendChild(document.createElement("pre"));
  result.id = "workers-result";
  const button = (label: string) => find(`button[aria-label="${label}"]`);
  const ready = async (selector: string) => {
    for (let i = 0; i < 100; i++) {
      const element = find(selector);
      if (element) return element;
      await wait(50);
    }
    throw new Error(`Missing ${selector}`);
  };
  const bounds = () => {
    const thread = find('[data-slot="worker-thread"]')!;
    const rect = thread.getBoundingClientRect();
    return {
      width: rect.width, overflow: thread.scrollWidth - thread.clientWidth,
      content: [...thread.querySelectorAll<HTMLElement>('[data-slot="worker-brief"], pre, [data-kind]')].map((node) => {
        const box = node.getBoundingClientRect();
        return { slot: node.dataset.slot ?? node.tagName, left: box.left - rect.left, right: rect.right - box.right };
      }),
    };
  };
  void (async () => {
    await ready('[data-slot="worker-card"] button[data-task="t1"]');
    find('[data-slot="worker-card"] button[data-task="t1"]')!.click();
    await ready('[data-slot="worker-brief"] .aui-md');
    await ready('[data-slot="worker-thread"] pre');
    await wait(500);
    // The worker view is a named region, and nothing above it takes it out of the accessibility tree.
    const view = find('[data-slot="worker-thread"]')!.closest('[role="region"], section');
    // A section with a name is a region.
    const role = view?.getAttribute("role") ?? (view?.hasAttribute("aria-label") ? "region" : view?.tagName.toLowerCase());
    const region = { role, label: view?.getAttribute("aria-label") };
    const hiddenBy: string[] = [];
    for (let node = find('[data-slot="worker-thread"]'); node && node !== document.body; node = node.parentElement) {
      const hidden = node.getAttribute("aria-hidden") === "true" || node.inert
        || (node.hasAttribute("role") && getComputedStyle(node).display === "contents");
      if (hidden) hiddenBy.push(node.outerHTML.slice(0, 120));
    }
    const sidebar = find('[data-slot="right-sidebar"]')!;
    const wide = bounds();
    // This is the same live CSS variable the sidebar drag updates before persisting its width.
    sidebar.style.setProperty("--right-sidebar-width", "320px");
    if (query.has("live-resize")) await new Promise(requestAnimationFrame);
    await wait(300);
    const narrow = bounds();
    const brief = find('[data-slot="worker-brief"]')!;
    brief.querySelector<HTMLButtonElement>("button")!.click();
    await wait();
    const expanded = bounds();
    const header = find('[data-slot="worker-header"]')!;
    const close = button("Close right panel")!;
    const headerRect = header.getBoundingClientRect();
    const closeRect = close.getBoundingClientRect();
    const pill = find('button[aria-label^="Worker details:"]')!;
    pill.focus();
    await wait(350);
    const tooltip = find('[role="tooltip"]')?.textContent;
    pill.click();
    await ready('[data-slot="worker-details"]');
    const details = !!find('[data-slot="worker-details"]');
    pill.click();
    await ready('[data-slot="worker-thread"]');
    const toggled = !find('[data-slot="worker-details"]');
    const noMenu = !button("Worker actions");
    const noTabs = !find('[role="tablist"][aria-label="Right panel"]');
    // Plans open in their own tab: the right panel has no Plan, even beside a worker.
    const noPlan = !find("#right-sidebar-plan") && !button("Plan");
    find('[data-slot="worker-header"] button')!.click();
    await ready('[role="tablist"][aria-label="Right panel"]');
    await wait(300);
    const listButton = button("Close right panel")!;
    const listRect = sidebar.getBoundingClientRect();
    const listCloseRect = listButton.getBoundingClientRect();
    const listClose = listCloseRect.width > 0 && listCloseRect.left >= listRect.left && listCloseRect.right <= listRect.right;
    button("Close right panel")!.click();
    await wait();
    const closed = sidebar.dataset.state === "collapsed";
    // A worker first shown without truncation gains its disclosure after a width change.
    useBoard.setState(({ board: current }) => ({ board: { ...current!, tasks: { ...current!.tasks,
      t1: { ...current!.tasks.t1!, spec: "Inspect the desktop worker panel and make its header compact. Keep its close button and model details easy to reach when the sidebar gets narrow." },
    } } }));
    find('[data-slot="worker-card"] button[data-task="t1"]')!.click();
    await ready('[data-slot="worker-brief"] .aui-md');
    sidebar.style.setProperty("--right-sidebar-width", "600px");
    if (query.has("live-resize")) await new Promise(requestAnimationFrame);
    await wait(300);
    const shortWideBounds = bounds();
    const shortWide = !!find('[data-slot="worker-brief"] button');
    sidebar.style.setProperty("--right-sidebar-width", "280px");
    if (query.has("live-resize")) await new Promise(requestAnimationFrame);
    await wait(300);
    const shortBody = find('[data-slot="worker-instructions"]')!;
    const shortMetrics = { width: shortBody.clientWidth, height: shortBody.clientHeight, scroll: shortBody.scrollHeight };
    const shortNarrow = !!find('[data-slot="worker-brief"] button');
    const minimum = bounds();
    const minimumHeader = find('[data-slot="worker-header"]')!.getBoundingClientRect();
    const headerFits = [...find('[data-slot="worker-header"]')!.querySelectorAll("button, h2")].every((node) => {
      const rect = node.getBoundingClientRect();
      return rect.width > 0 && rect.left >= minimumHeader.left && rect.right <= minimumHeader.right;
    });
    button("Close right panel")!.click();
    await wait();
    const workerClosed = sidebar.dataset.state === "collapsed";
    if (previousWidth === null) localStorage.removeItem("brigadier.rightSidebarWidth");
    else localStorage.setItem("brigadier.rightSidebarWidth", previousWidth);
    result.textContent = JSON.stringify({ region, hiddenBy, wide, narrow, expanded, minimum, details, toggled, tooltip, noMenu, noTabs,
      closeInHeader: closeRect.top >= headerRect.top && closeRect.bottom <= headerRect.bottom && closeRect.right <= headerRect.right,
      listClose, closed, workerClosed, headerFits, noPlan, shortWide, shortNarrow, shortWideBounds, shortMetrics });
  })().catch((error: unknown) => { result.textContent = JSON.stringify({ error: error instanceof Error ? error.stack : String(error) }); });
}

/** How wide a node shows against how wide its text is. */
const size = (node: HTMLElement) => ({ shown: node.clientWidth, full: node.scrollWidth });
if (headerTest) {
  const result = document.body.appendChild(document.createElement("pre"));
  result.id = "workers-header-result";
  void (async () => {
    for (let i = 0; i < 100 && !find('[data-slot="worker-card"] button[data-task="t1"]'); i++) await wait(50);
    find('[data-slot="worker-card"] button[data-task="t1"]')!.click();
    for (let i = 0; i < 100 && !find('[data-slot="worker-header"] h2'); i++) await wait(50);
    await wait(500);
    const header = find('[data-slot="worker-header"]')!;
    const title = header.querySelector("h2")!;
    const pill = header.querySelector<HTMLElement>('button[aria-label^="Worker details:"] span')!;
    result.textContent = JSON.stringify({ window: innerWidth, panel: header.clientWidth, title: size(title), pill: size(pill) });
  })().catch((error: unknown) => { result.textContent = JSON.stringify({ error: error instanceof Error ? error.stack : String(error) }); });
}

/** The dialog's added lines, tinted in the diff engine's own shadow root. */
const lines = () => [...document.querySelectorAll("[role=dialog] diffs-container")].flatMap((host) => [...host.shadowRoot?.querySelectorAll("[data-line-type=change-addition]") ?? []]);
if (diffTest) {
  const result = document.body.appendChild(document.createElement("pre"));
  result.id = "workers-diff-result";
  void (async () => {
    for (let i = 0; i < 100 && !find('[data-slot="worker-card"] button[data-task="t1"]'); i++) await wait(50);
    find('[data-slot="worker-card"] button[data-task="t1"]')!.click();
    let chip: HTMLElement | undefined;
    for (let i = 0; i < 100 && !chip; i++) {
      chip = [...document.querySelectorAll<HTMLElement>("[data-slot=file-list] button")].find((button) => button.textContent?.includes("task-1.diff"));
      await wait(50);
    }
    chip!.click();
    for (let i = 0; i < 100 && lines().length < 6 * 40; i++) await wait(50);
    await wait(300);
    // The diff's dialog: a pinned summary floats as a popover (also a dialog) once the worker's panel narrows the thread.
    const dialog = find("[role=dialog][data-slot=dialog-content]")!;
    const box = dialog.getBoundingClientRect();
    const list = find("[data-slot=artifact-diff]");
    result.textContent = JSON.stringify({
      title: dialog.querySelector("h2")?.textContent,
      colouredLines: lines().length,
      fits: box.top >= 0 && box.bottom <= innerHeight,
      scrolls: !!list && list.scrollHeight > list.clientHeight,
    });
  })().catch((error: unknown) => { result.textContent = JSON.stringify({ error: error instanceof Error ? error.stack : String(error) }); });
}
