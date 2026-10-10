/**
 * The mockup tab (SESSION-FIXES-PLAN.md Q10) on synthetic board data; no daemon. Screenshots
 * are drawn here; `stage=building|ready|compared|stopped`, `full=1` (full view), `comment=1`
 * (two notes made), `drive=1` (the rendering test's flow, its result in #mockup-result).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { MockupChip } from "@/app/conversation/MockupChip";
import { RightSidebar, RightSidebarContext } from "@/app/conversation/RightSidebar";
import { SessionTabViews } from "@/app/conversation/SessionTabBar";
import { useSidePanel } from "@/app/conversation/SidePanel";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { ArtifactRef, Request, Task } from "@/ipc/generated";
import { tokenColor } from "@/lib/tokens";
import { emptyBoard, useBoard } from "@/state/board";
import { changeNotes, mockupNotesOf } from "@/state/mockupNotes";
import { openMockupTab, sessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const id = "mockup-fixture";
const params = new URLSearchParams(location.search);
const stage = params.get("stage") ?? "ready";
const now = Date.now();
const WIDTH = 1280;
const HEIGHT = 820;
/** The drawn screenshots' type, in image pixels. */
const font = (size: number, weight = "") => `${weight} ${size}px -apple-system, system-ui, sans-serif`;

/** A fake app window: a sidebar of rows with a density toggle at its foot, then a thread. */
function draw(kind: "mockup" | "live", screen: number, version: number): HTMLCanvasElement {
  const canvas = document.createElement("canvas");
  canvas.width = WIDTH;
  canvas.height = HEIGHT;
  const g = canvas.getContext("2d")!;
  const [page, side, text, muted, card, blue] = ["--background", "--sidebar", "--foreground", "--muted-foreground", "--card", "--marker"].map((name) => tokenColor(name as `--${string}`));
  g.fillStyle = page!; g.fillRect(0, 0, WIDTH, HEIGHT);
  g.fillStyle = side!; g.fillRect(0, 0, 280, HEIGHT);
  g.font = font(22, "bold"); g.fillStyle = text!; g.fillText("Brigadier", 28, 60);
  const compact = screen === 2;
  const row = compact ? 26 : 34;
  g.font = font(compact ? 13 : 15);
  for (let i = 0; i < 12; i++) { g.fillStyle = i === 1 ? card! : side!; g.fillRect(12, 100 + i * row, 256, row - 4); g.fillStyle = muted!; g.fillText(`Session ${i + 1}`, 28, 100 + i * row + row / 2 + 5); }
  g.fillStyle = card!; g.beginPath(); g.roundRect(20, HEIGHT - 70, 240, 44, 12); g.fill();
  g.fillStyle = text!; g.fillText("Compact rows", 36, HEIGHT - 42);
  g.fillStyle = compact ? blue! : muted!; g.beginPath(); g.roundRect(204, HEIGHT - 60, 40, 24, 12); g.fill();
  g.fillStyle = text!; g.beginPath(); g.arc(compact ? 232 : 216, HEIGHT - 48, 9, 0, Math.PI * 2); g.fill();
  g.fillStyle = card!; g.beginPath(); g.roundRect(380, 60, 800, 120 + version * 10, 20); g.fill();
  g.font = font(16); g.fillStyle = text!;
  g.fillText(kind === "mockup" ? `Mockup, screen ${screen}, version ${version}` : `Live app, screen ${screen}`, 410, 110);
  return canvas;
}

const png = (canvas: HTMLCanvasElement) => canvas.toDataURL("image/png").split(",")[1]!;

function sideBySide(screen: number): string {
  const canvas = document.createElement("canvas");
  canvas.width = WIDTH * 2 + 40;
  canvas.height = HEIGHT;
  const g = canvas.getContext("2d")!;
  g.fillStyle = tokenColor("--card"); g.fillRect(0, 0, canvas.width, canvas.height);
  g.drawImage(draw("mockup", screen, 1), 0, 0);
  g.drawImage(draw("live", screen, 1), WIDTH + 40, 0);
  return png(canvas);
}

const images: Record<string, string> = {};
for (const screen of [1, 2]) {
  images[`mockup-${screen}-v1`] = png(draw("mockup", screen, 1));
  images[`mockup-${screen}-v2`] = png(draw("mockup", screen, 2));
  images[`live-${screen}`] = png(draw("live", screen, 1));
  images[`compare-${screen}`] = sideBySide(screen);
}

