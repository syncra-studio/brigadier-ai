/** A worker's thread with computer calls, its timeline holding a recorded bench run. No daemon. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { WorkerThread } from "@/app/conversation/WorkerThread";
import { TooltipProvider } from "@/components/ui/tooltip";
import benchRun from "@/fixtures/boards/computer-bench-run.json";
import night from "@/fixtures/boards/overnight-2026-10-03.json";
import type { ComputerAction, ProviderEvent, RawEntry, Request, Task } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";

const conversationId = "computer-fixture";
const base = Object.values(night.tasks)[0] as unknown as Task;
const task: Task = {
  ...base, id: "gui", number: 1, title: "Check the target range", conversationId, state: "done", blockedReason: null,
  role: null, phase: null, run: null, attempts: [], requestId: null, gateLink: null, candidate: null, report: null,
  quotaWait: null, error: null, messages: [], spec: "Press each control in the fixture app and say what happened.",
  createdAtMs: 8000, updatedAtMs: 80000,
};
const call = (itemId: string, name: string): ProviderEvent => ({ type: "toolCall", itemId, name, input: "{}", status: "completed", output: null });
const events: ProviderEvent[] = [
  { type: "message", itemId: "m1", role: "assistant", text: "I’ll look at the fixture’s window first." },
  call("c1", "mcp__computer__apps"),
  call("c2", "mcp__computer__observe"),
  call("c3", "mcp__computer__act"),
  call("c4", "mcp__computer__act"),
  call("r1", "Read"),
  { type: "message", itemId: "m2", role: "assistant", text: "Every control answered." },
];
const entries: RawEntry[] = events.map((event, index) => ({ streamSeq: index + 1, atMs: 9000 + index * 100, event }));
useBoard.setState({ board: { ...emptyBoard(conversationId), loaded: true, tasks: { gui: task },
  transcripts: { gui: { entries, hasMore: false, loading: false } },
  computer: { gui: { actions: benchRun as ComputerAction[], earlier: null, loading: false } } } });
// A 1×1 PNG for every stored screenshot; the reads are counted.
const reads: string[] = [];
const PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "readAttachment") {
    reads.push(req.id);
    return { method: req.method, data: PNG };
  }
  return { method: req.method };
});

const q = <T extends Element>(selector: string) => document.querySelector<T>(selector);
const step = () => q('[data-slot="computer-controls"] [aria-live]')?.textContent ?? null;
const key = (name: string) => q('[data-slot="computer-controls"]')!.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true }));
const playLabel = () => q('[data-slot="computer-controls"] button:nth-of-type(2)')?.getAttribute("aria-label") ?? null;

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    const seen: Record<string, unknown> = {};
    const at = (ms: number, run: () => void) => setTimeout(run, ms);
    const timers = [
      at(100, () => {
        const thread = q('[data-slot="worker-thread"]')!;
        seen.timelines = document.querySelectorAll('[data-slot="computer-timeline"]').length;
        seen.threadText = thread.textContent;
        seen.closedSteps = document.querySelectorAll('[data-slot="computer-step"]').length;
        seen.line = q('[data-slot="computer-timeline"] button')?.textContent;
        q<HTMLButtonElement>('[data-slot="computer-timeline"] button')!.click();
      }),
      at(300, () => {
        seen.steps = document.querySelectorAll('[data-slot="computer-step"]').length;
        seen.details = document.querySelectorAll('[data-slot="computer-step-detail"]').length;
        seen.opened = step();
        seen.image = q<HTMLImageElement>('[data-slot="computer-shot"] img')?.src.startsWith("blob:") ?? false;
        // From the focused Next at the end: a key moves back and Next keeps the focus.
        const next = q<HTMLButtonElement>('[data-slot="computer-controls"] button[aria-label="Next step"]')!;
        next.focus();
        next.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }));
      }),
      at(350, () => {
        seen.endFocus = { step: step(), focused: document.activeElement === q('[data-slot="computer-controls"] button[aria-label="Next step"]') };
        key("ArrowLeft");
      }),
      at(400, () => {
        seen.left = step();
        seen.current = q('[aria-current="step"]')?.textContent;
        key("Home");
      }),
      at(500, () => {
        seen.home = step();
        seen.homeShot = q('[data-slot="computer-shot"]') ? "image" : q('[data-slot="computer-timeline"]')?.textContent?.includes("No screenshot") ? "none" : "missing";
        key("End");
      }),
      at(600, () => {
        seen.end = step();
        key("Home");
      }),
      at(700, () => {
        q<HTMLButtonElement>('[data-slot="computer-controls"] button[aria-label="Play"]')!.click();
      }),
      at(2000, () => {
        seen.playing = { step: step(), button: playLabel() };
      }),
      at(700 + 1200 * 8 + 300, () => {
        seen.played = { step: step(), button: playLabel() };
        seen.reads = [...new Set(reads)];
        setResult(JSON.stringify(seen));
      }),
    ];
    return () => timers.forEach(clearTimeout);
  }, []);
  return <TooltipProvider><main className="flex h-screen flex-col"><WorkerThread task={task} /><pre id="computer-timeline-result">{result}</pre></main></TooltipProvider>;
}
createRoot(document.getElementById("root")!).render(<Fixture />);
