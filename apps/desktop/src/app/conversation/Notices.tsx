import { useState } from "react";

import { ABOVE_LINE } from "@/app/conversation/aboveLine";
import { Info, Warning, X } from "@/components/icons";
import type { Notice } from "@/ipc/generated";

/** Notices (environment problems, failed work), newest last; each can be dismissed. Limits and switches are dividers in the thread (#506). */
export function Notices({ notices }: { notices: readonly Notice[] }) {
  const [dismissed, setDismissed] = useState<ReadonlySet<number>>(new Set());
  const shown = notices.filter((notice) => !dismissed.has(notice.atMs)).slice(-3);
  if (shown.length === 0) return null;
  return (
    <ul aria-label="Notices" className="flex flex-col gap-1">
      {shown.map((notice) => (
        <li key={`${notice.atMs}:${notice.text}`} data-level={notice.level} className={ABOVE_LINE}>
          {notice.level === "warning" ? (
            <Warning aria-hidden className="text-muted-foreground size-icon-xs mt-px shrink-0" />
          ) : (
            <Info aria-hidden className="text-muted-foreground size-icon-xs mt-px shrink-0" />
          )}
          <span className="min-w-0 flex-1 whitespace-pre-wrap">{notice.text}</span>
          <button
            type="button"
            aria-label="Dismiss notice"
            className="text-muted-foreground hover:text-foreground shrink-0"
            onClick={() => setDismissed((current) => new Set(current).add(notice.atMs))}
          >
            <X className="size-icon-xs" />
          </button>
        </li>
      ))}
    </ul>
  );
}
