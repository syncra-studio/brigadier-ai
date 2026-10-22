/**
 * A standalone Vite entry, excluded from the app entry and production build: the thread's code
 * blocks (conversation · code block in app.pen) in a scrolling view like the thread's. A short
 * block shows whole; a 60-line prompt, an 86-line diff and a 17-line test start folded; the last
 * block is still being written and grows a line every 300ms (`stop=1` holds it at 38 lines),
 * unpaced: paced text reveals on animation frames, which a tab in the background never runs.
 * For screenshots: `expand=1` opens the prompt, `scroll=N` scrolls the view N pixels down, `open=1`
 * opens the diff's larger view, `wrap=1` turns on the prompt's word wrap. `still=1` turns
 * animations and transitions off: a tab in the background never runs them, so a screenshot of it
 * would show them at their first frame.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { MarkdownBlock } from "@/components/conversation";
import { TooltipProvider } from "@/components/overlays";
import type { Request } from "@/ipc/generated";

const query = new URLSearchParams(location.search);
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  return { method: req.method };
});

const fence = (language: string, lines: string[]) => `\`\`\`${language}\n${lines.join("\n")}\n\`\`\``;

const SHORT = fence("bash", ["pnpm install", "pnpm tauri dev", "open http://localhost:1420"]);

const PROMPT_LINES = [
  "You are working in the brigadier repo on branch fix/queue-pause.",
  "",
  "Goal",
  'Messages sent while the queue is paused must wait in the queue with the others. Today they are sent right away and the "Paused" label stays on.',
  "",
  "What we know",
  "- The composer calls sendMessage() before it checks queue.paused, so the check runs too late.",
  '- The log shows "queue paused" a few milliseconds before the send.',
  "- It only happens when two or more workers are running in the session.",
  "- Seen on 0.1.0 nightly, macOS 27.0. Not seen on 0.0.9.",
  "",
  "Where to look",
  "- app/conversation/Composer.tsx: handleSubmit and the useQueue hook it reads.",
  "- app/state/queue.ts: pause(), resume() and the enqueue path.",
  "- app/state/session.ts: how workers subscribe to queue changes.",
];
const PROMPT = fence("text", [...PROMPT_LINES, ...Array.from({ length: 45 }, (_, at) => `- Step ${at + 16}: keep the order of waiting items.`)]);

const DIFF_LINES = [
  "diff --git a/app/state/queue.ts b/app/state/queue.ts",
  "--- a/app/state/queue.ts",
  "+++ b/app/state/queue.ts",
  "@@ -41,9 +41,22 @@ export function createQueue(send: Send) {",
  "   function enqueue(item: QueueItem) {",
  "-    items.push(item);",
  "-    void send(item);",
  "+    if (state.paused) {",
  '+      items.push({ ...item, status: "waiting" });',
  "+      return;",
  "+    }",
];
const DIFF = fence("diff", [...DIFF_LINES, ...Array.from({ length: 75 }, (_, at) => (at % 5 === 0 ? `+    resumeAt(${at});` : at % 7 === 0 ? `-    flush(${at});` : `     keep(${at});`))]);

const TEST_LINES = [
  'import { describe, expect, it, vi } from "vitest";',
  'import { createQueue } from "./queue";',
  "",
  'describe("queue", () => {',
  '  it("holds messages while paused", async () => {',
  "    const send = vi.fn().mockResolvedValue(undefined);",
  "    const queue = createQueue(send);",
  "    queue.pause();",
  '    queue.enqueue({ id: "a" });',
  '    queue.enqueue({ id: "b" });',
  "    expect(send).not.toHaveBeenCalled();",
  "    await queue.resume();",
  "    expect(send.mock.calls.map(([item]) => item.id)).toEqual([",
  '      "a", "b",',
  "    ]);",
  "  });",
  "});",
];
const TEST = fence("ts", TEST_LINES);

const SETTLED = [
  "A short block shows whole:",
  SHORT,
  "A long prompt starts folded:",
  PROMPT,
  "The paragraph after it stays in view. A diff:",
  DIFF,
  "And a test:",
  TEST,
].join("\n\n");

/** The block being written: one more line every 300ms, its fence open until the end. */
function Writing() {
  const [count, setCount] = useState(query.has("stop") ? 38 : 1);
  useEffect(() => {
    if (query.has("stop")) return;
    const timer = window.setInterval(() => setCount((before) => (before < 60 ? before + 1 : before)), 300);
    return () => window.clearInterval(timer);
  }, []);
  const lines = Array.from({ length: count }, (_, at) => TEST_LINES[at % TEST_LINES.length] ?? "");
  const done = count >= 60;
  return <MarkdownBlock text={`Writing a test now:\n\n\`\`\`ts\n${lines.join("\n")}${done ? "\n```" : ""}`} streaming={!done} paced={false} />;
}

function Page() {
  return (
    <TooltipProvider>
      <div
        data-slot="aui_thread-viewport"
        className="bg-background text-foreground flex h-screen w-full flex-col overflow-x-hidden overflow-y-auto"
      >
        <main className="max-w-thread mx-auto flex w-full flex-col gap-6 px-4 py-8">
          <MarkdownBlock text={SETTLED} />
          <Writing />
        </main>
      </div>
    </TooltipProvider>
  );
}

if (query.has("still")) {
  const still = document.createElement("style");
  still.textContent = "*, *::before, *::after { animation: none !important; transition: none !important; }";
  document.head.append(still);
}

createRoot(document.getElementById("root")!).render(<Page />);

const buttons = (name: string) =>
  Array.from(document.querySelectorAll<HTMLButtonElement>("button")).filter((button) => button.textContent?.startsWith(name));
window.setTimeout(() => {
  if (query.has("wrap")) buttons("Enable word wrap")[1]?.click();
  if (query.has("expand")) buttons("Show more")[0]?.click();
  if (query.has("open")) buttons("Open in a larger view")[2]?.click();
  window.setTimeout(() => {
    const view = document.querySelector("[data-slot=aui_thread-viewport]");
    if (view) view.scrollTop = Number(query.get("scroll") ?? 0);
  }, 1000);
}, 500);
