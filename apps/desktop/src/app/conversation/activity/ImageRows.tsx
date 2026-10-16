import { ImageSquare } from "@openai/apps-sdk-ui/components/Icon";

import type { LeadStep } from "@/app/conversation/activity/group";
import { useViewConversation } from "@/app/conversation/viewContext";
import { type BlockOrchestratorStep, madeImage } from "@/app/conversation/blocks";
import { ImagePreview } from "@/components/assistant-ui/elements/image-viewer";
import { ThreadActivity } from "@/components/assistant-ui/elements/thread-activity";
import { ROW_DETAIL } from "@/components/assistant-ui/elements/activity-row";
import { cn } from "@/lib/utils";
import { useThreadImages } from "@/state/threadImages";

/**
 * The images the thread looked at or made, as rows of their own that end a group of work:
 * "Viewed an image", "Viewed 3 images", "Created an image". Opened, a row shows the images as
 * 80px tiles in a row that scrolls sideways, each opening the image viewer, with the row's other
 * images a step away.
 */

/** "Viewed an image", "Viewed 3 images", "Creating an image". */
export function imagesLabel(steps: readonly BlockOrchestratorStep[]): string {
  const made = steps.some(madeImage);
  const running = steps.some((step) => step.kind.type === "tool" && step.kind.status === "inProgress");
  const [doing, done] = made ? ["Creating", "Created"] : ["Viewing", "Viewed"];
  return `${running ? doing : done} ${steps.length === 1 ? "an image" : `${steps.length} images`}`;
}

/** Images as 80px tiles, each opening the viewer with the others a step away. */
export function ImageTiles({ conversationId, steps }: { conversationId: string | null; steps: readonly LeadStep[] }) {
  const ids = steps.flatMap((step) => (step.kind.type === "tool" ? [step.kind.itemId] : []));
  const images = useThreadImages(conversationId, ids);
  const made = steps.map(madeImage);
  const gallery = images.map((image, index) => ({
    name: made[index] ? "Created image" : "Inspected image",
    url: image.url,
  }));
  return (
    <div data-slot="image-tiles" className="flex gap-2 overflow-x-auto pt-1 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
      {images.map((image, index) => {
        const alt = gallery[index]?.name ?? "Inspected image";
        const tile = "border-border size-20 shrink-0 overflow-hidden rounded-lg border";
        if (!image.url) {
          return (
            <div key={ids[index]} data-slot="image-tile" title={image.error ?? undefined} className={cn(tile, "bg-foreground/5 text-muted-foreground flex items-center justify-center p-1 text-center text-xs")}>
              {image.error ? "Not available" : <span className="shimmer">Loading</span>}
            </div>
          );
        }
        return (
          <ImagePreview key={ids[index]} url={image.url} name={alt} gallery={{ images: gallery, index }}>
            <button type="button" data-slot="image-tile" aria-label={`Open ${alt.toLowerCase()}`} className={cn(tile, "focus-visible:ring-ring focus-visible:ring-2 focus-visible:outline-none")}>
              <img src={image.url} alt={alt} className="size-full object-cover" />
            </button>
          </ImagePreview>
        );
      })}
    </div>
  );
}

/** A run of image steps as one row of their own, opening to their tiles. */
export function ImagesRow({ steps }: { steps: readonly LeadStep[] }) {
  const conversationId = useViewConversation()?.id ?? null;
  const running = steps.some((step) => step.kind.type === "tool" && step.kind.status === "inProgress");
  const done = steps.filter((step) => step.kind.type === "tool" && step.kind.status === "completed");
  return (
    <ThreadActivity
      data-slot="images-row"
      data-made={steps.some(madeImage) || undefined}
      detail={done.length > 0 ? <ImageTiles conversationId={conversationId} steps={done} /> : undefined}
      detailClassName={cn(ROW_DETAIL, "ps-0")}
    >
      <ImageSquare aria-hidden className="size-4 shrink-0" />
      <span className={cn("min-w-0 truncate", running && "shimmer")}>{imagesLabel(steps)}</span>
    </ThreadActivity>
  );
}

/** The images a finished turn made, gathered above its end cards (the changed files, the answer's files). */
export function MadeImages({ steps }: { steps: readonly LeadStep[] }) {
  const conversationId = useViewConversation()?.id ?? null;
  const made = steps.filter((step) => madeImage(step) && step.kind.type === "tool" && step.kind.status === "completed");
  if (made.length === 0) return null;
  return (
    <div data-slot="made-images">
      <ImageTiles conversationId={conversationId} steps={made} />
    </div>
  );
}
