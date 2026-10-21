import { useState } from "react";

import { KindMark, ProvenanceLine } from "@/app/inspector/brain/NodeDetail";
import type { BrainHit } from "@/ipc/generated";
import { cn } from "@/lib/utils";

export function HitRow({ hit }: { hit: BrainHit }) {
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
