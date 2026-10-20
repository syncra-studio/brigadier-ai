import { type FormEvent, useState } from "react";

import { KINDS, NODE_KINDS } from "@/app/inspector/brain/kinds";
import { KindMark, ProvenanceLine } from "@/app/inspector/brain/NodeDetail";
import { Picker } from "@/app/inspector/providers/Picker";
import { Spinner } from "@/components/glyphs/spinner";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { ChevronRight, Search } from "@/components/icons";
import { Input } from "@/components/ui/input";
import type { BrainAnswer, BrainHit, NodeKind } from "@/ipc/generated";
import { formatMs } from "@/lib/format";
import { cn } from "@/lib/utils";
import { queryBrain } from "@/state/brain";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const ALL = "all";

function HitRow({ hit }: { hit: BrainHit }) {
  const [open, setOpen] = useState(false);
  const { node } = hit;
  return (
    <li className="border-b last:border-b-0">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        className="hover:bg-accent/50 flex w-full flex-col gap-0.5 px-3 py-2 text-start"
      >
        <span className="flex items-center gap-2">
          <KindMark node={node} />
          {hit.linked && (
            <span className="text-muted-foreground" title="Found through a link from another hit">
              · linked
            </span>
          )}
          <span className="flex-1" />
          <span className="text-muted-foreground font-mono tabular-nums">{hit.score.toFixed(3)}</span>
        </span>
        <span className={cn("font-medium wrap-break-word", node.state.type === "superseded" && "text-muted-foreground")}>
          {node.title}
        </span>
        <ProvenanceLine provenance={node.provenance} />
      </button>
      {open && node.body && (
        <p className="text-muted-foreground px-3 pb-2 whitespace-pre-wrap wrap-break-word">{node.body}</p>
      )}
    </li>
  );
}

/**
 * Runs the real retrieval the orchestrator's `query_brain` uses and shows what came back: the
 * hits with their scores, the time it took, whether embeddings took part, and the text the
 * orchestrator would read.
 */
export function BrainSearch({ brainKey }: { brainKey: string }) {
  const [text, setText] = useState("");
  const [kind, setKind] = useState<NodeKind | typeof ALL>(ALL);
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
      kinds: kind === ALL ? [] : [kind],
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
      <form onSubmit={submit} className="flex shrink-0 items-center gap-2 border-b px-3 py-2">
        <Input
          value={text}
          onChange={(event) => setText(event.target.value)}
          placeholder="Ask the Brain, as the orchestrator would"
          aria-label="Brain query"
          spellCheck={false}
          className="h-control-sm text-xs"
        />
        <Picker
          label="Kinds"
          value={kind}
          options={[
            { value: ALL, label: "All kinds" },
            ...NODE_KINDS.map((value) => ({ value, label: KINDS[value].label })),
          ]}
          onChange={(value) => setKind(value as NodeKind | typeof ALL)}
        />
        <Button type="submit" size="xs" disabled={busy || !text.trim()}>
          {busy ? <Spinner className="animate-spin" /> : <Search />}
          Search
        </Button>
      </form>
      {error && (
        <p role="alert" className="text-destructive shrink-0 border-b px-3 py-2 text-xs">
          {error}
        </p>
      )}
      <div data-selectable className="min-h-0 flex-1 overflow-y-auto text-xs">
        {answer ? (
          <>
            <p role="status" className="text-muted-foreground border-b px-3 py-2 tabular-nums">
              {answer.answer.hits.length} {answer.answer.hits.length === 1 ? "hit" : "hits"} for “{answer.query}” in{" "}
              <span className="text-foreground">{formatMs(answer.answer.tookUs / 1000)}</span> ·{" "}
              {answer.answer.semantic ? "keywords and embeddings" : "keywords only (no embeddings)"}
            </p>
            {answer.answer.hits.length === 0 ? (
              <p className="text-muted-foreground p-3">Nothing in this Brain matches.</p>
            ) : (
              <ul>
                {answer.answer.hits.map((hit) => (
                  <HitRow key={hit.node.id} hit={hit} />
                ))}
              </ul>
            )}
            <Collapsible open={raw} onOpenChange={setRaw} className="border-t">
              <CollapsibleTrigger className="text-muted-foreground hover:text-foreground flex w-full items-center gap-1 px-3 py-2 text-start">
                <ChevronRight className={cn("size-icon-xs transition-[rotate]", raw && "rotate-90")} />
                What the orchestrator reads · {answer.answer.text.length.toLocaleString()} characters
              </CollapsibleTrigger>
              <CollapsibleContent>
                <pre className="bg-muted/50 mx-3 mb-3 overflow-x-auto rounded-control p-2 font-mono whitespace-pre-wrap">
                  {answer.answer.text}
                </pre>
              </CollapsibleContent>
            </Collapsible>
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
