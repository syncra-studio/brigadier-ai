import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { ProviderGlyph } from "@/components/icons";
import { CheckboxMark } from "@/components/controls";
import type { ProjectCandidate } from "@/ipc/generated";
import { formatAgo, shortPath } from "@/lib/format";
import { cn } from "@/lib/utils";

export function ProjectCard({
  candidate,
  nowMs,
  selected,
  disabled,
  onToggle,
}: {
  candidate: ProjectCandidate;
  nowMs: number;
  selected: boolean;
  disabled: boolean;
  onToggle: () => void;
}) {
  const added = candidate.projectId !== null;
  const on = added || selected;
  const activity =
    candidate.sessions > 0
      ? `${candidate.sessions} ${candidate.sessions === 1 ? "session" : "sessions"} · ${formatAgo(candidate.lastActiveMs, nowMs)}`
      : null;
  return (
    <li className="min-w-0">
      <button
        type="button"
        role="checkbox"
        aria-checked={on}
        aria-label={candidate.name}
        disabled={disabled || added}
        onClick={onToggle}
        className={cn(
          "bg-card rounded-settings flex h-full w-full min-w-0 flex-col gap-1 border p-4 text-start transition-colors",
          "enabled:hover:bg-muted/60 disabled:cursor-default",
          added && "opacity-60",
        )}
      >
        <span className="flex w-full min-w-0 items-center gap-2">
          <span className="min-w-0 flex-1 truncate text-sm font-medium">{candidate.name}</span>
          <CheckboxMark checked={on} />
        </span>
        <span className="text-muted-foreground/70 w-full truncate font-mono text-xs">{shortPath(candidate.path)}</span>
        <span className="text-muted-foreground/70 flex min-w-0 items-center gap-1.5 pt-2.5 text-xs">
          {candidate.providers.map((provider) => (
            <span key={provider} title={PROVIDER_LABELS[provider]} className="flex shrink-0">
              <ProviderGlyph provider={provider} className="size-icon-sm" />
              <span className="sr-only">{PROVIDER_LABELS[provider]}</span>
            </span>
          ))}
          <span className="truncate">{added ? "Already added" : activity}</span>
        </span>
      </button>
    </li>
  );
}
