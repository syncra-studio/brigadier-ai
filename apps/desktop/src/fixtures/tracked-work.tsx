/** Production summary/history components, with IPC answered locally. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { TrackedWorkSections } from "@/app/conversation/SummaryContent";
import { PullRequestHistory } from "@/app/conversation/PullRequestHistory";
import { useTrackedWork } from "@/app/conversation/trackedWork";
import type { Request, TrackedPullRequest, TrackedWork } from "@/ipc/generated";
import { fixtureClick } from "@/fixtures/input";
import { sessionTabs } from "@/state/sessionTabs";

const now = Date.now();
const pr = (number: number, state: "open" | "merged"): TrackedPullRequest => ({ identity: { host: "github.com", owner: "org", repo: "repo", number }, account: "alice", viewer: "alice", pullRequest: { number, title: `Change ${number}`, url: `https://github.com/org/repo/pull/${number}`, state }, touchedAtMs: now - 1000, observedAtMs: now, actor: "worker 1", action: "opened", status: state === "merged" ? "Merged by Alex" : "Waiting on review", complete: true, forgotten: false });
// oxlint-disable-next-line unicorn/no-thenable -- Persisted wait metadata is text, never a callable then.
const work: TrackedWork = { pullRequests: [pr(7, "open"), pr(8, "merged")], prHistory: [], waitingOn: [{ id: "w-one", text: "Michael checks the ticket", actor: "session", createdAtMs: now - 1000, by: "2026-10-28", timezone: "Europe/Chisinau", dueAtMs: now - 1, doneAtMs: null, doneNote: null, doneNoticeDeliveredAtMs: null, wakeDeliveredAtMs: null, link: null, then: null }] };
work.prHistory = work.pullRequests.map((item) => ({ id: `event-${item.identity.number}`, atMs: now, identity: item.identity, account: "alice", actor: "GitHub", action: "status changed", detail: item.status }));
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "getTrackedWork") return { method: req.method, work: structuredClone(work) };
  if (req.method === "completeWaitingOn") {
    const wait = work.waitingOn.find((item) => item.id === req.id)!;
    wait.doneAtMs = now;
    return { method: req.method, wait };
  }
  if (req.method === "forgetPullRequest") {
    const item = work.pullRequests.find((candidate) => `${candidate.identity.host}/${candidate.identity.owner}/${candidate.identity.repo}/${candidate.identity.number}` === req.key)!;
    item.forgotten = true;
    work.prHistory.unshift({ id: "forgotten", atMs: now + 1, identity: item.identity, ...(item.account ? { account: item.account } : {}), actor: "you", action: "forgotten", detail: "History retained" });
    return { method: req.method };
  }
  return { method: req.method };
});
function App() {
  const { work: current, loading } = useTrackedWork("tracked-fixture");
  const [result, setResult] = useState<Record<string, unknown> | null>(null);
  useEffect(() => {
    if (loading || result) return;
    let stopped = false;
    const run = async () => {
      const summary = document.querySelector('[data-slot="tracked-summary"]')!;
      const initialRows = summary.querySelectorAll('[data-slot="tracked-pr-row"]').length;
      const hasOverdue = summary.textContent?.includes("Overdue");
      const historyLink = [...summary.querySelectorAll("button")].find((button) => button.textContent === "View history")!;
      await fixtureClick(historyLink);
      const historyPanel = sessionTabs("tracked-fixture").active;
      await fixtureClick(summary.querySelector('button[aria-label="Mark done: Michael checks the ticket"]')!);
      await new Promise((resolve) => setTimeout(resolve, 100));
      const waitGone = !summary.textContent?.includes("Michael checks the ticket");
      const forget = [...document.querySelectorAll('[data-slot="pr-history"] button')].find((button) => button.textContent === "Forget pull request")!;
      await fixtureClick(forget);
      await new Promise((resolve) => setTimeout(resolve, 100));
      if (!stopped) setResult({ initialRows, hasOverdue, historyPanel, waitGone, rowsAfterForget: summary.querySelectorAll('[data-slot="tracked-pr-row"]').length, historyRetained: document.querySelector('[data-slot="pr-history"]')?.textContent?.includes("Merged by Alex") });
    };
    void run().catch((error: unknown) => setResult({ error: String(error) }));
    return () => { stopped = true; };
  }, [loading, result]);
  return <div className="bg-background text-foreground flex min-h-screen gap-6 p-6">
    <div className="bg-popover w-96 shrink-0 py-2" data-slot="tracked-summary"><TrackedWorkSections sessionId="tracked-fixture" work={current} error={null} fallback={null} now={now} /></div>
    <div className="min-w-0 flex-1"><PullRequestHistory sessionId="tracked-fixture" /></div>
    {result && <pre id="tracked-work-result" hidden>{JSON.stringify(result)}</pre>}
  </div>;
}
createRoot(document.getElementById("root")!).render(<App />);