const ref = (name: string, hash: string): ArtifactRef => ({ id: hash, title: name, kind: "screenshot", mime: "image/png", bytes: 1, fileName: name });
const task = (number: number, change: Partial<Task>): Task => ({
  id: `task-${number}`, conversationId: id, number, title: "Density toggle mockup", kind: "implement", state: "reported",
  outputs: [], reworkRounds: 0, createdAtMs: now - 600000, updatedAtMs: now, ...change,
}) as Task;

const shots = (version: number) => [1, 2].map((screen) => ref(`mockup-${screen}.png`, `mockup-${screen}-v${version}`));
const tasks: Task[] = stage === "building" ? [task(1, { mockup: { type: "builds" }, state: "running" })]
  : stage === "stopped" ? [task(1, { mockup: { type: "builds" }, state: "stopped" })]
    : stage === "compared" ? [
      task(1, { mockup: { type: "builds" }, state: "landed", outputs: shots(1) }),
      task(2, { title: "Compact density toggle", mockup: { type: "matches", task: "task-1" }, state: "reported",
        outputs: [1, 2].flatMap((screen) => [ref(`live-${screen}.png`, `live-${screen}`), ref(`compare-${screen}.png`, `compare-${screen}`)]) }),
    ]
      : [task(1, { mockup: { type: "builds" }, outputs: shots(1) })];
useBoard.setState({ board: { ...emptyBoard(id), loaded: true, tasks: Object.fromEntries(tasks.map((t) => [t.id, t])) } });
useApp.setState({
  info: { platform: "macos", version: "fixture", processStartMs: now, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { status: "connected", daemon: null, reason: null },
});
if (params.has("comment")) changeNotes(id, () => ({ commenting: true, notes: [
  { id: "n1", screen: 1, x: 0.18, y: 0.92, text: "Put the toggle in the header instead" },
  { id: "n2", screen: 1, x: 0.62, y: 0.12, text: "Keep this card's height" },
] }));
if (params.has("full")) openMockupTab(id);

const calls: Request[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  calls.push(req);
  if (req.method === "readAttachment") return { method: req.method, data: images[req.id] ?? "" };
  if (req.method === "addAttachment") return { method: req.method, attachment: { id: `att-${req.name}`, name: req.name, mime: req.mime, bytes: req.data.length, pasted: false, inline: null } };
  if (req.method === "sendMessage") return { method: req.method, outcome: { type: "queued", item: { id: "q1", text: req.text, attachments: req.attachments, mentions: [], createdAtMs: now } } };
  return { method: req.method };
});

const button = (label: string) => [...document.querySelectorAll<HTMLButtonElement>("button")].find((b) => (b.getAttribute("aria-label") ?? b.textContent?.trim()) === label);
const pointer = (target: Element, type: string, x: number, y: number) => {
  const box = target.getBoundingClientRect();
  target.dispatchEvent(new PointerEvent(type, { bubbles: true, clientX: box.left + box.width * x, clientY: box.top + box.height * y, button: 0, pointerId: 1 }));
};
const type = (input: HTMLInputElement, value: string) => {
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
};

