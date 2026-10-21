import { type FormEvent, useState } from "react";

import { HitRow } from "@/app/inspector/brain/HitRow";
import { OrchestratorReadsDisclosure } from "@/app/inspector/brain/OrchestratorReadsDisclosure";
import { ALL_KINDS, QueryBar } from "@/app/inspector/brain/QueryBar";
import { ResultSummary } from "@/app/inspector/brain/ResultSummary";
import type { BrainAnswer, NodeKind } from "@/ipc/generated";
import { queryBrain } from "@/state/brain";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Runs the real retrieval the orchestrator's `query_brain` uses and shows what came back: the
 * hits with their scores, the time it took, whether embeddings took part, and the text the
 * orchestrator would read.
 */
export function BrainSearch({ brainKey }: { brainKey: string }) {
  const [text, setText] = useState("");
  const [kind, setKind] = useState<NodeKind | typeof ALL_KINDS>(ALL_KINDS);
  const [answer, setAnswer] = useState<{ query: string; answer: BrainAnswer } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [raw, setRaw] = useState(false);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const query = text.trim();
    if (!query) return;
    setBusy(true);
    setError(null);
    queryBrain(brainKey, {
      text: query,
      kinds: kind === ALL_KINDS ? [] : [kind],
      limit: null,
      maxTokens: null,
      files: false,
      history: false,
      caps: null,
      page: null,
    })
      .then((result) => setAnswer({ query, answer: result }))
      .catch((cause: unknown) => setError(errorText(cause)))
      .finally(() => setBusy(false));
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <QueryBar
        text={text}
        onTextChange={setText}
        kind={kind}
        onKindChange={setKind}
        busy={busy}
        onSubmit={submit}
      />
      {error && (
        <p role="alert" className="text-destructive shrink-0 border-b px-3 py-2 text-xs">
          {error}
        </p>
      )}
      <div data-selectable className="min-h-0 flex-1 overflow-y-auto text-xs">
        {answer ? (
          <>
            <ResultSummary query={answer.query} answer={answer.answer} />
            {answer.answer.hits.length === 0 ? (
              <p className="text-muted-foreground p-3">Nothing in this Brain matches.</p>
            ) : (
              <ul>
                {answer.answer.hits.map((hit) => (
                  <HitRow key={hit.node.id} hit={hit} />
                ))}
              </ul>
            )}
            <OrchestratorReadsDisclosure open={raw} onOpenChange={setRaw} text={answer.answer.text} />
          </>
        ) : (
          <p className="text-muted-foreground p-4">
            Search runs the same retrieval as the orchestrator&apos;s query_brain tool.
          </p>
        )}
      </div>
    </div>
  );
}
