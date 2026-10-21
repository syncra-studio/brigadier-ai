import type { BrainAnswer } from "@/ipc/generated";
import { formatMs } from "@/lib/format";

/** The line over a Brain search's hits: how many, how fast, and how it searched. */
export function ResultSummary({ query, answer }: { query: string; answer: BrainAnswer }) {
  return (
    <p role="status" className="text-muted-foreground border-b px-3 py-2 tabular-nums">
      {answer.hits.length} {answer.hits.length === 1 ? "hit" : "hits"} for “{query}” in{" "}
      <span className="text-foreground">{formatMs(answer.tookUs / 1000)}</span> ·{" "}
      {answer.semantic ? "keywords and embeddings" : "keywords only (no embeddings)"}
    </p>
  );
}
