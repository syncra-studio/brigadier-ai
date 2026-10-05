import type { OrchestratorStepKind } from "@/ipc/generated";

type ToolStep = Extract<OrchestratorStepKind, { type: "tool" }>;

const WORDS: Record<string, [string, string]> = {
  query_brain: ["Searching project memory", "Searched project memory"],
  code_search: ["Searching code", "Searched code"],
  code_refs: ["Finding code references", "Found code references"],
  search_transcript: ["Searching earlier messages", "Searched earlier messages"],
  Read: ["Reading a file", "Read a file"],
  read_file: ["Reading a file", "Read a file"],
  read_artifact: ["Reading a file", "Read a file"],
  read_report: ["Reading a worker’s report", "Read a worker’s report"],
  Grep: ["Searching files", "Searched files"],
  Glob: ["Finding files", "Found files"],
  delegate_task: ["Creating a worker", "Created a worker"],
  message_worker: ["Messaging a worker", "Messaged a worker"],
  answer_worker: ["Answering a worker", "Answered a worker"],
  stop_worker: ["Stopping a worker", "Stopped a worker"],
  list_tasks: ["Checking workers", "Checked workers"],
  remember: ["Saving project memory", "Saved project memory"],
  note_for_user: ["Noting a decision", "Noted a decision"],
  plan_phases: ["Planning the work", "Planned the work"],
  propose_phases: ["Planning the work", "Planned the work"],
  approve_outline: ["Reviewing an outline", "Reviewed an outline"],
  request_approval: ["Asking for approval", "Asked for approval"],
  ask_user: ["Asking a question", "Asked a question"],
  land_phase: ["Landing changes", "Landed changes"],
  phase_done: ["Checking a phase", "Checked a phase"],
  finish_session: ["Finishing the session", "Finished the session"],
  WebSearch: ["Searching the web", "Searched the web"],
  WebFetch: ["Reading a web page", "Read a web page"],
};

/** Tools whose successful result already has a worker row, card or authored action row. */
const OWN_RESULT = new Set([
  "delegate_task", "message_worker", "answer_worker", "read_report", "read_artifact",
  "note_for_user", "land_phase", "WebSearch", "WebFetch",
]);

export function toolHasOwnResult(kind: ToolStep): boolean {
  return kind.status === "completed" && OWN_RESULT.has(kind.name);
}

/** Actual tool activity, without claiming that a failed or declined call succeeded. */
export function toolWords(kind: ToolStep): string {
  const words = WORDS[kind.name] ?? ["Using a tool", "Used a tool"];
  const action = words[kind.status === "completed" ? 1 : 0];
  const detail = kind.detail ? `: ${kind.detail}` : "";
  const outcome = kind.status === "failed" ? " — failed" : kind.status === "declined" ? " — declined" : "";
  return `${action}${detail}${outcome}`;
}
