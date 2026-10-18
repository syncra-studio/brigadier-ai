/**
 * The Source panel: the message box, Commit with its menu, the branch, and the changes grouped
 * as Staged Changes and Changes; More Actions' Branch › Merge… picking a branch to merge from;
 * a merge with conflicts listing them under Merge Changes, the box holding git's message and
 * Commit reading Continue until they are staged (asking first while markers are left); then
 * Continue committing the merge; a merge that ends without it takes its message along.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SourcePanel } from "@/app/conversation/SourcePanel";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Request, SourceBranch, SourceFile, SourceState } from "@/ipc/generated";
import { sessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { useToasts } from "@/state/toasts";
import { emitWorkspaceChanged } from "@/state/workspaceCache";

const id = "source-panel-fixture";
const branch = "brigadier/912a9469/session";
const MESSAGE = `Merge branch 'main' into ${branch}`;
const file = (path: string, status: SourceFile["status"]): SourceFile => ({ path, status, oldPath: null });
let state: SourceState = {
  branch,
  remote: "origin",
  upstream: null,
  ahead: 2,
  staged: [file("src/cart/tax.ts", "added")],
  changes: [file("notes.txt", "untracked"), file("README.md", "modified")],
  merging: null,
  rebasing: null,
};
const ago = (minutes: number) => Date.now() - minutes * 60_000;
const BRANCHES: SourceBranch[] = [
  { name: branch, remote: false, atMs: ago(1), author: "Stephen", subject: "Tax" },
  { name: "main", remote: false, atMs: ago(120), author: "Stephen", subject: "Member discount" },
  { name: "feature/coupons", remote: false, atMs: ago(5), author: "Stephen", subject: "Coupons and express shipping" },
  { name: "origin/main", remote: true, atMs: ago(60), author: "Stephen", subject: "Report watchdog silence" },
];
const calls: Request[] = [];
mockIPC((command, args) => {
  if (command !== "ipc_request") return null;
  const req = (args as { request: Request }).request;
  calls.push(req);
  switch (req.method) {
    case "getSourceState":
      return { method: req.method, state };
    case "listSourceBranches":
      return { method: req.method, branches: BRANCHES };
    case "sourceAction":
      if (req.action.type === "merge") {
        state = {
          ...state,
          staged: [file("src/cart/coupon.ts", "modified")],
          changes: [file("src/cart/total.ts", "conflicted")],
          merging: { incoming: "main", message: MESSAGE },
        };
      }
      return { method: req.method, state };
    case "getConflictFile":
      return {
        method: req.method,
        file: { text: "<<<<<<< HEAD\na\n=======\nb\n>>>>>>> main\n", current: "a\n", incoming: "b\n", merged: null, currentLabel: "HEAD", incomingLabel: "main", rebasing: null },
      };
    case "stageFiles":
      state = { ...state, staged: [...state.staged, file("src/cart/total.ts", "modified")], changes: [] };
      return { method: req.method, state };
    case "commitChanges":
      state = { ...state, staged: [], merging: null, ahead: 4 };
      return { method: req.method, outcome: { commit: "abc", message: req.message ?? "", branch, pushed: false, note: null } };
    default:
      return { method: req.method };
  }
});
useApp.setState({ info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 } });

function until(check: () => boolean, timeoutMs = 4000): Promise<void> {
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

const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const all = (selector: string, root: ParentNode = document) => [...root.querySelectorAll(selector)];
const button = (name: string, root: ParentNode = document) =>
  (all("button", root) as HTMLButtonElement[]).find((element) => text(element) === name || element.getAttribute("aria-label") === name);
const groups = () =>
  all('[data-slot="source-panel"] section[aria-label]').map(
    (group) => text(group.querySelector("header > button")) + text(group.querySelector("header > span:last-child")),
  );
const box = () => document.querySelector<HTMLTextAreaElement>('[aria-label="Commit message"]')!;
const commitButton = () => box().closest("div")!.parentElement!.querySelector<HTMLButtonElement>(":scope > div:nth-child(2) > button")!;
/** Opens a menu from the keyboard: a closed menu that stays mounted would swallow a pointer press. */
const openMenu = (trigger: HTMLElement | undefined) => {
  trigger?.focus();
  trigger?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
};
const menuItems = () => all('[role="menu"][data-state="open"] [role^="menuitem"]').map(text);
const menuItem = (name: string) =>
  all('[role="menu"][data-state="open"] [role^="menuitem"]').find((item) => text(item) === name) as HTMLElement | undefined;
