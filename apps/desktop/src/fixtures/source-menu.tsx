/**
 * The Source panel's More Actions on a project's folder (#257): Branch › with Rebase, Create
 * (From), Rename, Delete and Publish; Stash ›; Show Git Output. Create Branch… takes a name in
 * its box, Delete Branch… a branch git won't delete as not merged offers Delete Anyway, Drop
 * Stash… asks first, and Git Output lists what ran, a failure in red, with Copy.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SourcePanel } from "@/app/side-panel/SourcePanel";
import { TooltipProvider } from "@/components/overlays";
import type { Request, SourceBranch, SourceGitRun, SourceState } from "@/ipc/generated";
import { useApp } from "@/state/store";
import { useToasts } from "@/state/toasts";

const project = "source-menu-project";
let state: SourceState = { branch: "main", remote: "origin", upstream: null, ahead: 0, staged: [], changes: [], merging: null, rebasing: null };
const ago = (minutes: number) => Date.now() - minutes * 60_000;
const BRANCHES: SourceBranch[] = [
  { name: "main", remote: false, atMs: ago(1), author: "Stephen", subject: "Tax" },
  { name: "feature/coupons", remote: false, atMs: ago(5), author: "Stephen", subject: "Coupons" },
  { name: "origin/main", remote: true, atMs: ago(60), author: "Stephen", subject: "Tax" },
];
const RUNS: SourceGitRun[] = [
  { atMs: ago(3), command: "git switch --quiet -c topic", output: "", ok: true },
  { atMs: ago(2), command: "git branch -d -- feature/coupons", output: "error: the branch 'feature/coupons' is not fully merged.", ok: false },
];
const calls: Request[] = [];
mockIPC((command, args) => {
  if (command !== "ipc_request") return null;
  const req = (args as { request: Request }).request;
  calls.push(req);
  switch (req.method) {
    case "getProjectSourceState":
      return { method: req.method, state };
    case "listSourceBranches":
      return { method: req.method, branches: BRANCHES };
    case "getSourceGitOutput":
      return { method: req.method, runs: RUNS };
    case "sourceAction":
      if (req.action.type === "deleteBranch" && !req.action.force) {
        throw "error: the branch 'feature/coupons' is not fully merged.";
      }
      if (req.action.type === "createBranch") state = { ...state, branch: req.action.name };
      return { method: req.method, state };
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
/** Opens a menu from the keyboard: a closed menu that stays mounted would swallow a pointer press. */
const openMenu = (trigger: HTMLElement | undefined) => {
  trigger?.focus();
  trigger?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
};
const menuItem = (name: string) =>
  all('[role="menu"][data-state="open"] [role^="menuitem"]').find((item) => text(item) === name) as HTMLElement | undefined;
const menuItems = () => all('[role="menu"][data-state="open"] [role^="menuitem"]').map(text);
const subItems = (name: string) => all('[role^="menuitem"]', menuItem(name)!.closest('[role="menu"]')!).map(text);
const openSub = (name: string) => {
  const trigger = menuItem(name);
  trigger?.focus();
  trigger?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
};
const closeMenus = () => document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
/** Opens More Actions › `sub` and picks `item` in it. */
async function choose(sub: string | null, item: string) {
  openMenu(button("More Actions"));
  await until(() => menuItems().length > 0);
  if (sub) {
    openSub(sub);
    await until(() => !!menuItem(item));
  }
  menuItem(item)!.click();
}
const typeInto = (input: HTMLInputElement, value: string) => {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  setter.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
};
const sourceActions = () => calls.filter((call) => call.method === "sourceAction").map((call) => (call.method === "sourceAction" ? call.action : null));

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    const seen: Record<string, unknown> = {};
    void (async () => {
      await until(() => !!button("More Actions"));
      openMenu(button("More Actions"));
      await until(() => menuItems().length > 0);
      seen.moreMenu = menuItems();
      openSub("Branch");
      await until(() => !!menuItem("Rebase Branch…"));
      seen.branchMenu = subItems("Rebase Branch…");
      closeMenus();
      closeMenus();
      await until(() => menuItems().length === 0);
      openMenu(button("More Actions"));
      await until(() => menuItems().length > 0);
      openSub("Stash");
      await until(() => !!menuItem("Pop Latest Stash"));
      seen.stashMenu = subItems("Pop Latest Stash");
      closeMenus();
      closeMenus();
      await until(() => menuItems().length === 0);

      // Create Branch…: a name, Enter.
      await choose("Branch", "Create Branch…");
      await until(() => !!document.querySelector('[data-slot="branch-name-box"] input'));
      const box = document.querySelector<HTMLInputElement>('[data-slot="branch-name-box"] input')!;
      seen.nameBox = [box.placeholder, text(document.querySelector('[data-slot="branch-name-box"] p'))];
      typeInto(box, " topic ");
      box.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
      await until(() => sourceActions().length === 1);
      await until(() => button("More Actions")?.disabled === false);

      // Delete Branch…: the picker offers local branches but the current one; not merged asks again.
      await choose("Branch", "Delete Branch…");
      await until(() => all('[data-slot="branch-picker"] [role="option"]').length > 0);
      const picker = document.querySelector('[data-slot="branch-picker"]')!;
      seen.deletePicker = [picker.querySelector("input")?.placeholder, all('[role="option"]', picker).map((option) => text(option.querySelector("span span")))];
      (all('[role="option"]', picker).find((option) => text(option).startsWith("feature/coupons")) as HTMLElement).click();
      await until(() => useToasts.getState().toasts.some((toast) => toast.actions.length > 0));
      const asked = useToasts.getState().toasts.find((toast) => toast.actions.length > 0)!;
      seen.notMerged = [asked.text, asked.actions.map((action) => action.label)];
      asked.actions[0]!.run();
      await until(() => sourceActions().length === 3);

      // Drop Stash… asks first.
      await choose("Stash", "Drop Stash…");
      await until(() => !!button("Drop"));
      seen.dropAsked = text(document.querySelector('[role="dialog"] h2'));
      button("Drop")!.click();
      await until(() => sourceActions().length === 4);
      seen.actions = sourceActions();

      // Show Git Output: what ran, a failure in red, Copy; no Close button.
      await choose(null, "Show Git Output");
      await until(() => all('[data-slot="git-output-run"]').length === 2);
      const dialog = document.querySelector('[data-slot="git-output"]')!;
      seen.output = {
        title: text(dialog.querySelector("h2")),
        runs: all('[data-slot="git-output-run"]', dialog).map((run) => text(run).replace(/^\S+ (AM|PM)?\s*/, "")),
        failedRed: all('[data-slot="git-output-run"] .text-destructive', dialog).map(text),
        buttons: all("button", dialog).map((element) => text(element) || element.getAttribute("aria-label") || ""),
      };
      setResult(JSON.stringify(seen));
    })().catch((error: unknown) => setResult(JSON.stringify({ error: String(error), seen, body: document.body.textContent?.slice(0, 2000) })));
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen w-80 flex-col">
        <SourcePanel target={{ type: "project", projectId: project, readOnly: false }} />
        {result && <pre id="source-menu-result">{result}</pre>}
      </div>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
