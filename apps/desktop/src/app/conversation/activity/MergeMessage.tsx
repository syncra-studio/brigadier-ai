import { useMemo } from "react";

import type { Merged } from "@/app/conversation/activity/revert";
import type { BlockOrchestratorStep } from "@/app/conversation/blocks";
import { useViewConversation } from "@/app/conversation/viewContext";
import { ImagePreview } from "@/components/assistant-ui/elements/image-viewer";
import type { OrchestratorStepKind } from "@/ipc/generated";
import { useBoard } from "@/state/board";
import type { ReviewTarget } from "@/state/sessionTabs";
import { useThreadImages } from "@/state/threadImages";

/**
 * What a finished turn shows of its merge and its proofs, under the answer: the screenshots and
 * recordings it made of the change (`attach_proof`) as small thumbnails in a row; the merge, in
 * the Changes card. What was checked lives in the Progress tab.
 */

type Proof = Extract<OrchestratorStepKind, { type: "proof" }>;

/** A turn's merge: `own` when this turn made it, else the later merge that took this turn's edits. */
export type TurnMerge = { merge: Merged; own: boolean };

const isMerge = (kind: BlockOrchestratorStep["kind"]): kind is Merged => kind.type === "merged" && !!(kind as Merged).mergeCommit;

/**
 * The merge a turn shows: the last one it made, else the last merge (any turn's) whose requests
 * include one of this turn's, so the turn that edited the files says they merged.
 */
export function useTurnMerge(steps: readonly BlockOrchestratorStep[], requestIds: readonly string[]): TurnMerge | null {
  const own = steps.flatMap((step) => (isMerge(step.kind) ? [step.kind] : [])).at(-1);
  const key = requestIds.join(" ");
  const later = useBoard((s) => {
    if (own || !key) return null;
    const ids = key.split(" ");
    const found = (s.board?.orchestratorSteps ?? [])
      .flatMap((step) => (isMerge(step.kind) && step.kind.requests?.some((id) => ids.includes(id)) ? [step.kind] : []))
      .at(-1);
    return found ? JSON.stringify(found) : null;
  });
  return useMemo(() => (own ? { merge: own, own: true } : later ? { merge: JSON.parse(later) as Merged, own: false } : null), [own, later]);
}

/** "Merged into main", "Merged into main on this Mac", "Merged into main (PR #7)". */
export function mergeWords(merge: Merged): string {
  if (merge.pr !== undefined) return `Merged into ${merge.base} (PR #${merge.pr})`;
  return merge.upstream ? `Merged into ${merge.base} on this Mac` : `Merged into ${merge.base}`;
}

/**
 * Where a merge's View changes opens: the merge commit's own change, in a diff tab of its own.
 * A fast-forward's commit is the branch's head: its change is all the merged commits (#411).
 */
export function mergeViewTarget(merge: Merged): Extract<ReviewTarget, { type: "commit" }> {
  const commits = merge.pr == null && merge.mergeCommit === merge.head && merge.commits > 1 ? merge.commits : undefined;
  return { type: "commit", commit: merge.mergeCommit!, ...(commits ? { commits } : {}), staged: false };
}

/** A finished turn's proofs, in the order the thread attached them: small thumbnails in a row. */
export function TurnProofs({ steps }: { steps: readonly BlockOrchestratorStep[] }) {
  const conversationId = useViewConversation()?.id ?? null;
  const proofs = steps.flatMap((step) => (step.kind.type === "proof" ? [step.kind as Proof] : []));
  const media = useThreadImages(conversationId, proofs.map((proof) => proof.id));
  if (proofs.length === 0) return null;
  const images = proofs.flatMap((proof, index) => (proof.mime.startsWith("image/") ? [{ name: proof.caption ?? proof.name, url: media[index]?.url ?? null }] : []));
  const frame = "border-border rounded-lg border h-40 w-48 object-cover";
  return (
    <div data-slot="turn-proofs" className="flex flex-wrap gap-2">
      {proofs.map((proof, index) => {
        const { url, error } = media[index] ?? { url: null, error: null };
        const label = proof.caption ?? proof.name;
        const kind = proof.mime.startsWith("video/") ? "video" : "image";
        let shown;
        if (!url) {
          shown = (
            <div title={error ?? label} className="border-border bg-foreground/5 text-muted-foreground flex h-40 w-48 items-center justify-center rounded-lg border p-1 text-center text-xs">
              {error ? "Not available" : <span className="shimmer">Loading</span>}
            </div>
          );
        } else if (kind === "video") {
          shown = <video src={url} controls muted playsInline preload="metadata" aria-label={label} title={label} className={frame} />;
        } else {
          const at = images.findIndex((image) => image.url === url);
          shown = (
            <ImagePreview url={url} name={label} gallery={{ images, index: Math.max(at, 0) }}>
              <button type="button" aria-label={`Open ${label}`} title={label} className="focus-visible:ring-ring group/proof relative block rounded-lg focus-visible:ring-2 focus-visible:outline-none">
                <img src={url} alt={label} className={frame} />
                <span aria-hidden className="pointer-events-none absolute inset-0 rounded-lg transition-colors group-hover/proof:bg-background/20" />
              </button>
            </ImagePreview>
          );
        }
        return (
          <div key={proof.id} data-slot="proof" data-kind={kind} data-caption={proof.caption}>
            {shown}
          </div>
        );
      })}
    </div>
  );
}
