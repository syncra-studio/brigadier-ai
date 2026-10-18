/**
 * A rebase in the Source panel that stopped on conflicts (#283): the branch reads "(rebasing)",
 * the box holds the message of the commit it replays, Commit reads Continue until the conflicts
 * are staged and the Commit menu offers Abort Rebase; the conflict tab says which commit it
 * replays. Continue goes on to the next commit's conflicts, and Abort Rebase ends it. Add
 * `?still` to stop at the first stop, for a look at it.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ConflictTab } from "@/app/conversation/ConflictTab";
import { SourcePanel } from "@/app/conversation/SourcePanel";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { ConflictFile, Request, SourceFile, SourceRebase, SourceState } from "@/ipc/generated";
import { useApp } from "@/state/store";
import { useToasts } from "@/state/toasts";

const id = "source-rebase-fixture";
const branch = "brigadier/912a9469/session";
const path = "src/cart/total.ts";
const file = (at: string, status: SourceFile["status"]): SourceFile => ({ path: at, status, oldPath: null });
const step = (at: number, commit: string, message: string): SourceRebase => ({ branch, onto: "main", step: at, total: 5, commit, message });
const MARKED = [
  'import { couponFor } from "./coupon";\n',
  "export function total(cart) {\n",
  "<<<<<<< HEAD\n",
  "  return cart.member ? 9 : 10;\n",
  "=======\n",
  "  return couponFor(cart).apply(10);\n",
  ">>>>>>> 4f2a9c1 (Apply coupons before tax)\n",
  "}\n",
].join("");
const SECOND = step(2, "4f2a9c1", "Apply coupons before tax\n\nCoupons now come off the subtotal.");
const THIRD = step(3, "9b03e7d", "Show the discount on the receipt");
let state: SourceState = {
  branch: null,
  remote: "origin",
  upstream: null,
  ahead: 0,
  staged: [file("src/cart/coupon.ts", "modified")],
  changes: [file(path, "conflicted")],
  merging: null,
  rebasing: SECOND,
};
let conflict: ConflictFile = {
  text: MARKED,
  current: "current side\n",
  incoming: "incoming side\n",
  merged: null,
  currentLabel: "7d41e0a • main",
  incomingLabel: `4f2a9c1 • ${branch}`,
  rebasing: SECOND,
};
const calls: Request[] = [];
mockIPC((command, args) => {
  if (command !== "ipc_request") return null;
  const req = (args as { request: Request }).request;
  calls.push(req);
  switch (req.method) {
    case "getSourceState":
      return { method: req.method, state };
    case "getConflictFile":
      return { method: req.method, file: conflict };
    case "stageFiles":
      state = { ...state, staged: [...state.staged, file(path, "modified")], changes: [] };
      return { method: req.method, state };
    case "sourceAction":
      if (req.action.type === "resolve") {
        conflict = { ...conflict, text: req.action.text };
      } else if (req.action.type === "continueRebase") {
        // The next commit stops on its own conflicts.
        state = { ...state, staged: [], changes: [file(path, "conflicted")], rebasing: THIRD };
        conflict = { ...conflict, text: MARKED.replace("4f2a9c1 (Apply coupons before tax)", "9b03e7d (Show the discount on the receipt)"), rebasing: THIRD };
      } else if (req.action.type === "abortRebase") {
        state = { ...state, branch, staged: [], changes: [], rebasing: null };
      }
      return { method: req.method, state };
    default:
      return { method: req.method };
  }
});
useApp.setState({ info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 } });

function until(check: () => boolean, timeoutMs = 4000): Promise<void> {
  const started = performance.now();
  return new Promise((resolve, reject) => {
    const tick = () => {
      if (check()) resolve();
      else if (performance.now() - started > timeoutMs) reject(new Error("timed out"));
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
const branchItem = () => text(document.querySelector('[data-slot="source-branch"]'));
const rebaseStep = () => text(document.querySelector('[data-slot="conflict-rebase"]'));
const openMenu = (trigger: HTMLElement | undefined) => {
  trigger?.focus();
  trigger?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
};
const menuItem = (name: string) =>
  all('[role="menu"][data-state="open"] [role^="menuitem"]').find((item) => text(item) === name) as HTMLElement | undefined;
const openSub = (name: string) => {
  const trigger = menuItem(name);
  trigger?.focus();
  trigger?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
};
const closeMenus = () => document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
const toasts = () => useToasts.getState().toasts.map((toast) => toast.text);

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (new URLSearchParams(location.search).has("still")) return;
    const seen: Record<string, unknown> = {};
    void (async () => {
      await until(() => groups().length === 3 && rebaseStep() !== "");
      seen.stopped = {
        groups: groups(),
        branch: branchItem(),
        message: box().value,
        boxDisabled: box().disabled,
        commit: text(commitButton()),
        commitDisabled: commitButton().disabled,
        step: rebaseStep(),
        lenses: all('[data-slot="conflict-lens"]').map(text),
      };
      openMenu(button("More Actions"));
      await until(() => !!menuItem("Commit"));
      openSub("Commit");
      await until(() => !!menuItem("Abort Rebase"));
      seen.commitSub = all('[role^="menuitem"]', menuItem("Abort Rebase")!.closest('[role="menu"]')!).map(text);
      closeMenus();
      closeMenus();
      openMenu(button("More Actions"));
      await until(() => !!menuItem("Branch"));
      openSub("Branch");
      await until(() => !!menuItem("Merge…"));
      seen.branchDisabled = ["Merge…", "Rebase Branch…"].map((name) => menuItem(name)?.getAttribute("data-disabled") === "");
      closeMenus();
      closeMenus();

      // Resolved in the conflict tab and staged, Continue goes on to the next commit's conflicts.
      button("Accept Incoming Change")!.click();
      await until(() => !conflict.text?.includes("<<<<<<<"));
      button("Stage Changes", document.querySelector('section[aria-label="Merge Changes"]')!)!.click();
      await until(() => !commitButton().disabled);
      seen.resolved = { groups: groups(), commit: text(commitButton()) };
      commitButton().click();
      await until(() => rebaseStep().startsWith("Rebasing 3"));
      seen.continued = {
        call: calls.find((call) => call.method === "sourceAction" && call.action.type === "continueRebase"),
        step: rebaseStep(),
        message: box().value,
        toasts: toasts(),
      };

      // Abort Rebase puts the branch back.
      openMenu(button("More Actions"));
      await until(() => !!menuItem("Commit"));
      openSub("Commit");
      await until(() => !!menuItem("Abort Rebase"));
      menuItem("Abort Rebase")!.click();
      await until(() => text(commitButton()) === "Commit");
      seen.aborted = { branch: branchItem(), message: box().value, boxDisabled: box().disabled };
      setResult(JSON.stringify(seen));
    })().catch((error: unknown) => setResult(JSON.stringify({ error: String(error), seen, body: document.body.textContent?.slice(0, 2000) })));
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen w-full">
        <div className="border-border flex w-80 shrink-0 flex-col border-e">
          <SourcePanel target={{ type: "conversation", conversationId: id }} />
        </div>
        <div className="flex min-w-0 flex-1 flex-col">
          <ConflictTab conversationId={id} tab={{ kind: "conflict", id: `conflict:${path}`, opener: null, path, view: "conflict" }} />
        </div>
        {result && <pre id="source-rebase-result">{result}</pre>}
      </div>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
