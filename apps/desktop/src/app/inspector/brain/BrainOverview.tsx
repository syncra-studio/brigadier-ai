import { BrainDetail } from "@/app/inspector/brain/BrainDetail";
import { BrainSection } from "@/app/inspector/brain/BrainSection";
import { DeveloperActions } from "@/app/inspector/brain/DeveloperActions";
import { EmbedderSection } from "@/app/inspector/brain/EmbedderSection";
import { IndexSection } from "@/app/inspector/brain/IndexSection";
import { NodesByKindList } from "@/app/inspector/brain/NodesByKindList";
import { count } from "@/app/inspector/brain/overviewText";
import { WorkersSection } from "@/app/inspector/brain/WorkersSection";
import { useNow } from "@/hooks/use-now";
import type { BrainOverview } from "@/ipc/generated";
import { formatBytes } from "@/lib/format";

/** A project's Brain at a glance, kept live by the tab. */
export function BrainOverviewView({
  brainKey,
  overview,
  moving,
}: {
  brainKey: string;
  overview: BrainOverview;
  moving: boolean;
}) {
  const now = useNow(moving ? 1000 : 30_000);
  const { stats } = overview;
  return (
    <>
      <BrainSection title="Knowledge">
        <dl className="grid grid-cols-2 gap-x-4 gap-y-1">
          <BrainDetail label="Nodes">{count.format(stats.nodes)}</BrainDetail>
          <BrainDetail label="Edges">{count.format(stats.edges)}</BrainDetail>
          <BrainDetail label="Stale">
            <span className={stats.stale > 0 ? "text-warning" : undefined}>{count.format(stats.stale)}</span>
          </BrainDetail>
          <BrainDetail label="Without an embedding">{count.format(stats.unembedded)}</BrainDetail>
          <BrainDetail label="Transcript entries">{count.format(stats.transcriptEntries)}</BrainDetail>
          <BrainDetail label="Size on disk">{formatBytes(stats.bytesOnDisk)}</BrainDetail>
        </dl>
        <NodesByKindList stats={stats} />
      </BrainSection>
      <IndexSection index={overview.index} now={now} />
      <WorkersSection projectId={brainKey} />
      <EmbedderSection overview={overview} />
      <DeveloperActions brainKey={brainKey} projectId={brainKey} hasIndex={overview.index !== null} />
    </>
  );
}
