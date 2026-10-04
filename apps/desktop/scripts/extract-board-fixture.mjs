// Builds a board fixture from a copy of a conversation's stored events, for the thread's
// tests and the fixture gallery: the latest record of each task, plan, request and run, every
// step and decision at its stream position, and the conversation's messages. Long texts are cut
// so the fixture stays small; everything the thread groups by is kept as stored.
//
//   node scripts/extract-board-fixture.mjs <copy of brigadier.db> <conversation id> <out.json>
//
// Read a copy, never the live database of a running app.
import { writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { DatabaseSync } from "node:sqlite";

const [database, conversationId, out] = process.argv.slice(2);
if (!database || !conversationId || !out) {
  console.error("usage: extract-board-fixture.mjs <db copy> <conversation id> <out.json>");
  process.exit(2);
}

const db = new DatabaseSync(database, { readOnly: true });
const rows = db
  .prepare("select stream_seq, kind, payload from events where stream = ? order by seq")
  .all(`conversation:${conversationId}`);

const cut = (text, length = 160) =>
  typeof text === "string" && text.length > length ? `${text.slice(0, length)}…` : text;

function task(raw) {
  const report = raw.report && { verdict: raw.report.verdict, checks: raw.report.checks, needsUser: [] };
  return {
    ...raw,
    spec: cut(raw.spec, 80),
    route: { ...raw.route, reason: cut(raw.route.reason, 80), explanation: null },
    attempts: [],
    workspace: null,
    outputs: [],
    kept: null,
    report: report ?? null,
    review: raw.review,
    fixes: raw.fixes.map((fix) => cut(fix, 80)),
    messages: raw.messages.map((message) => cut(message, 80)),
    blockedReason: cut(raw.blockedReason),
    error: cut(raw.error),
    candidate: raw.candidate && {
      ...raw.candidate,
      message: cut(raw.candidate.message, 80),
      excluded: [],
      diff: null,
    },
    gate: raw.gate && {
      ...raw.gate,
      findings: raw.gate.findings.map((finding) => ({ ...finding, text: cut(finding.text, 80) })),
      members: raw.gate.members.map((member) => ({
        ...member,
        result:
          member.result?.type === "failed"
            ? { ...member.result, findings: member.result.findings.map((finding) => cut(finding, 80)) }
            : member.result && "reason" in member.result
              ? { ...member.result, reason: cut(member.result.reason) }
              : member.result,
      })),
    },
  };
}

function plan(raw) {
  return {
    ...raw,
    steps: raw.steps.map((step) => ({ ...step, detail: cut(step.detail) })),
    reviewNotes: raw.reviewNotes.map((note) => cut(note)),
    responses: raw.responses.map((response) => ({
      ...response,
      finding: cut(response.finding),
      note: cut(response.note),
    })),
    gate: raw.gate && {
      ...raw.gate,
      findings: raw.gate.findings.map((finding) => ({ ...finding, text: cut(finding.text) })),
    },
  };
}

function run(raw) {
  return {
    ...raw,
    words: cut(raw.words),
    goal: cut(raw.goal),
    rules: cut(raw.rules),
    phases: raw.phases.map((phase) => ({
      ...phase,
      scope: cut(phase.scope),
      summary: cut(phase.summary),
      responses: [],
      criteria: phase.criteria.map((criterion) => ({ ...criterion, evidence: cut(criterion.evidence) })),
    })),
  };
}

const board = {
  conversationId,
  tasks: {},
  plans: {},
  requests: {},
  overnight: {},
  workerSteps: [],
  orchestratorSteps: [],
  decisions: [],
  waiting: {},
  messages: [],
};

for (const row of rows) {
  const payload = JSON.parse(row.payload);
  const position = row.stream_seq;
  switch (row.kind) {
    case "task.updated":
      board.tasks[payload.task.id] = task(payload.task);
      break;
    case "plan.updated":
      board.plans[payload.plan.id] = plan(payload.plan);
      break;
    case "request.updated":
      board.requests[payload.request.id] = payload.request;
      break;
    case "overnight.updated":
      board.overnight[payload.run.id] = run(payload.run);
      break;
    case "worker.step":
      board.workerSteps.push({ ...payload.step, position });
      break;
    case "orchestrator.step":
      board.orchestratorSteps.push({ ...payload.step, position });
      break;
    case "decision.made":
      board.decisions.push({ ...payload.decision, why: cut(payload.decision.why), position });
      break;
    case "waiting.updated":
      board.waiting[payload.item.id] = { ...payload.item, key: cut(payload.item.key, 40) };
      break;
    case "waiting.resolved":
      delete board.waiting[payload.id];
      break;
    case "message.appended":
      board.messages.push({ ...payload.message, seq: position, text: cut(payload.message.text, 400) });
      break;
  }
}

// The home folder is nobody's business in a fixture.
writeFileSync(out, `${JSON.stringify(board).replaceAll(homedir(), "~")}\n`);
console.log(
  `${Object.keys(board.tasks).length} tasks, ${board.workerSteps.length} worker steps, ` +
    `${board.decisions.length} decisions, ${board.messages.length} messages -> ${out}`,
);
