import type { FC } from "react";

import { CapsulePill } from "@/app/conversation/CapsulePill";
import { mono } from "@/components/conversation";
import type { DiffStat } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** "N files changed +a −d", with each file on hover; a click opens the changes, when it can. */
export const DiffPill: FC<{ diff: DiffStat; onOpen?: () => void }> = ({ diff, onOpen }) => {
  const files = diff.files.length;
  return (
    <CapsulePill
      {...(onOpen && { label: `${files} ${files === 1 ? "file" : "files"} changed, view changes`, onClick: onOpen })}
      tip={
        <ul className={cn(mono, "flex flex-col gap-0.5")}>
          {diff.files.map((file) => (
            <li key={file.path} className="flex gap-2">
              <span className="min-w-0 flex-1 truncate">{file.path}</span>
              {file.binary ? (
                <span className="text-muted-foreground">binary</span>
              ) : (
                <span className="tabular-nums">
                  <span className="text-success">+{file.insertions}</span>{" "}
                  <span className="text-destructive">−{file.deletions}</span>
                </span>
              )}
            </li>
          ))}
        </ul>
      }
    >
      <span>
        {files} {files === 1 ? "file" : "files"} changed
      </span>
      <span className="font-mono tabular-nums">
        <span className="text-success">+{diff.insertions}</span>{" "}
        <span className="text-destructive">−{diff.deletions}</span>
      </span>
    </CapsulePill>
  );
};
