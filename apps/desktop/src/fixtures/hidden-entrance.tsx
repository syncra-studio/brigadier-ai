/**
 * A standalone Vite entry, excluded from the app entry and production build: an approval in the
 * composer's place (ApprovalSlot) and a decision card on the composer rail, mounted while the
 * page is hidden and again while it shows, with the page-hidden mark the app sets at startup.
 *
 * `?drive=1` writes JSON into `#hidden-entrance-result` (`pageHidden.test.ts` checks it). In a
 * hidden or covered macOS window WebKit runs no animations, so an entrance animation started then
 * is held at its first frame until the window shows again; here every animation is paused instead
 * ("frozen"), and `document.hidden` is faked. For each card as it mounted: its entrance
 * animations, its opacity and its height.
 * - `hidden`: hidden, frames frozen: the cards are simply there;
 * - `shown`: visible, frames frozen, the control: each opens with its animation, at its first
 *   frame here, as the covered window showed it.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ApprovalSlot } from "@/app/conversation/ActionCards";
import { ComposerRail, ComposerRailItem } from "@/components/conversation";
import { TooltipProvider } from "@/components/overlays";
import { trackPageHidden } from "@/lib/pageHidden";
import { emptyBoard, type ShownApproval, useBoard } from "@/state/board";

trackPageHidden();

const conversationId = "hidden-entrance";
const approval: ShownApproval = {
  id: "ap", conversationId, taskId: null, requestId: null, position: 0, state: { type: "pending" }, createdAtMs: 0, resolvedAtMs: null,
  subject: { type: "cli", request: { id: "ap", kind: "command", tool: "Bash", command: "git push origin main", cwd: null, paths: [], reason: null, escalation: false, input: null, grant: "git push" } },
};
useBoard.setState({ board: { ...emptyBoard(conversationId), loaded: true, approvals: { ap: approval } } });
mockIPC((command, payload) => (command === "ipc_request" ? { method: (payload as { request: { method: string } }).request.method } : null));

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const frame = () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));

function setHidden(hidden: boolean) {
  Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
  document.dispatchEvent(new Event("visibilitychange"));
}

let mount: (round: number | null) => void = () => {};

function seen(selector: string) {
  const element = document.querySelector<HTMLElement>(selector);
  if (!element) return null;
  return {
    animations: element.getAnimations().length,
    opacity: getComputedStyle(element).opacity,
    height: Math.round(element.getBoundingClientRect().height),
  };
}

async function drive() {
  // The rail has been there a while, as a conversation's has: what comes now opens.
  await wait(600);
  const freeze = document.createElement("style");
  freeze.textContent = "*, *::before, *::after { animation-play-state: paused !important; }";
  document.head.append(freeze);
  const result: Record<string, unknown> = {};
  for (const [round, hidden] of [[1, true], [2, false]] as const) {
    setHidden(hidden);
    mount(round);
    await frame();
    result[hidden ? "hidden" : "shown"] = {
      slot: seen('[data-slot="approval-slot"]'),
      rail: seen('[data-slot="composer-rail-item"]'),
    };
    mount(null);
    await frame();
  }
  setHidden(false);
  freeze.remove();
  const out = document.createElement("pre");
  out.id = "hidden-entrance-result";
  out.textContent = JSON.stringify(result);
  document.body.append(out);
}

function Fixture() {
  const [round, setRound] = useState<number | null>(null);
  useEffect(() => {
    mount = setRound;
    if (new URLSearchParams(location.search).has("drive")) void drive();
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex w-180 flex-col gap-6 p-6">
        {round !== null && <ApprovalSlot key={round} ids={["ap"]} onReply={() => {}} />}
        <ComposerRail>
          {round !== null && (
            <ComposerRailItem key={round} variant="decision" label="Waiting for you">
              <p className="p-4 text-sm">Allow this command?</p>
            </ComposerRailItem>
          )}
        </ComposerRail>
      </div>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
