import { unstable_useTriggerPopoverScopeContextOptional } from "@assistant-ui/react";
import type { FC } from "react";

import { fuzzyMatch } from "@/components/assistant-ui/elements/menuMatch";

/**
 * A menu row's name with what the typed query matched in full white and the rest dimmed.
 * Inside a trigger popover it reads the query; elsewhere it is plain.
 */
export const MatchedText: FC<{ text: string }> = ({ text }) => {
  const query = unstable_useTriggerPopoverScopeContextOptional()?.query ?? "";
  const match = query ? fuzzyMatch(text, query) : null;
  if (!match) return text;
  const matched = new Set(match.at);
  // Runs of matched and unmatched letters, each keyed by where it starts.
  const runs: { from: number; text: string; hit: boolean }[] = [];
  Array.from(text).forEach((letter, index) => {
    const hit = matched.has(index);
    const last = runs.at(-1);
    if (last && last.hit === hit) last.text += letter;
    else runs.push({ from: index, text: letter, hit });
  });
  return (
    <>
      {runs.map((run) => (
        <span key={run.from} className={run.hit ? "text-foreground" : "text-muted-foreground"}>
          {run.text}
        </span>
      ))}
    </>
  );
};
