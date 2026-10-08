import { type Described, StepRow } from "@/app/conversation/activity/ActivityGroup";
import type { LeadStep } from "@/app/conversation/activity/group";
import { basename, type StepWords, toolStepWords } from "@/app/conversation/activity/words";
import { WebSearch } from "@/components/assistant-ui/elements/web-search";
import type { Task } from "@/ipc/generated";
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
    case "tool":
      return { words: toolStepWords(kind, tasks), status: kind.status };
    case "searchedWeb":
      return { words: { kind: "web", doing: `Searching the web for ${kind.query}`, done: `Searched the web for ${kind.query}`, web: true }, status: "completed" };
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

/** One step of the lead's own work, as a row of its group. */
export function LeadStepRow({ step }: { step: LeadStep }) {
  const tasks = useBoard((s) => s.board?.tasks);
  const described = describeLeadStep(step, tasks);
  const detail = step.kind.type === "searchedWeb" ? <WebSearch query={step.kind.query} results={[]} /> : undefined;
  return <StepRow {...described} detail={detail} slot="orchestrator-step" />;
}
