import { lazy, Suspense, useEffect, useId, useMemo, useState } from "react";

import { KINDS, NODE_KINDS } from "@/app/inspector/brain/kinds";
import { NodeDetail } from "@/app/inspector/brain/NodeDetail";
import { Picker } from "@/app/inspector/providers/Picker";
import { ChevronDown, Spinner } from "@/components/icons";
import { Button, Checkbox, Input } from "@/components/controls";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/overlays";
import type { BrainGraph, BrainOverview, NodeFilter, NodeKind } from "@/ipc/generated";
import { getBrainGraph, projectOf } from "@/state/brain";
import { useApp } from "@/state/store";

// sigma and graphology load only once the Brain tab is open (see `preloadGraphCanvas`).
const loadGraphCanvas = () => import("@/components/views/brain-graph");
const BrainGraphCanvas = lazy(loadGraphCanvas);

/** Loads the graph's libraries ahead of time, so showing the graph doesn't also evaluate them. */
export function preloadGraphCanvas() {
  void loadGraphCanvas();
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const LIMITS = [100, 300, 1000, 3000];
const ALL_CONVERSATIONS = "all";

function KindFilter({ kinds, onChange }: { kinds: NodeKind[]; onChange: (kinds: NodeKind[]) => void }) {
  const label = kinds.length === 0 ? "All kinds" : kinds.length === 1 ? KINDS[kinds[0] ?? "module"].label : `${kinds.length} kinds`;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="outline" size="xs" aria-label="Kinds" className="min-w-0 justify-between">
          <span className="truncate">{label}</span>
          <ChevronDown />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start">
        <DropdownMenuItem className="text-xs" onSelect={() => onChange([])}>
          All kinds
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        {NODE_KINDS.map((kind) => (
          <DropdownMenuCheckboxItem
            key={kind}
            className="text-xs"
            checked={kinds.includes(kind)}
            // Keep the menu open while picking several.
            onSelect={(event) => event.preventDefault()}
            onCheckedChange={(checked) =>
              onChange(checked ? [...kinds, kind] : kinds.filter((existing) => existing !== kind))
            }
          >
            <span aria-hidden className={`${KINDS[kind].swatch} size-2 shrink-0 rounded-full`} />
            {KINDS[kind].label}
          </DropdownMenuCheckboxItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** The Brain's nodes and edges, filtered, with the one clicked shown in full below. */
export function BrainGraphPanel({ brainKey, overview }: { brainKey: string; overview: BrainOverview | null }) {
  const projectId = projectOf(brainKey);
  const conversations = useApp((s) => s.conversations);
  const [kinds, setKinds] = useState<NodeKind[]>([]);
  const [session, setSession] = useState(ALL_CONVERSATIONS);
  const [typed, setTyped] = useState("");
  const [text, setText] = useState("");
  const [currentOnly, setCurrentOnly] = useState(false);
  const currentOnlyId = useId();
  const [limit, setLimit] = useState(300);
  /** The last read, and the filter it answered. */
  const [read, setRead] = useState<{
    filter: NodeFilter;
    graph: BrainGraph | null;
    error: string | null;
  } | null>(null);
  const [selected, setSelected] = useState<string | null>(null);

  // The text filter applies once typing pauses.
  useEffect(() => {
    const timer = window.setTimeout(() => setText(typed.trim()), 300);
    return () => window.clearTimeout(timer);
  }, [typed]);

  const filter = useMemo<NodeFilter>(
    () => ({
      kinds,
      sessionId: session === ALL_CONVERSATIONS ? null : session,
      currentOnly,
      text: text || null,
      limit,
    }),
    [kinds, session, currentOnly, text, limit],
  );

  // Read again when the Brain changes under the open graph (a scan, a job, a report).
  const stats = overview?.stats;
  const version = stats ? `${stats.nodes}:${stats.edges}:${stats.stale}:${stats.updatedMs}` : "";

  useEffect(() => {
    let current = true;
    getBrainGraph(brainKey, filter).then(
      (graph) => current && setRead({ filter, graph, error: null }),
      // The last graph stays shown under the error.
      (cause: unknown) =>
        current && setRead((last) => ({ filter, graph: last?.graph ?? null, error: errorText(cause) })),
    );
    return () => {
      current = false;
    };
    // `version` only says when to read again.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [brainKey, filter, version]);
  const graph = read?.graph ?? null;
  const error = read?.error ?? null;
  const loading = read?.filter !== filter;

  const sessionOptions = useMemo(
    () => [
      { value: ALL_CONVERSATIONS, label: "All conversations" },
      ...Object.values(conversations)
        .filter((conversation) => conversation.projectId === projectId)
        .toSorted((a, b) => b.updatedAtMs - a.updatedAtMs)
        .map((conversation) => ({ value: conversation.id, label: conversation.title })),
    ],
    [conversations, projectId],
  );

  const titles = useMemo(
    () => new Map((graph?.nodes ?? []).map((node) => [node.id, node.title])),
    [graph],
  );
  const node = selected ? graph?.nodes.find((entry) => entry.id === selected) : undefined;

  // The total is known only when the filter is by kind alone.
  const total =
    overview && filter.sessionId === null && filter.text === null && !currentOnly
      ? overview.stats.kinds
          .filter((entry) => kinds.length === 0 || kinds.includes(entry.kind))
          .reduce((sum, entry) => sum + entry.nodes, 0)
      : null;
  const shown = graph?.nodes.length ?? 0;

  return (
    <div className="flex min-h-0 flex-1 flex-col text-xs">
      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b px-3 py-2">
        <KindFilter kinds={kinds} onChange={setKinds} />
        <Picker label="Conversation" value={session} options={sessionOptions} onChange={setSession} />
        <Input
          value={typed}
          onChange={(event) => setTyped(event.target.value)}
          placeholder="Title or body contains…"
          aria-label="Filter by text"
          spellCheck={false}
          className="h-control-xs w-auto min-w-0 flex-1 text-xs"
        />
        <span className="flex items-center gap-1.5">
          <Checkbox
            id={currentOnlyId}
            checked={currentOnly}
            onCheckedChange={(checked) => setCurrentOnly(checked === true)}
          />
          <label htmlFor={currentOnlyId}>Hide superseded</label>
        </span>
        <Picker
          label="At most"
          value={String(limit)}
          options={LIMITS.map((value) => ({ value: String(value), label: `${value} nodes` }))}
          onChange={(value) => setLimit(Number(value))}
        />
      </div>
      <div className="text-muted-foreground flex shrink-0 items-center gap-2 border-b px-3 py-1.5 tabular-nums">
        {loading && <Spinner className="size-icon-xs animate-spin" />}
        <span className="flex-1">
          {graph &&
            (graph.truncated
              ? total !== null
                ? `Showing ${shown.toLocaleString()} of ${total.toLocaleString()} nodes, newest first`
                : `Showing the newest ${shown.toLocaleString()} nodes; more match`
              : `${shown.toLocaleString()} nodes`)}
          {graph && ` · ${graph.edges.length.toLocaleString()} edges`}
        </span>
        <span className="flex items-center gap-1.5">
          <span aria-hidden className="border-warning size-2 rounded-full border" />
          stale
        </span>
        <span className="flex items-center gap-1.5">
          <span aria-hidden className="bg-muted-foreground/30 size-2 rounded-full" />
          superseded
        </span>
      </div>
      {error && (
        <p role="alert" className="text-destructive shrink-0 border-b px-3 py-2">
          {error}
        </p>
      )}
      <div className="bg-background relative min-h-0 flex-1">
        {graph && shown > 0 && (
          <Suspense
            fallback={
              <p className="text-muted-foreground absolute inset-0 flex items-center justify-center gap-2">
                <Spinner className="size-icon-sm animate-spin" />
                Loading the graph viewer…
              </p>
            }
          >
            <BrainGraphCanvas graph={graph} selected={selected} onSelect={setSelected} />
          </Suspense>
        )}
        {graph && shown === 0 && (
          <p className="text-muted-foreground absolute inset-0 flex items-center justify-center">
            No nodes match.
          </p>
        )}
      </div>
      {node && graph && (
        <div className="max-h-1/2 shrink-0 overflow-y-auto border-t">
          <NodeDetail
            node={node}
            edges={graph.edges}
            titles={titles}
            onSelect={setSelected}
            onClose={() => setSelected(null)}
          />
        </div>
      )}
    </div>
  );
}
