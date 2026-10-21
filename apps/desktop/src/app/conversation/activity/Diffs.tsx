import { InlineDiff } from "@/app/conversation/activity/InlineDiff";

/**
 * Several files' changes, each its own card; an edit's several places in one file are one card,
 * "⋯" between them.
 */
export function Diffs({ files }: { files: readonly { path: string; diff: string }[] }) {
  const byPath = new Map<string, string[]>();
  for (const file of files) byPath.set(file.path, [...(byPath.get(file.path) ?? []), file.diff]);
  return (
    <div className="flex flex-col gap-1">
      {[...byPath].map(([path, diffs]) => (
        <InlineDiff key={path} path={path} diff={diffs.join("\n@@\n")} />
      ))}
    </div>
  );
}
