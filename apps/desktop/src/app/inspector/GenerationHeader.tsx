import { formatTokens, type Generation } from "@/app/inspector/orchestratorLog";
import { formatClock } from "@/lib/format";

export function GenerationHeader({ generation, injections }: { generation: Generation; injections: number }) {
  const { rebirth } = generation;
  return (
    <div className="h-row-sm bg-muted/40 text-muted-foreground flex items-center gap-2 px-3 text-xs">
      <span className="text-foreground font-medium">Generation {generation.generation + 1}</span>
      <span className="min-w-0 flex-1 truncate">
        {rebirth
          ? `reborn ${formatClock(rebirth.atMs)} at ${formatTokens(rebirth.record.atTokens)} tokens · briefing ${formatTokens(rebirth.record.briefingTokens)}`
          : "the first CLI session"}
      </span>
      <span className="shrink-0 tabular-nums">{injections} injections</span>
    </div>
  );
}
