import type { ReactNode } from "react";

import { mono } from "@/components/conversation";
import type { DiffStat } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** A commit id as shown on cards. */
export const short = (commit: string) => commit.slice(0, 8);

export function DiffStatView({ stat }: { stat: DiffStat }) {
  return (
    <div className="flex flex-col gap-0.5">
      <p className="text-xs">
        {stat.files.length} file{stat.files.length === 1 ? "" : "s"} ·{" "}
        <span className="text-success">+{stat.insertions}</span>{" "}
        <span className="text-destructive">−{stat.deletions}</span>
      </p>
      <ul className={cn(mono, "flex flex-col gap-0.5")}>
        {stat.files.map((file) => (
          <li key={file.path} className="flex gap-2">
            <span className="min-w-0 flex-1 truncate">{file.path}</span>
            {file.binary ? (
              <span className="text-muted-foreground">binary</span>
            ) : (
              <span>
                <span className="text-success">+{file.insertions}</span>{" "}
                <span className="text-destructive">−{file.deletions}</span>
              </span>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

/**
 * In the thread, a decision whose card is in the composer: a grey row, e.g. "Waiting for your
 * answer". `quiet`: an approval's, its words fainter ("Awaiting approval").
 */
export function WaitingRow({ icon, quiet = false, children }: { icon: ReactNode; quiet?: boolean; children: ReactNode }) {
  return (
    <div
      data-slot="waiting-row"
      className="text-muted-foreground min-h-row-sm flex min-w-0 items-center gap-2 text-sm [&_svg]:size-icon-md [&_svg]:shrink-0"
    >
      {icon}
      <span className={cn("shimmer min-w-0 truncate", quiet && "shimmer-quiet")}>{children}</span>
    </div>
  );
}
