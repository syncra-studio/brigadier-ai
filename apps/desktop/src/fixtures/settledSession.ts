/**
 * What this build makes of a session recorded before endings changed (THREAD-PARITY-PLAN §5
 * Q6, Q9), for the fixture page and the thread's tests:
 * - a session lists nothing for the user, so its "Waiting on you" items go, and a request that
 *   waited only on them (or on a question in the thread's text) is done;
 * - the merge the thread asked in its text is asked once on a card instead, under the last
 *   request, open.
 * The thread's own replies stay as recorded: the shorter ending its new instructions ask for
 * is not something a recording can show.
 */
import type { EventEnvelope, Question } from "@/ipc/generated";

/** The merge card the thread opens with `propose_merge`, as the daemon builds it. */
function mergeCard(conversationId: string, requestId: string, branch: string, base: string, atMs: number): Question {
  return {
    id: "fixture-merge-card",
    conversationId,
    taskId: null,
    requestId,
    position: Number.MAX_SAFE_INTEGER,
    kind: { type: "merge", branch, base },
    text: `Merge \`${branch}\` into \`${base}\`?`,
    options: [`Merge into ${base}`, "Not yet"],
    recommended: null,
    items: [
      {
        text: `Merge \`${branch}\` into \`${base}\`?`,
        options: [
          { label: `Merge into ${base}`, description: "Brings the right sidebar, the tabs and the review fixes." },
          { label: "Not yet", description: `The work stays on \`${branch}\`.` },
        ],
        recommended: null,
      },
    ],
    answer: null,
    answers: [],
    createdAtMs: atMs,
    answeredAtMs: null,
  };
}

export function settledSession(
  conversationId: string,
  events: readonly EventEnvelope[],
  merge: { branch: string; base: string } | null,
): EventEnvelope[] {
  const settled: EventEnvelope[] = [];
  let lastRequest: string | null = null;
  for (const envelope of events) {
    const { event } = envelope;
    if (event.type === "waitingOnYou" || event.type === "waitingResolved") continue;
    if (event.type === "requestUpdated") {
      lastRequest = event.request.id;
      if (event.request.state.type === "waiting") {
        const request = { ...event.request, state: { type: "done" as const }, endedAtMs: envelope.atMs };
        settled.push({ ...envelope, event: { ...event, request } });
        continue;
      }
    }
    settled.push(envelope);
  }
  const last = settled.at(-1);
  if (merge && last && lastRequest) {
    const question = mergeCard(conversationId, lastRequest, merge.branch, merge.base, last.atMs);
    settled.push({ ...last, seq: last.seq + 1, streamSeq: last.streamSeq + 1, event: { type: "questionUpdated", question } });
  }
  return settled;
}
