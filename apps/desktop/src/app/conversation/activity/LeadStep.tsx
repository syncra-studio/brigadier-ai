import { type Described, StepRow } from "@/app/conversation/activity/ActivityGroup";
import { EditedFileRows, editedFiles } from "@/app/conversation/activity/EditedFiles";
import type { LeadStep } from "@/app/conversation/activity/group";
import { leadStepDetail } from "@/app/conversation/activity/leadStepDetail";
import { basename, type StepWords, toolStepWords, webSearchWords } from "@/app/conversation/activity/words";
import { useViewConversation } from "@/app/conversation/viewContext";
import type { Task } from "@/ipc/generated";
import { formatDuration } from "@/lib/format";
import { useBoard } from "@/state/board";

function hostOf(url: string): string {
  try {
    return new URL(url).host || url;
  } catch {
    return url;
  }
}

/** A lead's work step's words and how it stands. */
export function describeLeadStep(step: LeadStep, tasks: Readonly<Record<string, Task>> = {}): Described {
  const { kind } = step;
  switch (kind.type) {
    case "tool": {
      const words = toolStepWords(kind, tasks);
      // A file change names its files: its summary counts them.
      const paths = kind.item === "files" ? kind.files?.map((file) => file.path) : undefined;
      return { words: paths?.length ? { ...words, paths } : words, status: kind.status, exit: kind.exit ?? null, startedAtMs: step.atMs, endedAtMs: kind.endedAtMs, stopped: kind.stopped };
    }
    case "searchedWeb":
      return { words: webSearchWords(kind.query), status: "completed" };
    case "readPage": {
      const host = hostOf(kind.url);
      return { words: { kind: "web", doing: `Reading ${host}`, done: `Read ${host}`, web: true }, status: "completed" };
    }
    case "readArtifact": {
      const name = basename(kind.name);
      return { words: { kind: "read", doing: `Reading ${name}`, done: `Read ${name}` }, status: "completed" };
    }
    default: {
      const words: StepWords = { kind: "other", doing: "Working", done: "Worked", phrase: "Worked" };
      return { words, status: "completed" };
    }
  }
}

/** How long a finished call took, when that is a second or more: "in 41s". */
export function tookFor(step: LeadStep): string | undefined {
  if (step.kind.type !== "tool" || step.kind.endedAtMs === undefined || step.atMs === undefined) return undefined;
  const ms = step.kind.endedAtMs - step.atMs;
  return ms >= 1000 ? `in ${formatDuration(ms)}` : undefined;
}

/** One step of the lead's own work, as a row of its group; it opens to what the step did. */
export function LeadStepRow({ step }: { step: LeadStep }) {
  const tasks = useBoard((s) => s.board?.tasks);
  const conversationId = useViewConversation()?.id ?? null;
  const described = describeLeadStep(step, tasks);
  // A file change with its files counted: a row per file, each with its link and "+7 −2".
  const files = editedFiles(step);
  if (files) return <EditedFileRows step={step} files={files} conversationId={conversationId} />;
  // A search the thread made has no results kept: its row says it all, with nothing to open.
  const detail = step.kind.type === "searchedWeb" ? undefined : leadStepDetail(conversationId, step, described.words.kind);
  return <StepRow {...described} detail={detail} suffix={tookFor(step)} slot="orchestrator-step" />;
}
