import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";
import type { TrackedPullRequest, TrackedWork, WaitingOn } from "@/ipc/generated";

const pr: TrackedPullRequest = { identity: { host: "github.com", owner: "org", repo: "repo", number: 7 }, account: "alice", viewer: "alice", pullRequest: { number: 7, title: "Changes", url: "https://github.com/org/repo/pull/7", state: "open" }, touchedAtMs: 1, observedAtMs: 2, actor: "worker 1", action: "opened", status: "Waiting on review", complete: true, forgotten: false };
// oxlint-disable-next-line unicorn/no-thenable -- This persisted wait field is text, never a callable then.
const wait: WaitingOn = { id: "w-123", text: "Michael checks ticket 2552", actor: "worker 1", createdAtMs: 1, dueAtMs: 2, timezone: "Europe/Chisinau", by: "2026-10-28", then: "Close the ticket", link: null, doneAtMs: null, doneNote: null, doneNoticeDeliveredAtMs: null, wakeDeliveredAtMs: null };

test("terminal and forgotten PRs leave live rows while their timeline remains", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { activePullRequests, activeWaits, waitingDue } = await server.ssrLoadModule("/src/app/conversation/trackedWork.ts");
    const { TrackedWorkSections } = await server.ssrLoadModule("/src/app/conversation/SummaryContent.tsx");
    const history = { id: "event-1", atMs: 3, identity: pr.identity, actor: "GitHub", action: "status changed", detail: "Merged by Alex" };
    const work: TrackedWork = { pullRequests: [pr], prHistory: [history], waitingOn: [wait] };
    assert.equal(activePullRequests(work).length, 1);
    assert.equal(activeWaits(work).length, 1);
    assert.equal(waitingDue(wait, 3), "Overdue 2026-10-28");
    for (const state of ["merged", "closed"] as const) {
      work.pullRequests = [{ ...pr, pullRequest: { ...pr.pullRequest!, state } }];
      assert.equal(activePullRequests(work).length, 0);
      const html = ReactDOMServer.renderToStaticMarkup(React.createElement(TrackedWorkSections, { sessionId: "s", work, error: null, fallback: work.pullRequests[0]!.pullRequest }));
      assert.doesNotMatch(html, /data-slot="tracked-pr-row"/);
      assert.match(html, /View history/);
      assert.match(html, /Waiting on/);
      assert.match(html, /Mark done: Michael checks ticket 2552/);
      assert.equal(work.prHistory[0]!.detail, "Merged by Alex");
    }
    work.pullRequests = [{ ...pr, forgotten: true }];
    work.waitingOn = [{ ...wait, doneAtMs: 4 }];
    assert.equal(activePullRequests(work).length, 0);
    assert.equal(activeWaits(work).length, 0);
  } finally { await server.close(); }
});
