import { BrainDetail } from "@/app/inspector/brain/BrainDetail";
import { BrainProgress } from "@/app/inspector/brain/BrainProgress";
import { BrainSection } from "@/app/inspector/brain/BrainSection";
import { EmbedderState } from "@/app/inspector/brain/EmbedderState";
import type { BrainOverview } from "@/ipc/generated";
import { formatBytes } from "@/lib/format";

export function EmbedderSection({ overview }: { overview: BrainOverview }) {
  const { embedder } = overview;
  const state = embedder.state;
  return (
    <BrainSection title="Embedding model">
      <dl className="grid grid-cols-2 gap-x-4 gap-y-1">
        <BrainDetail label="Model">
          <span className="font-mono" title={embedder.model}>
            {embedder.model}
          </span>
        </BrainDetail>
        <BrainDetail label="State"><EmbedderState state={state} /></BrainDetail>
        <BrainDetail label="Size">{formatBytes(embedder.modelBytes)}</BrainDetail>
        <BrainDetail label="Dimensions">{embedder.dimensions}</BrainDetail>
      </dl>
      {state.type === "downloading" && (
        <BrainProgress label="Model download" value={state.total > 0 ? state.received / state.total : null} />
      )}
      {state.type === "failed" && (
        <p role="alert" className="text-destructive whitespace-pre-wrap">
          {state.error}
        </p>
      )}
    </BrainSection>
  );
}
