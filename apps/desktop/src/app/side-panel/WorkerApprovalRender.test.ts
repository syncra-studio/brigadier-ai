import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

import type { ApprovalDecision, ApprovalRequest, Decider } from "@/ipc/generated";

const request: ApprovalRequest = {
  id: "ap1", kind: "command", tool: "Bash", command: "touch /tmp/outside.txt", cwd: null, paths: [], reason: null, escalation: false, input: null, grant: null,
};
const approval = (resolution: { decision: ApprovalDecision; decidedBy: Decider } | null) =>
  ({ kind: "item", item: { kind: "approval", key: "approval:ap1", request, resolution } }) as const;

test("a worker's permission prompt shows in its thread as a read-only row: waiting on its orchestrator, then what it answered (D48)", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { EntryView } = await server.ssrLoadModule("/src/app/side-panel/EntryView.tsx");
    const { workerActivity } = await server.ssrLoadModule("/src/app/conversation/activity/group.ts");
    const shown = (entry: ReturnType<typeof approval>) => {
      const html = ReactDOMServer.renderToStaticMarkup(React.createElement(EntryView, { entry, sent: [] }));
      assert.doesNotMatch(html, /<button/, "never a card with buttons");
      return /data-slot="worker-approval"[^>]*>.*<span[^>]*>([^<]*)<\/span>/.exec(html)?.[1] ?? null;
    };
    assert.equal(shown(approval(null)), "Waiting for the orchestrator: touch /tmp/outside.txt");
    assert.equal(shown(approval({ decision: { type: "allow" }, decidedBy: "orchestrator" })), "Allowed by orchestrator: touch /tmp/outside.txt");
    assert.equal(shown(approval({ decision: { type: "deny", message: "outside its worktree" }, decidedBy: "orchestrator" })), "Denied by orchestrator: outside its worktree");
    assert.equal(shown(approval({ decision: { type: "deny", message: "" }, decidedBy: "orchestrator" })), "Denied by orchestrator");
    // Answered by anyone else, the command's own row tells what came of it.
    assert.equal(shown(approval({ decision: { type: "allow" }, decidedBy: "policy" })), null);
    // The orchestrator's answer stays a row of the worker's thread.
    const rows = workerActivity([approval({ decision: { type: "allow" }, decidedBy: "orchestrator" })]);
    assert.equal(rows.length, 1);
    assert.equal(workerActivity([approval({ decision: { type: "allow" }, decidedBy: "policy" })]).length, 0);
  } finally {
    await server.close();
  }
});
