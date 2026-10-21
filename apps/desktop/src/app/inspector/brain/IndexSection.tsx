import { BrainDetail } from "@/app/inspector/brain/BrainDetail";
import { BrainProgress } from "@/app/inspector/brain/BrainProgress";
import { BrainSection } from "@/app/inspector/brain/BrainSection";
import { IndexState } from "@/app/inspector/brain/IndexState";
import { count } from "@/app/inspector/brain/overviewText";
import type { IndexStatus } from "@/ipc/generated";
import { formatAgo, formatDateTime, formatMs } from "@/lib/format";

export function IndexSection({ index, now }: { index: IndexStatus | null; now: number }) {
  if (!index) {
    return (
      <BrainSection title="Code index">
        <p className="text-muted-foreground">This project has no repository to index.</p>
      </BrainSection>
    );
  }
  const languages = index.languages.toSorted((a, b) => b.files - a.files);
  return (
    <BrainSection title="Code index">
      <dl className="grid grid-cols-2 gap-x-4 gap-y-1">
        <BrainDetail label="State"><IndexState index={index} /></BrainDetail>
        <BrainDetail label="Files">{count.format(index.files)}</BrainDetail>
        <BrainDetail label="Symbols">{count.format(index.symbols)}</BrainDetail>
        <BrainDetail label="References">{count.format(index.references)}</BrainDetail>
        <BrainDetail label="Watching">{index.watching ? "yes" : "no"}</BrainDetail>
        <BrainDetail label="Last scan">
          {index.lastScanAtMs === null ? (
            "never"
          ) : (
            <span title={formatDateTime(index.lastScanAtMs)}>
              {formatAgo(index.lastScanAtMs, now)}
              {index.lastScanMs !== null && ` · ${formatMs(index.lastScanMs)}`}
            </span>
          )}
        </BrainDetail>
        {index.lastScanParsed !== null && (
          <BrainDetail label="Parsed in last scan">{count.format(index.lastScanParsed)} files</BrainDetail>
        )}
        <BrainDetail label="Last change applied">
          {index.updatedAtMs === null ? "—" : formatAgo(index.updatedAtMs, now)}
        </BrainDetail>
      </dl>
      {index.state.type === "scanning" && (
        <BrainProgress
          label="Index scan"
          value={index.state.total > 0 ? index.state.done / index.state.total : null}
        />
      )}
      {index.state.type === "failed" && (
        <p role="alert" className="text-destructive whitespace-pre-wrap">
          {index.state.error}
        </p>
      )}
      {languages.length > 0 && (
        <p className="text-muted-foreground">
          {languages.map((entry) => `${entry.language} ${count.format(entry.files)}`).join(" · ")}
        </p>
      )}
      <p className="text-muted-foreground truncate font-mono" title={index.root}>
        {index.root}
      </p>
    </BrainSection>
  );
}
