import { Branch } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, useEffect, useMemo, useRef, useState } from "react";

import type { SourceBranch } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/**
 * A quick pick of branches over the Source panel: the merge's "Select a branch to merge from"
 * and the checkout's "Select a branch to checkout". Typing narrows it; ↑ ↓ and Enter pick,
 * Escape or a click outside closes it. Local branches first, then remote ones, each newest
 * first, the first of each group labelled.
 */
export const BranchPicker: FC<{
  placeholder: string;
  /** Null while they load. */
  branches: SourceBranch[] | null;
  nowMs: number;
  onPick: (branch: SourceBranch) => void;
  onClose: () => void;
}> = ({ placeholder, branches, nowMs, onPick, onClose }) => {
  const [query, setQuery] = useState("");
  const [at, setAt] = useState(0);
  const root = useRef<HTMLDivElement>(null);
  const shown = useMemo(() => {
    const words = query.trim().toLowerCase();
    return (branches ?? []).filter((branch) => !words || branch.name.toLowerCase().includes(words));
  }, [branches, query]);
  useEffect(() => {
    const away = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) onClose();
    };
    window.addEventListener("pointerdown", away);
    return () => window.removeEventListener("pointerdown", away);
  }, [onClose]);
  const picked = Math.min(at, Math.max(0, shown.length - 1));
  return (
    <div
      ref={root}
      data-slot="branch-picker"
      className="bg-popover text-popover-foreground border-border rounded-surface shadow-menu absolute inset-x-2 top-1 z-20 flex max-h-96 flex-col border p-1"
    >
      <input
        autoFocus
        value={query}
        aria-label={placeholder}
        aria-controls="branch-picker-list"
        aria-activedescendant={shown[picked] ? `branch-picker-${picked}` : undefined}
        placeholder={placeholder}
        onChange={(event) => {
          setQuery(event.target.value);
          setAt(0);
        }}
        onKeyDown={(event) => {
          if (event.key === "Escape") onClose();
          else if (event.key === "ArrowDown") setAt(Math.min(picked + 1, shown.length - 1));
          else if (event.key === "ArrowUp") setAt(Math.max(picked - 1, 0));
          else if (event.key === "Enter" && shown[picked]) onPick(shown[picked]);
          else return;
          event.preventDefault();
        }}
        className="border-ring rounded-control placeholder:text-muted-foreground h-control-md shrink-0 border bg-transparent px-2 text-sm outline-none"
      />
      <div id="branch-picker-list" role="listbox" aria-label={placeholder} className="min-h-0 overflow-y-auto">
        {branches === null && <p className="text-muted-foreground px-2 py-1 text-sm">Loading…</p>}
        {branches !== null && shown.length === 0 && (
          <p className="text-muted-foreground px-2 py-1 text-sm">No matching branches</p>
        )}
        {shown.map((branch, index) => {
          const first = index === 0 || shown[index - 1]!.remote !== branch.remote;
          return (
            <div
              key={`${branch.remote}:${branch.name}`}
              id={`branch-picker-${index}`}
              role="option"
              tabIndex={-1}
              aria-selected={index === picked}
              onPointerMove={() => setAt(index)}
              onClick={() => onPick(branch)}
              onKeyDown={(event) => event.key === "Enter" && onPick(branch)}
              className={cn("rounded-control mt-1 flex cursor-default flex-col px-2 py-1", index === picked && "bg-muted")}
            >
              <span className="flex min-w-0 items-center gap-1.5 text-sm">
                <Branch className="text-muted-foreground size-icon-sm shrink-0" />
                <span className="min-w-0 truncate">{branch.name}</span>
                <span className="text-muted-foreground shrink-0 text-xs">{ago(branch.atMs, nowMs)}</span>
                <span className="flex-1" />
                {first && (
                  <span className="text-muted-foreground shrink-0 text-xs">
                    {branch.remote ? "remote branches" : "branches"}
                  </span>
                )}
              </span>
              <span className="text-muted-foreground truncate text-xs">
                {branch.author} • {branch.subject}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
};

const say = (count: number, unit: string) => `${count} ${unit}${count === 1 ? "" : "s"} ago`;

/** "5 minutes ago", "2 hours ago", "3 days ago". */
export function ago(atMs: number, nowMs: number): string {
  const minutes = Math.floor(Math.max(0, nowMs - atMs) / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return say(minutes, "minute");
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return say(hours, "hour");
  const days = Math.floor(hours / 24);
  return days < 30 ? say(days, "day") : days < 365 ? say(Math.floor(days / 30), "month") : say(Math.floor(days / 365), "year");
}
