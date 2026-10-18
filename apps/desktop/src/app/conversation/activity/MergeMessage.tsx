import type { BlockOrchestratorStep } from "@/app/conversation/blocks";
import { useViewConversation } from "@/app/conversation/viewContext";
import { ImagePreview } from "@/components/assistant-ui/elements/image-viewer";
import type { OrchestratorStepKind } from "@/ipc/generated";
import { useThreadImages } from "@/state/threadImages";

/**
 * What the thread attached to its reply, the merge message: the screenshots and recordings it
 * made of the change (`attach_proof`), under the answer, each with what it shows.
 */

type Proof = Extract<OrchestratorStepKind, { type: "proof" }>;

/** The proofs of a finished turn, in the order the thread attached them. */
export function MergeMessage({ steps }: { steps: readonly BlockOrchestratorStep[] }) {
  const conversationId = useViewConversation()?.id ?? null;
  const proofs = steps.flatMap((step) => (step.kind.type === "proof" ? [step.kind as Proof] : []));
  const media = useThreadImages(conversationId, proofs.map((proof) => proof.id));
  if (proofs.length === 0) return null;
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
    </div>
  );
}
