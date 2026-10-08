import { namedTasks, workerName } from "@/app/conversation/rowWords";
import type { OrchestratorStepKind, Task } from "@/ipc/generated";

export type ToolStep = Extract<OrchestratorStepKind, { type: "tool" }>;
export type ToolKind =
  | "read"
  | "search"
  | "list"
  | "edit"
  | "run"
  | "worker"
  | "message"
  | "memory"
  | "plan"
  | "approval"
  | "land"
  | "report"
  | "web"
  | "image"
  | "tool";
type Words = { kind: ToolKind; doing: string; done: string };
const words = (kind: ToolKind, doing: string, done: string): Words => ({
  kind,
  doing,
  done,
});

/** Providers wrap the same action in different namespaces; those are never UI copy. */
export function toolName(name: string): string {
  return (
    name
      .split(/__|[./:]/)
      .filter(Boolean)
      .at(-1) ?? ""
  );
}

const WORDS: Record<string, Words> = {
  ToolSearch: words("tool", "Loading tools", "Loaded tools"),
  tool_search: words("tool", "Loading tools", "Loaded tools"),
  query_brain: words(
    "search",
    "Searching project memory",
    "Searched project memory",
  ),
  code_search: words("search", "Searching code", "Searched code"),
  code_refs: words(
    "search",
    "Finding code references",
    "Found code references",
  ),
  project_map: words("list", "Mapping the project", "Mapped the project"),
  search_transcript: words(
    "search",
    "Searching earlier messages",
    "Searched earlier messages",
  ),
  Read: words("read", "Reading a file", "Read a file"),
  read: words("read", "Reading a file", "Read a file"),
  read_file: words("read", "Reading a file", "Read a file"),
  read_artifact: words("read", "Reading a file", "Read a file"),
  read_report: words(
    "read",
    "Reading a worker’s report",
    "Read a worker’s report",
  ),
  Grep: words("search", "Searching files", "Searched files"),
  Glob: words("list", "Finding files", "Found files"),
  LS: words("list", "Listing files", "Listed files"),
  delegate_task: words("worker", "Creating a worker", "Created a worker"),
  spawn_agent: words("worker", "Creating a worker", "Created a worker"),
  message_worker: words(
    "message",
    "Messaging a worker",
    "Sent a message to a worker",
  ),
  route_follow_up: words("message", "Sending a follow-up", "Sent a follow-up"),
  answer_worker: words("message", "Answering a worker", "Answered a worker"),
  stop_worker: words("worker", "Stopping a worker", "Stopped a worker"),
  list_tasks: words("worker", "Checking workers", "Checked workers"),
  remember: words("memory", "Saving project memory", "Saved project memory"),
  note_for_user: words("memory", "Recording a decision", "Recorded a decision"),
  plan_phases: words("plan", "Planning the work", "Planned the work"),
  propose_overnight: words("plan", "Planning the work", "Planned the work"),
  approve_outline: words("plan", "Reviewing an outline", "Reviewed an outline"),
  start_verifier: words("worker", "Starting a verifier", "Started a verifier"),
  review_code: words("approval", "Asking for a review", "Asked for a review"),
  review_plan: words("approval", "Asking for a review", "Asked for a review"),
  request_approval: words(
    "approval",
    "Asking for approval",
    "Asked for approval",
  ),
  ask_user: words("approval", "Asking a question", "Asked a question"),
  ask_orchestrator: words("message", "Asking the lead", "Asked the lead"),
  submit_outline: words("report", "Sending an outline", "Sent an outline"),
  submit_report: words("report", "Sending a report", "Sent a report"),
  land_phase: words("land", "Landing changes", "Landed changes"),
  settle_step: words("plan", "Settling a phase", "Settled a phase"),
  end_run: words("plan", "Ending the run", "Ended the run"),
  finish_session: words(
    "plan",
    "Finishing the session",
    "Finished the session",
  ),
  WebSearch: words("web", "Searching the web", "Searched the web"),
  web_search: words("web", "Searching the web", "Searched the web"),
  WebFetch: words("web", "Reading a web page", "Read a web page"),
  web_fetch: words("web", "Reading a web page", "Read a web page"),
  Bash: words("run", "Running a command", "Ran a command"),
  run: words("run", "Running a command", "Ran a command"),
  run_unsandboxed: words("run", "Running a command", "Ran a command"),
  run_check: words("run", "Running checks", "Ran checks"),
  start_preview: words("run", "Starting a preview", "Started a preview"),
  stop_preview: words("run", "Stopping a preview", "Stopped a preview"),
  preview_log: words(
    "read",
    "Reading a preview’s output",
    "Read a preview’s output",
  ),
  exec_command: words("run", "Running a command", "Ran a command"),
  shell: words("run", "Running a command", "Ran a command"),
  apply_patch: words("edit", "Editing files", "Edited files"),
  Edit: words("edit", "Editing a file", "Edited a file"),
  MultiEdit: words("edit", "Editing files", "Edited files"),
  Write: words("edit", "Writing a file", "Wrote a file"),
  NotebookEdit: words("edit", "Editing a notebook", "Edited a notebook"),
  imagegen: words("image", "Creating an image", "Created an image"),
};

