import type { StepWords } from "@/app/conversation/activity/words";
import { Clock, Globe, Stop } from "@/components/icons";

import { LIST_ICON, WORK_ICONS } from "./activity-row";

/** Which icon a step's row has, by name (the name is also on the row, for tests). */
export function stepIconName(words: StepWords, stopped = false): string {
  return stopped ? "stop" : words.web ? "web" : words.waiting ? "waiting" : words.list ? "list" : words.kind;
}

export function StepIcon({ words, stopped = false }: { words: StepWords; stopped?: boolean }) {
  const name = stepIconName(words, stopped);
  const Icon = name === "stop" ? Stop : name === "web" ? Globe : name === "waiting" ? Clock : name === "list" ? LIST_ICON : WORK_ICONS[words.kind];
  return (
    <span data-icon={name} className="contents">
      <Icon aria-hidden className="size-4 shrink-0" />
    </span>
  );
}