function Fixture() {
  const { panel } = useSidePanel(id, true);
  const [result, setResult] = useState("");
  const { openTab } = panel;
  useEffect(() => { if (!params.has("full")) openTab("mockup"); }, [openTab]);
  useEffect(() => {
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    const timers: ReturnType<typeof setTimeout>[] = [];
    const at = (ms: number, action: () => void) => timers.push(setTimeout(action, ms));
    const surface = () => document.querySelector('[data-slot="mockup-surface"]')!;
    at(1500, () => {
      seen.chip = document.querySelector('[data-slot="mockup-chip"]')?.textContent;
      seen.pill = button("File actions for Density toggle mockup")?.textContent;
      seen.zoom = document.querySelector('[aria-label^="Current zoom"]')?.textContent;
      seen.tabs = [...document.querySelectorAll('[aria-label="Right sidebar"] [role="tab"]')].map((tab) => tab.getAttribute("aria-label"));
      button("Comment")?.click();
    });
    at(1800, () => {
      seen.helper = document.querySelector('[aria-label="Image editing tools"]')?.textContent;
      pointer(surface(), "pointerdown", 0.3, 0.4);
      pointer(surface(), "pointerup", 0.3, 0.4);
    });
    at(2100, () => {
      const input = document.querySelector<HTMLInputElement>('[data-slot="mockup-note"] input')!;
      seen.placeholder = input.placeholder;
      type(input, "Make this card blue");
    });
    at(2300, () => {
      button("Add comment")?.click();
    });
    at(2500, () => {
      pointer(surface(), "pointerdown", 0.85, 0.85);
      pointer(surface(), "pointerup", 0.85, 0.85);
    });
    at(2800, () => { type(document.querySelector<HTMLInputElement>('[data-slot="mockup-note"] input')!, "Taller footer band"); });
    at(3000, () => { button("Add comment")?.click(); });
    at(3200, () => {
      seen.markers = [...document.querySelectorAll('[aria-label^="Edit comment"]')].map((marker) => marker.textContent);
      seen.count = document.querySelector('[aria-label="Image editing tools"]')?.textContent;
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", metaKey: true, bubbles: true }));
    });
    // Marking and sending decode and encode images, which take real time: wait for the send.
    const until = (done: () => boolean, then: () => void, tries = 400) => at(50, () => (done() || tries === 0 ? then() : until(done, then, tries - 1)));
    at(3300, () => until(() => calls.some((call) => call.method === "sendMessage"), () => {
      seen.attachments = calls.filter((call) => call.method === "addAttachment").map((call) => call.method === "addAttachment" && call.name);
      const sent = calls.find((call) => call.method === "sendMessage");
      seen.message = sent?.method === "sendMessage" ? sent.text : null;
      seen.sentTo = sent?.method === "sendMessage" ? sent.conversationId : null;
      at(200, () => {
        seen.working = document.querySelector('[aria-label="Image editing tools"]')?.textContent;
        seen.notesLeft = mockupNotesOf(id).notes.length;
        // The worker revises the screenshots under the same names and reports again.
        const board = useBoard.getState().board!;
        useBoard.setState({ board: { ...board, tasks: { ...board.tasks, "task-1": { ...board.tasks["task-1"]!, outputs: shots(2), reworkRounds: 1 } } } });
      });
      at(600, () => {
        seen.revised = document.querySelector('[aria-label="Image editing tools"]')?.textContent;
        button("See latest changes")?.click();
      });
      at(1000, () => {
        seen.after = button("File actions for Density toggle mockup")?.textContent;
        button("Enter full view")?.click();
      });
      at(1400, () => {
        seen.fullTab = sessionTabs(id).active;
        seen.fullShown = Boolean(document.querySelector('[role="tabpanel"][aria-label="Mockup"] [data-slot="mockup"]'));
        button("Exit full view")?.click();
      });
      at(1800, () => {
        seen.exited = { tabs: sessionTabs(id).tabs.length, sidebar: document.querySelector('[data-slot="right-sidebar"]')?.getAttribute("data-state") };
        setResult(JSON.stringify(seen));
      });
    }));
    return () => timers.forEach(clearTimeout);
  }, []);
  return (
    <RightSidebarContext.Provider value={panel}>
      <main className="bg-background text-foreground flex h-screen w-full min-w-0">
        <div className="relative flex min-w-0 flex-1 flex-col">
          <header className="h-titlebar flex shrink-0 items-center justify-between gap-3 px-5">
            <span className="text-sm font-medium">Add a compact density toggle to the sidebar</span>
            <MockupChip conversationId={id} />
          </header>
          <div className="relative min-h-0 flex-1">
            <div className="mx-auto max-w-xl space-y-4 px-6 py-12 text-sm">
              <p>Here is a mockup of the compact density toggle: two screens, the sidebar as it is and with compact rows.</p>
              <p className="text-muted-foreground">Mark anything to change on the screenshots, then send your notes.</p>
            </div>
            <SessionTabViews conversationId={id} />
          </div>
        </div>
        <RightSidebar conversationId={id} />
      </main>
      {result && <pre id="mockup-result">{result}</pre>}
    </RightSidebarContext.Provider>
  );
}
createRoot(document.getElementById("root")!).render(<TooltipProvider><SidebarProvider defaultOpen={false}><Fixture /></SidebarProvider></TooltipProvider>);