const openSub = (name: string) => {
  const trigger = menuItem(name);
  trigger?.focus();
  trigger?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
};
const closeMenus = () => document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));

function Fixture() {
  const [result, setResult] = useState("");
  const [shown, setShown] = useState(true);
  useEffect(() => {
    const seen: Record<string, unknown> = {};
    void (async () => {
      await until(() => groups().length === 2);
      seen.groups = groups();
      seen.placeholder = box().placeholder;
      seen.commit = text(commitButton());
      seen.branch = text(document.querySelector('[data-slot="source-branch"]'));
      seen.panelText = text(document.querySelector('[data-slot="source-panel"]'));
      seen.rowActions = all('section[aria-label="Changes"] li').map((row) => all("button .sr-only", row).map(text));

      openMenu(button("Commit Actions"));
      await until(() => menuItems().length > 0);
      seen.commitMenu = menuItems();
      closeMenus();
      await until(() => menuItems().length === 0);

      openMenu(button("More Actions"));
      await until(() => menuItems().length > 0);
      seen.moreMenu = menuItems();
      openSub("Branch");
      await until(() => !!menuItem("Merge…"));
      menuItem("Merge…")!.click();
      await until(() => all('[data-slot="branch-picker"] [role="option"]').length > 0);
      const picker = document.querySelector('[data-slot="branch-picker"]')!;
      seen.pickerPlaceholder = picker.querySelector("input")?.placeholder;
      seen.picker = all('[role="option"]', picker).map(text);
      (all('[role="option"]', picker)[0] as HTMLElement).click();

      await until(() => groups().length === 3);
      seen.merging = {
        groups: groups(),
        message: box().value,
        commit: text(commitButton()),
        commitDisabled: commitButton().disabled,
        branch: text(document.querySelector('[data-slot="source-branch"]')),
        merged: calls.find((call) => call.method === "sourceAction"),
        toast: useToasts.getState().toasts.map((toast) => [toast.text, toast.actions.map((action) => action.label)]),
        rowActions: all('section[aria-label="Merge Changes"] li button .sr-only').map(text),
      };
      openMenu(button("More Actions"));
      await until(() => menuItems().length > 0);
      openSub("Commit");
      await until(() => !!menuItem("Abort Merge"));
      // Menus closed in virtual time stay mounted: read the Commit submenu itself.
      seen.commitSub = all('[role^="menuitem"]', menuItem("Abort Merge")!.closest('[role="menu"]')!).map(text);
      closeMenus();
      closeMenus();

      // A click on a conflicted file opens its conflict tab.
      (all('section[aria-label="Merge Changes"] li button')[0] as HTMLElement).click();
      seen.openedTab = sessionTabs(id).active;

      // Staging it while it still has markers asks first.
      button("Stage Changes", document.querySelector('section[aria-label="Merge Changes"]')!)!.click();
      await until(() => !!button("Stage"));
      seen.asked = text(document.querySelector('[role="dialog"] h2'));
      button("Stage")!.click();
      await until(() => groups().length === 2 && !commitButton().disabled);
      seen.resolved = { groups: groups(), commit: text(commitButton()) };
      commitButton().click();
      await until(() => calls.some((call) => call.method === "commitChanges"));
      seen.committed = calls.find((call) => call.method === "commitChanges");

      // A merge aborted elsewhere takes git's message out of the box with it.
      await until(() => box().value === "");
      state = { ...state, changes: [file("src/cart/total.ts", "conflicted")], merging: { incoming: "feature", message: "Merge branch 'feature'" } };
      emitWorkspaceChanged({ type: "conversation", conversationId: id });
      await until(() => box().value === "Merge branch 'feature'");
      state = { ...state, changes: [], merging: null };
      emitWorkspaceChanged({ type: "conversation", conversationId: id });
      await until(() => text(commitButton()) === "Commit");
      seen.afterAbort = box().value;

      // Closing the panel keeps the unsent commit message for when it opens again.
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(box(), "Unsent message");
      box().dispatchEvent(new Event("input", { bubbles: true }));
      setShown(false);
      await until(() => !document.querySelector('[aria-label="Commit message"]'));
      setShown(true);
      await until(() => !!document.querySelector('[aria-label="Commit message"]'));
      seen.keptMessage = box().value;
      setResult(JSON.stringify(seen));
    })().catch((error: unknown) => setResult(JSON.stringify({ error: String(error), seen, body: document.body.textContent?.slice(0, 2000) })));
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen w-80 flex-col">
        {shown && <SourcePanel target={{ type: "conversation", conversationId: id }} />}
        {result && <pre id="source-panel-result">{result}</pre>}
      </div>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