/** All callers, including transcript replay, share the same semantic action vocabulary. */
export function toolActivity(name: string): Words {
  const short = toolName(name);
  const known = WORDS[short];
  if (known) return known;
  const action = short
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .replace(/[_-]+/g, " ")
    .toLowerCase();
  return action && action !== "tool"
    ? words("tool", `Calling ${action}`, `Called ${action}`)
    : words("tool", "Running an action", "Completed an action");
}

/** Only suppress a raw call when its authored result actually exists in this turn. */
const OWN_RESULT: Record<string, string> = {
  delegate_task: "created",
  start_verifier: "created",
  message_worker: "messaged",
  answer_worker: "answered",
  read_report: "readReport",
  read_artifact: "readArtifact",
  note_for_user: "decided",
  land_phase: "landed",
  WebSearch: "searchedWeb",
  web_search: "searchedWeb",
  WebFetch: "readPage",
  web_fetch: "readPage",
};

export function toolHasOwnResult(
  kind: ToolStep,
  results?: readonly {
    kind: { type: string };
    position: number;
    requestId: string | null;
  }[],
  requestId?: string | null,
  position = 0,
): boolean {
  const result = OWN_RESULT[toolName(kind.name)];
  if (kind.status !== "completed" || !result) return false;
  // The board may be replayed one event at a time. Keep the call until its result arrives.
  const nextCall =
    results?.find(
      (step) => step.kind.type === "tool" && step.position > position,
    )?.position ?? Number.POSITIVE_INFINITY;
  return !!results?.some(
    (step) =>
      step.requestId === requestId &&
      step.kind.type === result &&
      step.position >= position &&
      step.position < nextCall,
  );
}

/** A preview is named by what it is, not run as a command in the words. */
const PREVIEW = ["start_preview", "stop_preview"];

/** The past tense describes an attempted action too; its outcome never looks still active. */
export function toolWords(
  kind: ToolStep,
  tasks: Readonly<Record<string, Task>> = {},
): string {
  const activity = toolActivity(kind.name);
  let action = kind.status === "inProgress" ? activity.doing : activity.done;
  const worker = kind.detail ? tasks[kind.detail] : undefined;
  const detail = worker
    ? `“${workerName(tasks, worker)}”`
    : kind.detail
      ? namedTasks(kind.detail, tasks).replace(
          /(?<![\w/-])task-\d+(?![\w/.-])/g,
          "a worker",
        )
      : "";
  const short = toolName(kind.name);
  let target = detail;
  if (
    detail &&
    ["Read", "read", "read_file", "read_artifact"].includes(short)
  ) {
    action = `${kind.status === "inProgress" ? "Reading" : "Read"} ${detail}`;
    target = "";
  } else if (detail && activity.kind === "run" && !PREVIEW.includes(short)) {
    action = `${kind.status === "inProgress" ? "Running" : "Ran"} ${detail}`;
    target = "";
  } else if (
    detail &&
    detail !== "a worker" &&
    [
      "delegate_task",
      "spawn_agent",
      "message_worker",
      "answer_worker",
      "stop_worker",
      "read_report",
    ].includes(short)
  ) {
    action = action.replace("a worker", detail);
    target = "";
  }
  const outcome =
    kind.status === "failed"
      ? " — failed"
      : kind.status === "declined"
        ? " — stopped"
        : "";
  return `${action}${target ? `: ${target}` : ""}${outcome}`;
}
