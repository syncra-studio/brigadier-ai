import { BlobText } from "@/app/inspector/BlobText";
import { BriefingSectionsTable } from "@/app/inspector/BriefingSectionsTable";
import { formatTokens } from "@/app/inspector/orchestratorLog";
import { rebirthTimes, seconds, WAIT_SHOWN_MS } from "@/app/inspector/rebirthTimes";
import type { RebirthRecord } from "@/ipc/generated";

export function RebirthDetail({ record }: { record: RebirthRecord }) {
  const times = rebirthTimes(record);
  return (
    <div className="flex flex-col gap-2 px-3 pb-3">
      {times.kind === "split" && (
        <p className="text-muted-foreground tabular-nums">
          {times.note !== null &&
            (record.handoffBlob
              ? `handoff note ${seconds(times.note)} · `
              : `no handoff note (tried for ${seconds(times.note)}) · `)}
          swap {seconds(times.swap)}
          {times.waited >= WAIT_SHOWN_MS && " · the wait for the next turn is not counted"}
        </p>
      )}
      <p className="text-muted-foreground">
        {record.provider}
        {record.model && ` · ${record.model}`}
        {record.windowTokens !== null && ` · window ${formatTokens(record.windowTokens)}`}
        {" · "}
        <span className="font-mono">{record.oldNativeId ?? "?"}</span> →{" "}
        <span className="font-mono">{record.newNativeId ?? "not started yet"}</span>
      </p>
      <BriefingSectionsTable sections={record.sections} />
      {record.handoffBlob ? (
        <BlobText label="Handoff note" hash={record.handoffBlob} />
      ) : (
        <p className="text-warning">The outgoing orchestrator wrote no handoff note.</p>
      )}
      <BlobText label={`Briefing · ${formatTokens(record.briefingTokens)} tokens`} hash={record.briefingBlob} />
    </div>
  );
}
