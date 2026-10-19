import { ArrowRotateCcw, ExclamationMarkCircle } from "@openai/apps-sdk-ui/components/Icon";
import { useMemo } from "react";

import { useAskRequest } from "@/app/conversation/activity/askRequest";
import { mergeWarning } from "@/app/conversation/activity/mergeChecks";
import { type Merged, revertRequest } from "@/app/conversation/activity/revert";
import type { BlockOrchestratorStep } from "@/app/conversation/blocks";
import { useViewConversation } from "@/app/conversation/viewContext";
import { DiffGlyph } from "@/components/assistant-ui/elements/diff-glyph";
import { ImagePreview } from "@/components/assistant-ui/elements/image-viewer";
import { Button } from "@/components/ui/button";
import type { OrchestratorStepKind } from "@/ipc/generated";
import { useBoard } from "@/state/board";
import { openPanelTab, openReviewTab } from "@/state/sessionTabs";
import { useThreadImages } from "@/state/threadImages";

/**
 * What a finished turn shows of its merge and its proofs, under the answer: the screenshots and
 * recordings it made of the change (`attach_proof`) as small thumbnails in a row, the merge in
 * the Edited files card's header (or a small header of its own when the turn edited nothing), and
 * one plain line when something about the merged code is really wrong. What was checked lives in
 * the Reviews tab.
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
 * Revert on a merge: one click sends the thread the fixed request, which it carries out (a revert
 * can conflict with later merges: the thread judges). Once sent, here or queued, it reads
 * "Revert asked".
 */
export function MergeRevert({ merge }: { merge: Merged }) {
  const conversationId = useViewConversation()?.id ?? null;
  const text = revertRequest(merge);
  const { asked, ask } = useAskRequest(conversationId ?? "", text);
  if (!text || !conversationId) return null;
  if (asked) {
    return <span data-slot="merge-revert" className="text-muted-foreground px-2 text-sm">Revert asked</span>;
  }
  return (
    <Button data-slot="merge-revert" variant="ghost" size="sm" title={text} onClick={ask} className="text-foreground font-normal">
      Revert
      <ArrowRotateCcw className="size-icon-sm" />
    </Button>
  );
}

/** View changes on a merge: the merge commit's own change, in the diff tab. */
export function MergeViewChanges({ merge }: { merge: Merged }) {
  const conversationId = useViewConversation()?.id ?? null;
  if (!conversationId || !merge.mergeCommit) return null;
  return (
    <Button
      data-slot="merge-view-changes"
      variant="ghost"
      size="sm"
      onClick={() => openReviewTab(conversationId, { type: "commit", commit: merge.mergeCommit!, staged: false })}
      className="border-border border font-normal"
    >
      View changes
    </Button>
  );
}

/**
 * A merge's own header, in the Edited files card's shape, for a turn that merged and edited
 * nothing itself ("Merge it" after the work) or whose changes can't be counted.
 */
export function MergeHeader({ merge }: { merge: Merged }) {
  return (
    <div data-slot="merge-header" className="border-foreground/10 rounded-shell flex items-center gap-3 border p-3">
      <span className="bg-foreground/5 text-foreground rounded-control flex size-control-lg shrink-0 items-center justify-center">
        <DiffGlyph className="size-icon-md" />
      </span>
      <span className="flex min-w-0 flex-1 flex-col">
        <span data-slot="merge-header-title" className="text-foreground truncate text-sm font-medium">
          {mergeWords(merge)}
        </span>
        <span className="text-muted-foreground truncate text-xs">
          {merge.commits === 1 ? "1 commit" : `${merge.commits} commits`} from {merge.branch}
        </span>
      </span>
      <MergeRevert merge={merge} />
      <MergeViewChanges merge={merge} />
    </div>
  );
}

/**
 * The one plain line under a merge when something about the merged code is really wrong (an
 * open finding, a failed check): a click opens the Reviews tab, where every check is.
 */
export function MergeWarning({ merge }: { merge: Merged }) {
  const conversationId = useViewConversation()?.id ?? null;
  const text = merge.record ? mergeWarning(merge.record) : null;
  if (!text || !conversationId) return null;
  return (
    <button
      type="button"
      data-slot="merge-warning"
      onClick={() => openPanelTab(conversationId, { type: "reviews" }, "Reviews")}
      className="text-warning hover:text-foreground flex w-fit items-center gap-1.5 text-start text-sm transition-colors"
    >
      <ExclamationMarkCircle aria-hidden className="size-icon-sm shrink-0" />
      {text}
    </button>
  );
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
