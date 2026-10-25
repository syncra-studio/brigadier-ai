/**
 * The notices tray on its own (D51): an account notice is dismissed with ×, then the tray is
 * drawn anew from a fresh read of the conversation, the way switching to another view and back
 * (or an app restart) draws it; then the problem really happens again. `?drive=1` writes what it
 * saw into `#notice-dismiss-result`. No daemon: a stand-in keeps the conversation's notices and
 * says each change to the board as the daemon's events would.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { Notices } from "@/app/conversation/Notices";
import { TooltipProvider } from "@/components/overlays";
import type { DomainEvent, Notice, Request } from "@/ipc/generated";
import { applyBoardEvents, emptyBoard, useBoard } from "@/state/board";

const ID = "notice-session";
const ACCOUNT =
  "Your organization has disabled Claude subscription access for Claude Code · Use an Anthropic API key instead, or ask your admin to enable access";

/** What the daemon keeps of the conversation's notices, and what it was asked. */
let stored: Notice[] = [{ level: "warning", text: ACCOUNT, atMs: Date.now() - 60_000 }];
const calls: Request[] = [];
let seq = 0;

function say(event: DomainEvent): void {
  seq += 1;
  applyBoardEvents([{ seq, stream: `conversation:${ID}`, streamSeq: seq, atMs: Date.now(), event }]);
}

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  calls.push(req);
  if (req.method === "threadView") return { method: req.method, view: { entries: [], turns: [] } };
  if (req.method === "dismissNotice") {
    stored = stored.filter((notice) => notice.atMs !== req.atMs);
    say({ type: "noticeDismissed", conversationId: ID, atMs: req.atMs });
  }
  return { method: req.method };
});

/** The conversation's board as a fresh read of it gives. */
function read(): void {
  useBoard.setState({ board: { ...emptyBoard(ID), loaded: true, notices: stored } });
}

const root = createRoot(document.getElementById("root")!);

/** Draws the tray, the `drawn`th time a new one. */
function draw(drawn: number): void {
  root.render(
    <TooltipProvider>
      <div className="bg-background text-foreground w-160 p-4">
        <Notices key={drawn} conversationId={ID} />
      </div>
    </TooltipProvider>,
  );
}

read();
draw(0);

const pause = (ms = 100) => new Promise((resolve) => setTimeout(resolve, ms));
const shown = () => [...document.querySelectorAll('[aria-label="Notices"] li[data-level] [data-slot="notice-text"]')].map((item) => item.textContent);

async function drive() {
  for (let tries = 0; tries < 50 && shown().length === 0; tries += 1) await pause();
  const seen: Record<string, unknown> = { before: shown() };
  document.querySelector<HTMLButtonElement>('[aria-label="Notices"] li[data-level] button')?.click();
  await pause();
  seen.dismissed = shown();
  // Drawn anew from a fresh read, as on switching back to the session.
  read();
  draw(1);
  await pause(300);
  seen.reread = shown();
  // A later turn hits the same problem again: a new notice.
  const again: Notice = { level: "warning", text: ACCOUNT, atMs: Date.now() };
  stored = [...stored, again];
  say({ type: "conversationNotice", conversationId: ID, notice: again });
  await pause(300);
  seen.again = shown();
  seen.dismissCalls = calls.filter((call) => call.method === "dismissNotice").length;
  const result = document.createElement("pre");
  result.id = "notice-dismiss-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

if (new URLSearchParams(location.search).has("drive")) void drive();
