/**
 * The right sidebar's Files tree with a checkout's changes: coloured names with their letters,
 * a dot on folders holding changes, deleted files struck through, ignored entries dimmed and an
 * ignored folder read when opened, again when the tree comes back and anew for another checkout;
 * a change heard from the daemon reads the tree again.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { FilesTab } from "@/app/conversation/FilesTab";
import type { Conversation, Request, SourceFile } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { useApp } from "@/state/store";
import { emitWorkspaceChanged } from "@/state/workspaceCache";

const id = "files-tree-fixture";
const conversation: Conversation = { id, kind: "session", title: "Files tree", projectId: null,
  pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: null, lifecycle: "active",
  forkedFrom: null, sideOf: null, fallback: null, quotaWait: null };
useApp.setState({ conversations: { [id]: conversation }, selection: { type: "conversation", id },
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 } });
useBoard.setState({ board: { ...emptyBoard(id), loaded: true } });

let files = ["README.md", "package.json", "src/app.ts", "src/lib/util.ts", "src/new.ts"];
let changes: SourceFile[] = [
  { path: "README.md", oldPath: null, status: "modified" },
  { path: "src/new.ts", oldPath: null, status: "untracked" },
  { path: "src/old.ts", oldPath: null, status: "deleted" },
];
const staged: SourceFile[] = [{ path: "src/lib/util.ts", oldPath: null, status: "added" }];
let folder = [{ name: "react", folder: true }, { name: ".package-lock.json", folder: false }];
/** While set, folder reads answer only once it settles: a late answer. */
let hold: Promise<void> | null = null;
const asked: string[] = [];
mockIPC((command, args) => {
  if (command !== "ipc_request") return null;
  const req = (args as { request: Request }).request;
  asked.push(req.method);
  if (req.method === "getWorkspaceTree")
    return { method: req.method, tree: { files, truncated: false, ignored: [".env", "node_modules/"], ignoredTruncated: false } };
  if (req.method === "getSourceState")
    return { method: req.method, state: { branch: "main", remote: null, upstream: null, ahead: 0, staged, changes, merging: null, rebasing: null } };
  if (req.method === "listWorkspaceFolder") {
    const answer = { method: req.method, folder: { entries: folder, truncated: false } };
    return hold ? hold.then(() => answer) : answer;
  }
  return { method: req.method };
});

/** Each visible row: its name, letter, colour class, and how it is drawn. */
function rows() {
  return [...document.querySelectorAll<HTMLButtonElement>('[role="tree"] [role="treeitem"] > button')].map((button) => {
    const name = button.querySelector("span.truncate");
    const marker = button.lastElementChild;
    return {
      name: name?.textContent ?? "",
      title: button.title,
      letter: marker?.classList.contains("text-xs") ? marker.textContent : null,
      dot: !!marker?.classList.contains("rounded-full"),
      colour: [...button.classList].find((c) => /^text-(warning|success|destructive|link)$/.test(c)) ?? null,
      struck: !!name?.classList.contains("line-through"),
      dimmed: getComputedStyle(button).opacity === "0.5",
      icon: [...button.querySelector("svg[class*='text-file-']")?.classList ?? []].find((c) => c.startsWith("text-file-")) ?? null,
    };
  });
}

function until(check: () => boolean, timeoutMs = 3000): Promise<void> {
  const start = performance.now();
  return new Promise((resolve, reject) => {
    const tick = () => {
      if (check()) resolve();
      else if (performance.now() - start > timeoutMs) reject(new Error("timed out"));
      else setTimeout(tick, 20);
    };
    tick();
  });
}

const row = (name: string) => [...document.querySelectorAll<HTMLButtonElement>('[role="treeitem"] > button')]
  .find((button) => button.querySelector("span.truncate")?.textContent === name);

const folderReads = () => asked.filter((method) => method === "listWorkspaceFolder").length;

function Fixture() {
  const [result, setResult] = useState("");
  const [shown, setShown] = useState(true);
  useEffect(() => {
    const seen: Record<string, unknown> = {};
    void (async () => {
      await until(() => !!row("src") && !!row("README.md")?.className.includes("text-warning"));
      seen.top = rows();
      row("src")!.click();
      await until(() => !!row("lib"));
      row("lib")!.click();
      await until(() => !!row("util.ts"));
      seen.opened = rows();
      row("node_modules")!.click();
      await until(() => !!row("react"));
      seen.ignoredOpened = rows().filter((r) => ["node_modules", "react", ".package-lock.json"].includes(r.name));
      // The daemon says the checkout changed: a file appeared and README's change was undone.
      files = [...files, "CHANGELOG.md"].toSorted();
      changes = [...changes.filter((file) => file.path !== "README.md"), { path: "CHANGELOG.md", oldPath: null, status: "untracked" }];
      const readsBefore = folderReads();
      emitWorkspaceChanged({ type: "conversation", conversationId: id });
      await until(() => !!row("CHANGELOG.md")?.className.includes("text-success") && !row("README.md")?.className.includes("text-warning"));
      seen.changed = rows().filter((r) => ["CHANGELOG.md", "README.md"].includes(r.name));
      seen.folderReread = folderReads() > readsBefore;
      // Another checkout's change is not this tree's.
      const reads = asked.length;
      emitWorkspaceChanged({ type: "conversation", conversationId: "another" });
      await new Promise((resolve) => setTimeout(resolve, 100));
      seen.otherIgnored = asked.length === reads;
      // The tree goes (another view is shown) and comes back with the ignored folder still open.
      setShown(false);
      await until(() => !row("src"));
      setShown(true);
      await until(() => !!row("react"));
      seen.remounted = rows().filter((r) => ["node_modules", "react"].includes(r.name)).map((r) => r.name);
      // The session moves to another checkout while the old one's folder read is on its way.
      let release: (() => void) | undefined;
      hold = new Promise((resolve) => { release = resolve; });
      const heldFrom = folderReads();
      emitWorkspaceChanged({ type: "conversation", conversationId: id });
      await until(() => folderReads() > heldFrom);
      hold = null;
      folder = [{ name: "vue", folder: true }];
      useApp.setState({ conversations: { [id]: { ...conversation, lifecycle: "hibernated" } } });
      await until(() => !!row("vue") && !row("react"));
      release?.();
      await new Promise((resolve) => setTimeout(resolve, 150));
      seen.otherCheckout = rows().filter((r) => ["react", "vue"].includes(r.name)).map((r) => r.name);
      setResult(JSON.stringify(seen));
    })().catch((error: unknown) => setResult(JSON.stringify({ error: String(error), rows: rows() })));
  }, []);
  return <div className="bg-background text-foreground flex h-screen w-80 flex-col">
    {shown && <FilesTab conversationId={id} />}
    {result && <pre id="files-tree-result">{result}</pre>}
  </div>;
}

createRoot(document.getElementById("root")!).render(<Fixture />);
