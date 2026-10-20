import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";
import { createStore } from "zustand/vanilla";

import type { RawEntry, Task } from "@/ipc/generated";

/** A worker that reported, was messaged with changes and works again (issue #208). */
const OLD = "Done, following the change of direction: the rows are cards now.";
const SENT = "Make the rows compact instead.";
const entries: RawEntry[] = [
  { type: "message", itemId: "u1", role: "user", text: "Build the rows." },
  { type: "toolCall", itemId: "r1", name: "mcp__brigadier__submit_report", input: JSON.stringify({ summary: OLD }), status: "completed", output: "ok" },
  { type: "turnCompleted", status: "completed", durationMs: 10, usage: null },
  { type: "message", itemId: "u2", role: "user", text: `Message from the orchestrator:\n[settings] Reviews are on.\n\n${SENT}` },
  { type: "turnStarted" },
  { type: "command", itemId: "c1", command: "ls src", cwd: null, status: "inProgress", exitCode: null, output: "", durationMs: null },
].map((event, index) => ({ streamSeq: index + 1, atMs: 1000 + index, event }) as unknown as RawEntry);

const task = {
  id: "t1", number: 1, title: "Rows", conversationId: "c", state: "running", spec: "Build the rows.",
  createdAtMs: 1000, updatedAtMs: 2000, attempts: [], outputs: [], messages: [SENT], reworkRounds: 1, error: null,
  report: { summary: OLD, artifacts: [], needsUser: [] },
} as unknown as Task;

test("a worker messaged after its answer keeps that answer in history, then the message, then the new turn", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  Object.assign(globalThis, { document: { documentElement: {} }, getComputedStyle: () => ({ getPropertyValue: () => "" }) });
  try {
    const { WorkerThread } = await server.ssrLoadModule("/src/app/side-panel/WorkerThread.tsx");
    const { TooltipProvider } = await server.ssrLoadModule("/src/components/overlays/tooltip.tsx");
    const { emptyBoard, BoardStoreContext } = await server.ssrLoadModule("/src/state/board.ts");
    const render = (shown: Task) => {
      const store = createStore(() => ({
        board: { ...emptyBoard("c"), loaded: true, tasks: { t1: shown }, transcripts: { t1: { entries, loading: false, hasMore: false } } },
        orchestrator: null,
      }));
      return ReactDOMServer.renderToStaticMarkup(React.createElement(BoardStoreContext.Provider, { value: store },
        React.createElement(TooltipProvider, null, React.createElement(WorkerThread, { task: shown }))));
    };
    const html = render(task);
    const at = (text: string) => html.indexOf(text);
    assert.ok(at(OLD) >= 0, "the old answer stays");
    assert.equal(html.split(OLD).length, 2, "the old answer shows once");
    assert.ok(at(SENT) > at(OLD), "the message the worker received follows its old answer");
    assert.equal(at("[settings]"), -1, "Brigadier's own notes on the message stay out");
    assert.ok(at("Listing files in src") > at(SENT), "the new turn's live step follows the message");
    assert.equal(html.split(SENT).length, 2, "the message shows once, not again under previous messages");

    // Stopped before it reported again, its old answer stays where it was, not the answer at the bottom.
    const stopped = render({ ...task, state: "stopped" } as unknown as Task);
    assert.ok(stopped.indexOf(OLD) >= 0 && stopped.indexOf(OLD) < stopped.indexOf(SENT));
    assert.equal(stopped.split(OLD).length, 2);

    // When the new turn reports, its answer is the final one at the bottom; the old one stays above.
    const NEW = "Done: the rows are compact.";
    const reported = { ...task, state: "reported", report: { summary: NEW, artifacts: [], needsUser: [] } } as unknown as Task;
    entries.push({ streamSeq: 99, atMs: 3000, event: { type: "toolCall", itemId: "r2", name: "mcp__brigadier__submit_report", input: JSON.stringify({ summary: NEW }), status: "completed", output: "ok" } } as unknown as RawEntry);
    const after = render(reported);
    // History past the last three rows folds under previous messages, the old answer with it.
    assert.match(after, /1 previous message</);
    assert.equal(after.indexOf(OLD), -1);
    assert.ok(after.indexOf(NEW) > after.indexOf("Listing files in src"), "the new answer is the last");
    assert.equal(after.split(NEW).length, 2);
  } finally {
    Reflect.deleteProperty(globalThis, "document");
    Reflect.deleteProperty(globalThis, "getComputedStyle");
    await server.close();
  }
});
