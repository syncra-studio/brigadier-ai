import type { ReactNode } from "react";

import { KINDS, ORIGIN_LABELS, stateLabel } from "@/app/inspector/brain/kinds";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { X } from "@/components/icons";
import type { Edge, EdgeKind, Node, Provenance } from "@/ipc/generated";
import { formatDateTime } from "@/lib/format";
import { useApp } from "@/state/store";

const EDGE_LABELS: Record<EdgeKind, string> = {
  contains: "contains",
  dependsOn: "depends on",
  about: "is about",
  decidedIn: "decided in",
  supersedes: "supersedes",
  implements: "implements",
  consumes: "consumes",
  relates: "relates to",
};

/** A conversation's title while it exists, else its id. */
function ConversationName({ id }: { id: string }) {
  const title = useApp((s) => s.conversations[id]?.title);
  return <span title={id}>{title ?? id}</span>;
}

function shortHash(hash: string): string {
  return hash.slice(0, 12);
}

/** Where a node came from, in one line: origin, conversation, model, commit, date. */
export function ProvenanceLine({ provenance }: { provenance: Provenance }) {
  const parts: ReactNode[] = [ORIGIN_LABELS[provenance.origin]];
  if (provenance.sessionId) parts.push(<ConversationName key="session" id={provenance.sessionId} />);
  if (provenance.worker?.model) parts.push(provenance.worker.model);
  if (provenance.commit) parts.push(<span key="commit" className="font-mono">{shortHash(provenance.commit)}</span>);
  parts.push(formatDateTime(provenance.recordedAtMs));
  return (
    <span className="text-muted-foreground">
      {parts.map((part, index) => (
        // The parts are fixed in order; the index is their identity.
        // oxlint-disable-next-line react/no-array-index-key
        <span key={index}>
          {index > 0 && " · "}
          {part}
        </span>
      ))}
    </span>
  );
}

export function KindMark({ node }: { node: Node }) {
  const state = stateLabel(node.state);
  return (
    <span className="inline-flex items-center gap-1.5">
      <span aria-hidden className={`${KINDS[node.kind].swatch} size-2 shrink-0 rounded-full`} />
      <span className="text-muted-foreground">{KINDS[node.kind].label}</span>
      {state && <Badge variant={node.state.type === "stale" ? "warning" : "secondary"}>{state}</Badge>}
    </span>
  );
}

function Detail({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="min-w-0 break-all">{children}</dd>
    </>
  );
}

/** One node in full: body, files with their hashes, full provenance and its edges. */
export function NodeDetail({
  node,
  edges,
  titles,
  onSelect,
  onClose,
}: {
  node: Node;
  edges: readonly Edge[];
  /** Titles of the other nodes loaded, by id, for the edge list. */
  titles: ReadonlyMap<string, string>;
  onSelect: (id: string) => void;
  onClose: () => void;
}) {
  const { provenance } = node;
  const outgoing = edges.filter((edge) => edge.from === node.id);
  const incoming = edges.filter((edge) => edge.to === node.id);
  const link = (id: string) =>
    titles.has(id) ? (
      <button type="button" className="text-link hover:text-link/80 text-start" onClick={() => onSelect(id)}>
        {titles.get(id)}
      </button>
    ) : (
      <span className="font-mono">{id}</span>
    );
  return (
    <div data-selectable className="flex flex-col gap-3 p-3 text-xs">
      <div className="flex items-start gap-2">
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <KindMark node={node} />
          <h3 className="text-sm font-medium wrap-break-word">{node.title}</h3>
        </div>
        <Button variant="ghost" size="icon-xs" aria-label="Close node" onClick={onClose}>
          <X />
        </Button>
      </div>
      {node.state.type === "stale" && (
        <p className="text-warning">
          Stale since {formatDateTime(node.state.sinceMs)}: {node.state.reason}
        </p>
      )}
      {node.state.type === "superseded" && (
        <p className="text-muted-foreground">
          Superseded by {link(node.state.by)}
          {node.state.reason && `: ${node.state.reason}`}
        </p>
      )}
      {node.body && <p className="whitespace-pre-wrap wrap-break-word">{node.body}</p>}
      {node.files.length > 0 && (
        <section className="flex flex-col gap-1">
          <h4 className="text-muted-foreground font-medium">Files</h4>
          <ul className="flex flex-col gap-0.5 font-mono">
            {node.files.map((file) => (
              <li key={file.path} className="flex gap-2">
                <span className="min-w-0 flex-1 truncate" title={file.path}>
                  {file.path}
                </span>
                <span className="text-muted-foreground shrink-0" title={file.hash ?? undefined}>
                  {file.hash ? shortHash(file.hash) : "no hash"}
                </span>
              </li>
            ))}
          </ul>
        </section>
      )}
      <section className="flex flex-col gap-1">
        <h4 className="text-muted-foreground font-medium">Provenance</h4>
        <dl className="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-3 gap-y-0.5">
          <Detail label="Origin">{ORIGIN_LABELS[provenance.origin]}</Detail>
          {provenance.sessionId && (
            <Detail label="Conversation">
              <ConversationName id={provenance.sessionId} />
            </Detail>
          )}
          {provenance.taskId && <Detail label="Task">{provenance.taskId}</Detail>}
          {provenance.jobId && <Detail label="Brain job">{provenance.jobId}</Detail>}
          {provenance.worker && (
            <Detail label="Model">
              {provenance.worker.provider}
              {provenance.worker.model && ` · ${provenance.worker.model}`}
            </Detail>
          )}
          {provenance.commit && (
            <Detail label="Commit">
              <span className="font-mono">{provenance.commit}</span>
            </Detail>
          )}
          <Detail label="Recorded">{formatDateTime(provenance.recordedAtMs)}</Detail>
          <Detail label="Updated">{formatDateTime(node.updatedAtMs)}</Detail>
          {node.expiresAtMs !== null && <Detail label="Expires">{formatDateTime(node.expiresAtMs)}</Detail>}
          {node.key && (
            <Detail label="Key">
              <span className="font-mono">{node.key}</span>
            </Detail>
          )}
          <Detail label="Embedded">{node.embedded ? "yes" : "no"}</Detail>
          <Detail label="Id">
            <span className="font-mono">{node.id}</span>
          </Detail>
        </dl>
      </section>
      {(outgoing.length > 0 || incoming.length > 0) && (
        <section className="flex flex-col gap-1">
          <h4 className="text-muted-foreground font-medium">Edges</h4>
          <ul className="flex flex-col gap-0.5">
            {outgoing.map((edge) => (
              <li key={`out:${edge.kind}:${edge.to}`}>
                <span className="text-muted-foreground">{EDGE_LABELS[edge.kind]} </span>
                {link(edge.to)}
              </li>
            ))}
            {incoming.map((edge) => (
              <li key={`in:${edge.kind}:${edge.from}`}>
                {link(edge.from)}
                <span className="text-muted-foreground"> {EDGE_LABELS[edge.kind]} this</span>
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
