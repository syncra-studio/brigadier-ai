/**
 * A conflicted file's tabs: its conflicts in place, each accepted with Accept Current, Incoming
 * or Both and written back only over the file as it was read; Compare Changes opening both
 * sides; and the merge editor, whose picks make the result and whose Complete Merge asks first
 * while conflicts are left, then writes the result and stages it.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ConflictTab } from "@/app/conversation/ConflictTab";
import { TooltipProvider } from "@/components/overlays";
import type { ConflictFile, Request, SourceAction } from "@/ipc/generated";
import { type ConflictTabState, sessionTabs } from "@/state/sessionTabs";

const id = "conflict-tab-fixture";
const path = "src/cart/total.ts";
const MARKED = [
  "import { couponFor } from \"./coupon\";\n",
  "export function total(cart) {\n",
  "<<<<<<< HEAD\n",
  "  return cart.member ? 9 : 10;\n",
  "=======\n",
  "  return couponFor(cart).apply(10);\n",
  ">>>>>>> main\n",
  "}\n",
  "export function shipping(cart) {\n",
  "<<<<<<< HEAD\n",
  "  return cart.total > 50 ? 0 : 4.99;\n",
  "=======\n",
  "  return cart.express ? 9.99 : 4.99;\n",
  ">>>>>>> main\n",
  "}\n",
].join("");
const MERGED = MARKED.replaceAll("<<<<<<< HEAD\n", "<<<<<<< Current\n")
  .replace("=======\n", "||||||| Base\n  return 10;\n=======\n")
  .replace(/=======\n  return cart.express/, "||||||| Base\n  return 4.99;\n=======\n  return cart.express")
  .replaceAll(">>>>>>> main\n", ">>>>>>> Incoming\n");

let file: ConflictFile = {
  text: MARKED,
  current: "current side\n",
  incoming: "incoming side\n",
  merged: MERGED,
  currentLabel: "0288795 • brigadier/912a9469/session",
  incomingLabel: "11e363f • main",
  rebasing: null,
};
const writes: Extract<SourceAction, { type: "resolve" }>[] = [];
mockIPC((command, args) => {
  if (command !== "ipc_request") return null;
  const req = (args as { request: Request }).request;
  if (req.method === "getConflictFile") return { method: req.method, file };
  if (req.method === "sourceAction" && req.action.type === "resolve") {
    writes.push(req.action);
    if (req.action.expected !== file.text) throw new Error("changed since it was opened");
    file = { ...file, text: req.action.text };
    return { method: req.method, state: { branch: "b", remote: null, upstream: null, ahead: 0, staged: [], changes: [], merging: null, rebasing: null } };
  }
  return { method: req.method };
});

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

const lenses = () => [...document.querySelectorAll('[data-slot="conflict-lens"]')].map((lens) => lens.textContent);
const button = (label: string, within: ParentNode = document) =>
  [...within.querySelectorAll("button")].find((entry) => entry.textContent === label);
const pane = (label: string) => document.querySelector(`section[aria-label="${label}"]`)!;
const tints = () => [...document.querySelectorAll('[data-slot="conflict-tab"] .font-mono > [class*="bg-"]')].map((row) => {
  const tint = [...row.classList].find((c) => c.startsWith("bg-"));
  return `${tint}:${row.textContent?.replace(/^\d+/, "")}`;
});

function Fixture() {
  const [tab, setTab] = useState<ConflictTabState>({ kind: "conflict", id: `conflict:${path}`, opener: null, path, view: "conflict" });
  const [result, setResult] = useState("");
  useEffect(() => {
    const seen: Record<string, unknown> = {};
    void (async () => {
      await until(() => lenses().length === 2);
      seen.lenses = lenses();
      seen.tints = tints();
      seen.mergeButton = !!button("Resolve in Merge Editor");
      button("Accept Incoming Change")!.click();
      await until(() => lenses().length === 1);
      seen.firstWrite = writes[0];
      seen.afterIncoming = file.text;
      button("Accept Both Changes")!.click();
      await until(() => lenses().length === 0);
      seen.afterBoth = file.text;
      seen.mergeButtonGone = !button("Resolve in Merge Editor");

      // A conflict's Compare Changes opens both sides in their own tab.
      file = { ...file, text: MARKED };
      setTab((current) => ({ ...current, id: "conflict:again" }));
      await until(() => lenses().length === 2);
      button("Compare Changes")!.click();
      seen.compareTab = sessionTabs(id).active;

      // The merge editor: the result starts with the common version, both conflicts left.
      setTab({ kind: "conflict", id: `merge:${path}`, opener: null, path, view: "merge" });
      await until(() => !!document.querySelector('[data-slot="merge-editor"]'));
      seen.panes = [...document.querySelectorAll("section[aria-label] > div:first-child")].map((head) => head.textContent);
      seen.resultLenses = [...pane("Result").querySelectorAll('[data-slot="conflict-lens"]')].map((lens) => lens.textContent);
      button("Accept Incoming", pane("Incoming"))!.click();
      await until(() => document.querySelector('[data-slot="conflicts-remaining"]')?.textContent === "1 Conflict Remaining");
      seen.afterAccept = [...pane("Result").querySelectorAll('[data-slot="conflict-lens"]')].map((lens) => lens.textContent);
      seen.incomingLenses = [...pane("Incoming").querySelectorAll('[data-slot="conflict-lens"]')].map((lens) => lens.textContent);
      seen.currentLenses = [...pane("Current").querySelectorAll('[data-slot="conflict-lens"]')].map((lens) => lens.textContent);
      // One conflict is left: Complete Merge asks first.
      button("Complete Merge")!.click();
      await until(() => !!button("Complete with Conflicts"));
      seen.asked = document.querySelector('[role="dialog"]')?.textContent;
      button("Complete with Conflicts")!.click();
      await until(() => writes.length === 3);
      seen.completed = writes[2];
      setResult(JSON.stringify(seen));
    })().catch((error: unknown) => setResult(JSON.stringify({ error: String(error), seen, lenses: lenses(), body: document.body.textContent?.slice(0, 2000) })));
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen w-full flex-col">
        <ConflictTab key={tab.id} conversationId={id} tab={tab} />
        {result && <pre id="conflict-tab-result">{result}</pre>}
      </div>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
