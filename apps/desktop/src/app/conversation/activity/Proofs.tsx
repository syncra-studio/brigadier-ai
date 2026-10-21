import type { FC, ReactNode } from "react";

import { kindOf, LAST_LINES, printed, type Proof } from "@/app/conversation/activity/previewProofs";
import { ShellBox } from "@/app/conversation/activity/StepDetail";
import { ImagePreview } from "@/components/conversation";
import { formatTime } from "@/lib/format";
import { cn } from "@/lib/utils";
import { useThreadImages } from "@/state/threadImages";

/** A text file read as a data URL, as words. */
function textOf(url: string): string {
  const data = url.slice(url.indexOf(",") + 1);
  return new TextDecoder().decode(Uint8Array.from(atob(data), (char) => char.charCodeAt(0)));
}

/** The proofs, two to a row: each cropped to the top of what it shows, "1. What it shows" and when under it. */
export const Proofs: FC<{ proofs: readonly { proof: Proof; atMs: number | undefined }[]; conversationId: string | null }> = ({ proofs, conversationId }) => {
  const media = useThreadImages(conversationId, proofs.map(({ proof }) => proof.id));
  const images = proofs.flatMap(({ proof }, index) => (kindOf(proof) === "screenshot" ? [{ name: proof.caption ?? proof.name, url: media[index]?.url ?? null }] : []));
  const frame = "border-border block h-44.5 w-full overflow-hidden rounded-lg border";
  return (
    <div data-slot="turn-proofs" className="grid grid-cols-2 gap-3">
      {proofs.map(({ proof, atMs }, index) => {
        const { url, error } = media[index] ?? { url: null, error: null };
        // What it shows in the thread's words; a proof taken without them goes by its file's name.
        const label = proof.caption ?? proof.name;
        const kind = kindOf(proof);
        let shown: ReactNode;
        if (url && printed(proof)) {
          // Its last lines, under the command or file it came from.
          const lines = textOf(url).trimEnd().split("\n");
          shown = <ShellBox command={proof.takenOn ?? proof.name} output={lines.slice(-LAST_LINES).join("\n")} status="inProgress" exit={null} />;
        } else if (!url) {
          shown = (
            <div title={error ?? label} className={cn(frame, "bg-foreground/5 text-muted-foreground flex items-center justify-center p-1 text-center text-xs")}>
              {error ? "Not available" : <span className="shimmer">Loading</span>}
            </div>
          );
        } else if (kind === "recording") {
          shown = <video src={url} controls muted playsInline preload="metadata" aria-label={label} title={label} className={cn(frame, "object-cover object-top")} />;
        } else {
          const at = images.findIndex((image) => image.url === url);
          shown = (
            <ImagePreview url={url} name={label} gallery={{ images, index: Math.max(at, 0) }}>
              <button type="button" aria-label={`Open ${label}`} title={label} className={cn(frame, "focus-visible:ring-ring group/proof relative focus-visible:ring-2 focus-visible:outline-none")}>
                <img src={url} alt={label} className="size-full object-cover object-top" />
                <span aria-hidden className="group-hover/proof:bg-background/20 pointer-events-none absolute inset-0 transition-colors" />
              </button>
            </ImagePreview>
          );
        }
        return (
          <figure key={proof.id} data-slot="proof" data-kind={kind} data-caption={proof.caption} className={cn("flex min-w-0 flex-col gap-2", printed(proof) && "col-span-2")}>
            {shown}
            <figcaption className="flex min-w-0 items-center gap-1.5 text-xs">
              <span className="text-muted-foreground shrink-0">{index + 1}.</span>
              <span className="text-foreground min-w-0 truncate">{label}</span>
              {atMs !== undefined && <span className="text-muted-foreground shrink-0">{formatTime(atMs)}</span>}
            </figcaption>
          </figure>
        );
      })}
    </div>
  );
};
