import { Button } from "@/components/controls";
import { Unarchive } from "@/components/icons";

/** An archived conversation's bar above the composer: it says so, with Restore. */
export function ArchivedBar({ error, busy, onRestore }: { error: string | null; busy: boolean; onRestore: () => void }) {
  return (
    <div className="bg-muted rounded-control flex items-center gap-2 px-3 py-2 text-sm">
      <span className="min-w-0 flex-1">
        Archived. Restore it to continue.
      </span>
      {error && (
        <span role="alert" className="text-destructive text-xs">
          {error}
        </span>
      )}
      <Button size="sm" disabled={busy} onClick={onRestore}>
        <Unarchive />
        Restore
      </Button>
    </div>
  );
}
