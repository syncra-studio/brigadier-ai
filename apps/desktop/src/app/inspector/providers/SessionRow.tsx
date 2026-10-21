import { PROVIDER_LABELS, STATE_VARIANTS } from "@/app/inspector/providers/shared";
import { Badge } from "@/components/controls";
import type { RawSession } from "@/ipc/generated";
import { formatDateTime } from "@/lib/format";
import { selectRawSession } from "@/state/actions";

function sessionTitle(session: RawSession): string {
  switch (session.source.type) {
    case "live":
      return session.cwd ?? "";
    case "replay":
    case "simulation":
      return session.source.title;
  }
}

/** One raw session in the list: opening it shows its transcript. */
export function SessionRow({ session }: { session: RawSession }) {
  return (
    <li>
      <button
        type="button"
        onClick={() => selectRawSession(session.id)}
        className="hover:bg-accent h-row-sm rounded-control flex w-full items-center gap-2 px-2 text-start text-xs"
      >
        <span className="w-20 shrink-0">{PROVIDER_LABELS[session.provider]}</span>
        <Badge variant={STATE_VARIANTS[session.state]}>{session.state}</Badge>
        {session.source.type !== "live" && (
          <Badge variant="outline">{session.source.type}</Badge>
        )}
        <span className="text-muted-foreground min-w-0 flex-1 truncate font-mono">
          {sessionTitle(session)}
        </span>
        <span className="text-muted-foreground shrink-0">
          {formatDateTime(session.createdAtMs)}
        </span>
      </button>
    </li>
  );
}
