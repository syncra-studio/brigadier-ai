import type { CommitHow } from "@/app/side-panel/sourceTarget";
import { DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator } from "@/components/overlays";

/** The Commit button's menu: Commit, Commit (Amend), Commit & Push and Commit & Sync, each off where it can't. */
export function CommitActionsMenu({ blocked, onCommit }: { blocked: (how: CommitHow) => boolean; onCommit: (how: CommitHow) => void }) {
  return (
    <DropdownMenuContent align="end">
      <DropdownMenuItem disabled={blocked("commit")} onSelect={() => onCommit("commit")}>
        Commit
      </DropdownMenuItem>
      <DropdownMenuItem disabled={blocked("amend")} onSelect={() => onCommit("amend")}>
        Commit (Amend)
      </DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem disabled={blocked("push")} onSelect={() => onCommit("push")}>
        Commit & Push
      </DropdownMenuItem>
      <DropdownMenuItem disabled={blocked("sync")} onSelect={() => onCommit("sync")}>
        Commit & Sync
      </DropdownMenuItem>
    </DropdownMenuContent>
  );
}
