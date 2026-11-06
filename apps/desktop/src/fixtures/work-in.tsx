/**
 * A standalone Vite entry, excluded from the app entry and production build: a new session's
 * utility bar (project, Work in, branch) on a project whose folder has `feature-x` checked out,
 * IPC mocked. It opens Work in, picks Local checkout, New worktree again, then Existing worktree
 * and the worktree another session works in (goal 9), and writes the menu's rows, the pills and the setup each asks for as JSON into `<pre id="work-in-result">`;
 * `stop=1` holds the page with the menu open for a screenshot, `stop=local` once Local checkout
 * is picked.
 */
import { mockIPC } from "@/ipc/nativeMocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ComposerHint } from "@/app/conversation/ComposerHint";
import type { ComposerTarget } from "@/app/conversation/composerTarget";
import { useResolvedDraft } from "@/app/conversation/draftSetup";
import { UtilityBar } from "@/app/conversation/UtilityBar";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, Project, RepoInfo, Request } from "@/ipc/generated";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const project = { id: "p1", name: "todo-app", createdAtMs: 0, repos: [{ path: "/work/todo-app", name: "todo-app" }], prefs: {}, trust: [] } as unknown as Project;
const repo: RepoInfo = {
  path: "/work/todo-app", name: "todo-app", currentBranch: "feature-x", dirty: true,
  branches: [
    { name: "main", commit: "a1", checkedOutAt: null, worker: false },
    { name: "feature-x", commit: "b2", checkedOutAt: "/work/todo-app", worker: false },
    { name: "fix-login", commit: "c3", checkedOutAt: "/Users/me/.brigadier/s/ab12cd34/work", worker: false },
    { name: "hotfix", commit: "e5", checkedOutAt: "/work/todo-app-hotfix", worker: false },
    { name: "brigadier/ab12cd34/task-1", commit: "d4", checkedOutAt: "/Users/me/.brigadier/s/ab12cd34/workers/1/work", worker: true },
  ],
};

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "getRepoInfo") return { method: req.method, repo };
  return { method: req.method };
});
// The session that works in fix-login's folder: its row reads by its title (D184).
const fixLogin = {
  id: "s1", title: "Fix the login", createdAtMs: 0, lifecycle: "active",
  setup: { type: "session", repo: "/work/todo-app", environment: { type: "newWorktree", base: "main", branch: "fix-login", path: "/Users/me/.brigadier/s/ab12cd34/work", start: null } },
} as unknown as Conversation;
useApp.setState({
  projects: { p1: project },
  conversations: { s1: fixLogin },
  selection: { type: "draft", kind: "session", projectId: "p1" },
  connection: { status: "connected", daemon: null, reason: null },
});

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
async function until<T>(what: string, find: () => T | null | undefined, ms = 10_000): Promise<T> {
  const end = performance.now() + ms;
  for (;;) {
    const found = find();
    if (found) return found;
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(16);
  }
}

const pill = (label: string) => document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);
const rows = () => [...document.querySelectorAll<HTMLElement>("[role=menuitemradio]")];
const row = (label: string) => rows().find((each) => each.textContent === label);
const options = () => [...document.querySelectorAll<HTMLElement>("[role=option]")];
const setup = () => JSON.parse(document.querySelector("#work-in-setup")?.textContent ?? "null") as unknown;
const hint = () => document.querySelector("#work-in-hint")?.textContent ?? "";
const press = (element: HTMLElement) => {
  // Radix opens on pointerdown, as a real click does.
  element.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0, pointerType: "mouse" }));
  element.click();
};

async function drive(): Promise<Record<string, unknown>> {
  const seen: Record<string, unknown> = {};
  await until("the branch pill", () => pill("Base branch")?.textContent === "feature-x");
  seen.worktreePills = [pill("Work in")?.textContent, pill("Base branch")?.textContent];
  seen.worktreeSetup = setup();
  seen.worktreeHint = hint();
  press(pill("Work in")!);
  await until("the Work in menu", () => rows().length > 0);
  seen.rows = rows().map((each) => [each.textContent, each.getAttribute("aria-checked")]);
  if (query.get("stop") === "1") return seen;
  press(row("Local checkout")!);
  await until("the menu to close", () => rows().length === 0);
  await until("the local checkout's branch pill", () => pill("Branch"));
  seen.localPills = [pill("Work in")?.textContent, pill("Branch")?.textContent];
  seen.localSetup = setup();
  seen.localHint = hint();
  if (query.get("stop") === "local") return seen;
  press(pill("Work in")!);
  await until("the Work in menu again", () => rows().length > 0);
  seen.localRows = rows().map((each) => [each.textContent, each.getAttribute("aria-checked")]);
  press(row("New worktree")!);
  await until("the base branch pill again", () => pill("Base branch"));
  seen.backSetup = setup();
  // Existing worktree: nothing picked yet, then the worktrees with their branches.
  press(pill("Work in")!);
  await until("the Work in menu a third time", () => rows().length > 0);
  press(row("Existing worktree")!);
  await until("the worktree pill", () => pill("Worktree"));
  seen.existingPills = [pill("Work in")?.textContent, pill("Worktree")?.textContent];
  seen.existingProblem = setup();
  press(pill("Worktree")!);
  await until("the worktree list", () => options().length > 0);
  seen.worktreeRows = options().map((each) => each.textContent);
  if (query.get("stop") === "existing") return seen;
  press(options().find((each) => each.textContent?.endsWith("fix-login"))!);
  await until("the picked worktree", () => pill("Worktree")?.textContent === "Fix the login");
  seen.pickedPill = pill("Worktree")?.textContent;
  seen.existingSetup = setup();
  return seen;
}

function Fixture() {
  const selection = useApp((s) => s.selection);
  const resolved = useResolvedDraft(selection);
  const [result, setResult] = useState("");
  useEffect(() => {
    drive().then(
      (seen) => { if (!query.has("stop")) setResult(JSON.stringify(seen)); },
      (error: unknown) => setResult(JSON.stringify({ error: String(error), body: document.body.textContent?.slice(0, 2000) })),
    );
  }, []);
  const target = { conversation: null, resolved } as unknown as ComposerTarget;
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen flex-col justify-end gap-2 p-6">
        <div className="mx-auto w-full max-w-2xl">
          <UtilityBar resolved={resolved} />
          <div id="work-in-hint"><ComposerHint target={target} /></div>
        </div>
        <pre id="work-in-setup" hidden>{JSON.stringify(resolved.target?.kind === "session" ? resolved.target.setup : resolved.problem)}</pre>
        {result && <pre id="work-in-result">{result}</pre>}
      </div>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
