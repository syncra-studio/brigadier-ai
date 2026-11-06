/**
 * Two tabs of one name each show their checkout, dim, only while they clash; the row of the tab in
 * front lights up in Files and Source, in a draft and a session, and on stacked rows only in that
 * tab's own checkout. `?drive=1` runs the checks; `?shot=<name>` holds one state for a screenshot.
 */
import { mockIPC } from "@/ipc/nativeMocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@/components/conversation";

import { SessionTabBar } from "@/app/shell/SessionTabBar";
import { FilesTab } from "@/app/side-panel/FilesTab";
import { ProjectDraftPane } from "@/app/side-panel/ProjectDraftPane";
import { SourcePanel } from "@/app/side-panel/SourcePanel";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, Project, Request, SourceFile, SourceState } from "@/ipc/generated";
import { CHAT_TAB, closeTab, openFileTab, openReviewTab, selectTab, useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { readUserWorktrees } from "@/state/workspaceCache";
import type { WorkspacePaneTab } from "@/state/workspacePane";

const params = new URLSearchParams(location.search);
const PROJECT = "project";
const DRAFT = `draft:${PROJECT}`;
const SESSION = "session-1";
const WORKTREE = "/scratch/brigadier-tabs-mine";
const file = (path: string, status: SourceFile["status"]): SourceFile => ({ path, status, oldPath: null });
const source = (changes: SourceFile[], staged: SourceFile[] = []): SourceState =>
  ({ branch: "main", remote: null, upstream: null, ahead: 0, behind: 0, staged, changes, merging: null, rebasing: null }) as unknown as SourceState;
const FOLDER = source([file("src/main.ts", "modified"), file("only-in-folder.txt", "untracked")], [file("README.md", "modified")]);
const MINE = source([file("src/main.ts", "modified")]);
const TREE = { files: ["README.md", "only-in-folder.txt", "src/main.ts"], truncated: false, ignored: [], ignoredTruncated: false };

useSessionTabs.setState({ sessions: {} });
localStorage.clear();
useApp.setState({
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { status: "connected", daemon: null, reason: null },
  projects: { [PROJECT]: { id: PROJECT, name: "brigadier-tabs", repos: [{ path: "/scratch/brigadier-tabs" }] } as unknown as Project },
  conversations: { [SESSION]: {
    id: SESSION, kind: "session", projectId: PROJECT, title: "Tidy the tabs", pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, lifecycle: "active",
    forkedFrom: null, sideOf: null, quotaWait: null,
    setup: { type: "session", repo: "/scratch/brigadier-tabs", environment: { type: "localCheckout", branch: "main" } },
  } as unknown as Conversation },
});
mockIPC((command, args) => {
  if (command !== "ipc_request") return null;
  const req = (args as { request: Request }).request;
  switch (req.method) {
    case "listUserWorktrees": return { method: req.method, paths: [WORKTREE] };
    case "getProjectSourceState": case "getSourceState": return { method: req.method, state: FOLDER };
    case "getWorktreeSourceState": return { method: req.method, state: MINE };
    case "getWorkspaceTree": return { method: req.method, tree: TREE };
    case "pageList": return { method: req.method, pages: [] };
    default: return { method: req.method };
  }
});

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(what: string, check: () => unknown, ms = 8000): Promise<void> {
  const start = performance.now();
  while (!check()) { if (performance.now() - start > ms) throw new Error(`timed out: ${what}`); await wait(20); }
}
const all = <E extends Element>(selector: string, root: ParentNode = document) => [...root.querySelectorAll<E>(selector)];
const tabs = () => all('[data-slot="session-tabs"] [data-session-tab]').map((tab) => tab.querySelector(".session-tab-title")?.textContent ?? "");
const notes = () => all('[data-slot="tab-name-note"]').map((note) => note.textContent);
/** The lit rows of each Source panel on the page, by the file's name and its group. */
const litSource = () => all('[data-slot="source-panel"]').map((panel) => all("li[aria-current]", panel).map((row) => row.querySelector("button")?.title));
const litFiles = () => all('[role="tree"] li[aria-selected="true"]').map((row) => row.querySelector("button")?.title);
const treeRow = (name: string) => all<HTMLElement>('[role="tree"] li').find((row) => row.querySelector("button")?.title.startsWith(name));

function Fixture() {
  const [view, setView] = useState<{ owner: string; pane: WorkspacePaneTab }>({ owner: params.get("session") ? SESSION : DRAFT, pane: (params.get("pane") as WorkspacePaneTab | null) ?? "source" });
  const [result, setResult] = useState("");
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  useEffect(() => {
    void readUserWorktrees(PROJECT);
    const shot = params.get("shot");
    if (shot === "clash") {
      openFileTab(DRAFT, "README.md");
      openFileTab(DRAFT, "src/main.ts");
      openFileTab(DRAFT, "src/main.ts", { checkout: WORKTREE });
    }
    if (shot === "worktree") { openFileTab(DRAFT, "src/main.ts", { checkout: WORKTREE }); void openRows(); }
    if (shot === "folder") { openReviewTab(DRAFT, { type: "file", path: "src/main.ts", staged: false }); void openRows(); }
    if (shot === "files") openFileTab(DRAFT, "only-in-folder.txt");
    if (shot === "session") openReviewTab(SESSION, { type: "file", path: "src/main.ts", staged: false });
    if (!params.has("drive")) return;
    void (async () => {
      const seen: Record<string, unknown> = {};
      await until("rows", () => all('[data-slot="source-row"]').length === 2);
      await openRows();
      // One tab has nothing to tell apart.
      openFileTab(DRAFT, "src/main.ts");
      await until("a tab", () => tabs().length === 1);
      seen.oneTab = { tabs: tabs(), notes: notes() };
      // The same name from another checkout: each says whose, dim.
      openFileTab(DRAFT, "src/main.ts", { checkout: WORKTREE });
      await until("two tabs", () => tabs().length === 2);
      await wait(250);
      seen.clash = { tabs: tabs(), notes: notes() };
      const note = document.querySelector('[data-slot="tab-name-note"]')!;
      seen.noteDim = getComputedStyle(note).color !== getComputedStyle(note.parentElement!).color;
      // Other names don't clash.
      openFileTab(DRAFT, "README.md");
      await until("three tabs", () => tabs().length === 3);
      seen.clashWithOther = { tabs: tabs(), notes: notes() };
      closeTab(DRAFT, "file:README.md");
      // The other goes: the note goes with the clash.
      closeTab(DRAFT, `file:${WORKTREE}//src/main.ts`);
      await until("note gone", () => notes().length === 0 && tabs().length === 1);
      seen.clashOver = { tabs: tabs(), notes: notes() };

      // Stacked rows: only the row in the tab's own checkout.
      await until("lit folder", () => litSource().flat().length === 1);
      seen.folderFile = litSource();
      openFileTab(DRAFT, "src/main.ts", { checkout: WORKTREE });
      await until("lit worktree", () => litSource()[1]?.length === 1);
      seen.worktreeFile = litSource();
      openReviewTab(DRAFT, { type: "file", path: "src/main.ts", staged: false }, undefined, WORKTREE);
      await wait(100);
      seen.worktreeChange = litSource();
      // A Review tab is of one side of a file; the other kinds of Review have no one row.
      openReviewTab(DRAFT, { type: "file", path: "README.md", staged: true });
      await until("staged row", () => litSource()[0]?.[0] === "README.md");
      seen.stagedChange = litSource();
      openReviewTab(DRAFT, { type: "file", path: "README.md", staged: false });
      await until("no such unstaged row", () => litSource().flat().length === 0);
      seen.unstagedOfStaged = litSource();
      openReviewTab(DRAFT, { type: "changes", staged: false });
      await wait(100);
      seen.wholeGroup = litSource();
      selectTab(DRAFT, CHAT_TAB);
      await wait(100);
      seen.chatInFront = litSource();

      // Files, the project's tree: not lit by a worktree's tab.
      setView({ owner: DRAFT, pane: "files" });
      await until("tree", () => treeRow("only-in-folder.txt"));
      openFileTab(DRAFT, "only-in-folder.txt");
      await until("lit file", () => litFiles().length === 1);
      seen.draftFiles = litFiles();
      seen.litBackground = getComputedStyle(treeRow("only-in-folder.txt")!.querySelector("button")!).backgroundColor !== getComputedStyle(treeRow("README.md")!.querySelector("button")!).backgroundColor;
      openReviewTab(DRAFT, { type: "file", path: "src/main.ts", staged: false }, undefined, WORKTREE);
      await wait(100);
      seen.draftFilesWorktreeTab = litFiles();

      // A session alike, and within one checkout the name's folder tells them apart.
      setView({ owner: SESSION, pane: "source" });
      await until("session source", () => all('[data-slot="source-panel"]').length === 1 && all("li", all('[data-slot="source-panel"]')[0]).length > 0);
      openReviewTab(SESSION, { type: "file", path: "src/main.ts", staged: false });
      await until("session lit", () => litSource().flat().length === 1);
      seen.sessionSource = litSource();
      setView({ owner: SESSION, pane: "files" });
      await until("session tree", () => treeRow("README.md"));
      openFileTab(SESSION, "README.md");
      await until("session file", () => litFiles().length === 1);
      seen.sessionFiles = litFiles();
      openFileTab(SESSION, "src/a/index.ts");
      openFileTab(SESSION, "src/b/index.ts");
      await until("same checkout's clash", () => notes().length === 2);
      seen.sameCheckout = notes();
      setResult(JSON.stringify(seen));
    })().catch((error: unknown) => setResult(JSON.stringify({ error: String(error instanceof Error ? error.stack ?? error.message : error) })));
  }, []);
  const conversation = useApp((s) => (view.owner === SESSION ? s.conversations[SESSION]! : null));
  return (
    <TooltipProvider>
      <AssistantRuntimeProvider runtime={runtime}>
        <div className="bg-background text-foreground flex h-screen flex-col">
          <SessionTabBar id={view.owner} conversation={conversation} onRename={() => {}} />
          <div className="flex min-h-0 flex-1">
            <main className="min-w-0 flex-1" />
            <aside className="border-border flex w-sidebar shrink-0 flex-col border-s" id="workspace-pane-content">
              {view.owner === SESSION
                ? view.pane === "files" ? <FilesTab conversationId={SESSION} /> : <SourcePanel target={{ type: "conversation", conversationId: SESSION }} />
                : <ProjectDraftPane key={view.pane} projectId={PROJECT} tab={view.pane} searchRequest={0} onSearchHandled={() => {}} />}
            </aside>
          </div>
        </div>
        {result && <pre id="tab-names-rows-result" hidden>{result}</pre>}
      </AssistantRuntimeProvider>
    </TooltipProvider>
  );
}

/** Unfolds the worktree's row, which starts folded. */
async function openRows(): Promise<void> {
  await until("rows", () => all('[data-slot="source-row"]').length === 2);
  const fold = all<HTMLButtonElement>('[data-slot="source-row"] button[aria-label^="Fold"][aria-expanded="false"]')[0];
  fold?.click();
}

createRoot(document.getElementById("root")!).render(<Fixture />);
