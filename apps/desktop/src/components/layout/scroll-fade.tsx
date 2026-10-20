import { cn } from "@/lib/utils";

/**
 * A scroll view's edge: what scrolls under it fades into the background (thread-top-fade and
 * thread-bottom-fade, globals.css). It lies over the view, never in what scrolls, so a
 * rubber-band overscroll moves the content under it and never the fade (#232). The view's own
 * padding at each end is deeper than the fade, so an edge shows only while there is more to
 * scroll to that way. The conversation's edges and a Settings page's.
 */
function ScrollFade({ edge, className }: { edge: "top" | "bottom"; className?: string }) {
  return (
    <div
      aria-hidden
      data-slot="scroll-fade"
      data-edge={edge}
      className={cn(
        "pointer-events-none absolute inset-x-0 z-10",
        edge === "top" ? "thread-top-fade top-0" : "thread-bottom-fade bottom-0",
        className,
      )}
    />
  );
}

export { ScrollFade };
