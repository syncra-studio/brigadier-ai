import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { Button } from "@/components/controls";
import type { Fixture } from "@/ipc/generated";

/** One replay fixture: what it holds, and Replay. */
export function FixtureRow({
  fixture,
  busy,
  onReplay,
}: {
  fixture: Fixture;
  busy: boolean;
  onReplay: () => void;
}) {
  return (
    <li className="flex items-center gap-2 text-xs">
      <span className="w-20 shrink-0">{PROVIDER_LABELS[fixture.provider]}</span>
      <span className="min-w-0 flex-1 truncate" title={fixture.id}>
        {fixture.title}
        <span className="text-muted-foreground">
          {" "}
          · {fixture.lines} lines
          {fixture.cliVersion && ` · v${fixture.cliVersion}`}
          {fixture.id.startsWith("builtin:") ? " · built in" : " · recorded"}
        </span>
      </span>
      <Button size="xs" variant="outline" disabled={busy} onClick={onReplay}>
        Replay
      </Button>
    </li>
  );
}
