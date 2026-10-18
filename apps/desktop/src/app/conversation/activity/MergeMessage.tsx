import { useRef, useState } from "react";

import type { BlockOrchestratorStep } from "@/app/conversation/blocks";
import { useViewConversation } from "@/app/conversation/viewContext";
import { ImagePreview } from "@/components/assistant-ui/elements/image-viewer";
import { Button } from "@/components/ui/button";
import type { OrchestratorStepKind } from "@/ipc/generated";
import { send } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";
import { useThreadImages } from "@/state/threadImages";

/**
 * What the thread attached to its reply, the merge message: the screenshots and recordings it
 * made of the change (`attach_proof`), under the answer, each with what it shows; and, once the
 * turn merged, Revert.
 */

type Proof = Extract<OrchestratorStepKind, { type: "proof" }>;
type Merged = Extract<OrchestratorStepKind, { type: "merged" }>;

/** The request Revert sends as the user's: what to take back, where, and how it went in. */
export function revertRequest(merge: Merged): string | null {
  if (!merge.mergeCommit) return null;
  return `Revert merge ${merge.mergeCommit.slice(0, 7)}${merge.pr === undefined ? "" : ` (PR #${merge.pr})`} on ${merge.base} and push it the way it went in`;
}

/**
 * Revert on the turn's last merge: one click sends the thread the fixed request, which it
 * carries out (a revert can conflict with later merges: the thread judges). Once sent, here or
 * queued, it reads "Revert asked".
 */
function Revert({ conversationId, merge }: { conversationId: string; merge: Merged }) {
  const text = revertRequest(merge);
  const [sending, setSending] = useState(false);
  // A second click before the first one's render sends nothing.
  const clicked = useRef(false);
  const asked = useApp(
    (state) =>
      !!text &&
      (state.threads[conversationId]?.items.some((item) => item.role === "user" && item.text === text) ||
        state.pending.some((entry) => entry.conversationId === conversationId && entry.text === text)),
  );
  const queued = useBoard((state) => !!text && state.board?.conversationId === conversationId && state.board.queue.items.some((item) => item.text === text));
  if (!text) return null;
  if (asked || queued || sending) {
    return <p data-slot="merge-revert" className="text-muted-foreground text-sm">Revert asked</p>;
  }
  return (
    <Button
      data-slot="merge-revert"
      size="sm"
      variant="ghost"
      className="w-fit font-normal"
      title={text}
      onClick={() => {
        if (clicked.current) return;
        clicked.current = true;
        setSending(true);
        send({ text, attachments: [], mentions: [] }, undefined, { to: conversationId }).catch(() => {
          clicked.current = false;
          setSending(false);
        });
      }}
    >
      Revert
    </Button>
  );
}

/** The proofs of a finished turn, in the order the thread attached them, then Revert. */
export function MergeMessage({ steps }: { steps: readonly BlockOrchestratorStep[] }) {
  const conversationId = useViewConversation()?.id ?? null;
  const proofs = steps.flatMap((step) => (step.kind.type === "proof" ? [step.kind as Proof] : []));
  const merge = steps.flatMap((step) => (step.kind.type === "merged" && step.kind.mergeCommit ? [step.kind as Merged] : [])).at(-1);
  const media = useThreadImages(conversationId, proofs.map((proof) => proof.id));
  if (proofs.length === 0 && !merge) return null;
  const images = proofs.flatMap((proof, index) => (proof.mime.startsWith("image/") ? [{ name: proof.caption ?? proof.name, url: media[index]?.url ?? null }] : []));
  return (
    <div data-slot="merge-message" className="mt-activity flex flex-col gap-3">
      {proofs.map((proof, index) => {
        const { url, error } = media[index] ?? { url: null, error: null };
        const label = proof.caption ?? proof.name;
        const frame = "border-border max-h-80 max-w-full rounded-lg border";
        let shown;
        if (!url) {
          shown = (
            <div title={error ?? undefined} className="border-border bg-foreground/5 text-muted-foreground flex h-24 w-40 items-center justify-center rounded-lg border p-1 text-center text-xs">
              {error ? "Not available" : <span className="shimmer">Loading</span>}
            </div>
          );
        } else if (proof.mime.startsWith("video/")) {
          shown = <video src={url} controls muted playsInline preload="metadata" aria-label={label} className={frame} />;
        } else {
          const at = images.findIndex((image) => image.url === url);
          shown = (
            <ImagePreview url={url} name={label} gallery={{ images, index: Math.max(at, 0) }}>
              <button type="button" aria-label={`Open ${label}`} className="focus-visible:ring-ring w-fit rounded-lg focus-visible:ring-2 focus-visible:outline-none">
                <img src={url} alt={label} className={frame} />
              </button>
            </ImagePreview>
          );
        }
        return (
          <figure key={proof.id} data-slot="proof" data-kind={proof.mime.startsWith("video/") ? "video" : "image"} className="flex flex-col gap-1">
            {shown}
            {proof.caption && <figcaption className="text-muted-foreground text-sm">{proof.caption}</figcaption>}
          </figure>
        );
      })}
      {merge && conversationId && <Revert conversationId={conversationId} merge={merge} />}
    </div>
  );
}
