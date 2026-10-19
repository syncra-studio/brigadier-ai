import type { FC } from "react";

import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { ContextUsage } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** Ring geometry in its 16-unit view box: a 2-unit stroke inside it. */
const RADIUS = 7;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

/** "45k": tokens in thousands. */
function thousands(tokens: number): string {
  return `${Math.round(tokens / 1000)}k`;
}

/** The share of the context window in use (0–1) for the ring; `null` draws the track only. */
export function contextShare(usage: ContextUsage | null): number | null {
  if (!usage?.windowTokens || usage.windowTokens <= 0) return null;
  return Math.min(1, Math.max(0, usage.usedTokens / usage.windowTokens));
}

/** The ring itself: a track, and an arc for the share used. Also the `/compact` row's icon. */
export const ContextArc: FC<{ share: number | null; className?: string }> = ({
  share,
  className,
}) => (
  <svg viewBox="0 0 16 16" aria-hidden className={cn("-rotate-90", className)}>
    {/* A track that shows on the composer's surface (`muted` matched it, so a small share
        showed only its round cap, a stray dot). */}
    <circle cx="8" cy="8" r={RADIUS} fill="none" className="stroke-foreground/15 stroke-2" />
    {share !== null && share > 0 && (
      <circle
        cx="8"
        cy="8"
        r={RADIUS}
        fill="none"
        strokeLinecap="round"
        strokeDasharray={`${share * CIRCUMFERENCE} ${CIRCUMFERENCE}`}
        className="stroke-muted-foreground stroke-2"
      />
    )}
  </svg>
);

/**
 * How full the model's context is, shown by the model picker: a thin ring whose
 * arc is the share used (no number, no colour change), and on hover "Context window:",
 * "17% used (83% left)", "45k / 258k tokens used". With no known window only the tokens show.
 */
export const ContextRing: FC<{ usage: ContextUsage }> = ({ usage }) => {
  const { usedTokens, windowTokens } = usage;
  const share = contextShare(usage);
  const percent = share === null ? null : Math.round(share * 100);
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          role="img"
          aria-label={
            percent === null
              ? `Context usage: ${thousands(usedTokens)} tokens`
              : `Context usage: ${percent}%`
          }
          className="flex size-control-sm shrink-0 items-center justify-center"
        >
          <ContextArc share={share} className="size-icon-md" />
        </span>
      </TooltipTrigger>
      <TooltipContent side="top" className="flex-col gap-0.5 px-3 py-2 text-center text-sm">
        <span className="text-muted-foreground">Context window:</span>
        {percent !== null && windowTokens ? (
          <>
            <span>
              {percent}% used ({100 - percent}% left)
            </span>
            <span>
              {thousands(usedTokens)} / {thousands(windowTokens)} tokens used
            </span>
          </>
        ) : (
          <span>{thousands(usedTokens)} tokens used</span>
        )}
      </TooltipContent>
    </Tooltip>
  );
};
