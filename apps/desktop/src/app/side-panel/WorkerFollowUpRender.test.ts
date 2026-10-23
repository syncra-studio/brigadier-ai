import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";
import type { RawEntry, ThreadView, WorkerRow } from "@/ipc/generated";

/** A worker that answered, was messaged with changes and works again (issue #208). */
const OLD = "Done, following the change of direction: the rows are cards now.";
const SENT = "Make the rows compact instead.";
const entries: RawEntry[] = [
  { type: "message", itemId: "u1", role: "user", text: "Build the rows." },
  { type: "message", itemId: "r1", role: "assistant", text: OLD },
  { type: "turnCompleted", status: "completed", durationMs: 10, usage: null },
  { type: "message", itemId: "u2", role: "user", text: `Message from the orchestrator:\n[settings] Reviews are on.\n\n${SENT}` },
  { type: "turnStarted" },
  { type: "command", itemId: "c1", command: "ls src", cwd: null, status: "inProgress", exitCode: null, output: "", durationMs: null },
].map((event, index) => ({ streamSeq: index + 1, atMs: 1000 + index, event }) as unknown as RawEntry);

const row: WorkerRow = {
  id: "t1", number: 1, title: "Rows", agent: "claude", model: null, startedAtMs: 1000, working: true, lastEnd: "done", gone: false,
  lastEventAtMs: 2000, limit: null, freshContext: false, waiting: [], mcp: [], branch: null, base: null, diff: null, ahead: 0, merged: false,
};
/** Its thread as `workerThread` reads it from its CLI's files. */
const thread = (): ThreadView => ({ entries: entries.map((entry) => ({ event: entry.event, atMs: entry.atMs })), turns: [] });

test("a worker messaged after its answer keeps that answer in history, then the message, then the new turn", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  Object.assign(globalThis, { document: { documentElement: {} }, getComputedStyle: () => ({ getPropertyValue: () => "" }) });
  try {
    const { WorkerThread } = await server.ssrLoadModule("/src/app/side-panel/WorkerThread.tsx");
    const { TooltipProvider } = await server.ssrLoadModule("/src/components/overlays/tooltip.tsx");
    const { useWorkerThreads } = await server.ssrLoadModule("/src/state/cliThread.ts");
    const render = (shown: WorkerRow) => {
      // A server render reads a store's first state.
      Object.assign(useWorkerThreads.getInitialState().views, { t1: thread() });
      return ReactDOMServer.renderToStaticMarkup(React.createElement(TooltipProvider, null, React.createElement(WorkerThread, { conversationId: "c", row: shown })));
    };
    const html = render(row);
    const at = (text: string) => html.indexOf(text);
    assert.ok(at(OLD) >= 0, "the old answer stays");
    assert.equal(html.split(OLD).length, 2, "the old answer shows once");
    assert.ok(at(SENT) > at(OLD), "the message the worker received follows its old answer");
    // Its brief is its first bubble, not a message it was sent.
    assert.match(html, /data-slot="worker-brief"[^]*Build the rows\./);
    assert.ok(at("Listing files in src") > at(SENT), "the new turn's live step follows the message");
    assert.equal(html.split(SENT).length, 2, "the message shows once, not again under previous messages");

    // Stopped before it reported again, its old answer stays where it was, not the answer at the bottom.
    const stopped = render({ ...row, working: false, lastEnd: "stopped" });
    assert.ok(stopped.indexOf(OLD) >= 0 && stopped.indexOf(OLD) < stopped.indexOf(SENT));
    assert.equal(stopped.split(OLD).length, 2);

    // When the new turn reports, its answer is the final one at the bottom; the old one stays above.
    const NEW = "Done: the rows are compact.";
    const reported: WorkerRow = { ...row, working: false, lastEnd: "done" };
    entries.push({ streamSeq: 99, atMs: 3000, event: { type: "message", itemId: "r2", role: "assistant", text: NEW } } as unknown as RawEntry);
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
