import type { ComponentProps, ReactNode } from "react";

import { Spinner } from "@/components/glyphs/spinner";
import { cn } from "@/lib/utils";

/**
 * The Message queue element (assistant-ui), laid out as a queue card on the composer
 * rail: one quiet row per waiting message, in the order they will be sent, no header and no
 * count. Brigadier's controls (steer, delete, the actions menu, the drag grip) come in as
 * children.
 */
export function MessageQueue({ className, ...props }: ComponentProps<"ol">) {
  return (
    <ol
      data-slot="message-queue"
      className={cn("max-h-queue-max flex flex-col overflow-y-auto px-1 py-1", className)}
      {...props}
    />
  );
}

/** "Queue paused because you interrupted", with its Resume button. */
export function MessageQueuePaused({ children }: { children?: ReactNode }) {
  return (
    <div
      data-slot="message-queue-paused"
      className="text-muted-foreground min-h-row flex items-center gap-2 px-3 text-sm"
    >
      <span className="bg-warning size-2 shrink-0 rounded-full" />
      <span className="min-w-0 flex-1 truncate">Queue paused because you interrupted</span>
      {children}
    </div>
  );
}

/** The queue glyph: lines of a list with an arrow into them. */
export function QueueGlyph({ className }: { className?: string }) {
  return (
    <svg aria-hidden viewBox="0 0 20 20" fill="currentColor" className={className}>
      <path d="M9.75 4.25h6.5a.75.75 0 0 1 0 1.5h-6.5a.75.75 0 0 1 0-1.5Zm0 5h6.5a.75.75 0 0 1 0 1.5h-6.5a.75.75 0 0 1 0-1.5Zm0 5h6.5a.75.75 0 0 1 0 1.5h-6.5a.75.75 0 0 1 0-1.5ZM3.5 6.6a.5.5 0 0 1 .8-.4l3.9 3.4a.5.5 0 0 1 0 .8l-3.9 3.4a.5.5 0 0 1-.8-.4Z" />
    </svg>
  );
}

/**
 * One queued message: the glyph (a drag grip on hover), its text, then its actions. While it
 * is `deciding` (a session's orchestrator judges whether it joins the working answer) a
 * spinner takes the glyph's place.
 */
export function MessageQueueItem({
  dragging,
  deciding,
  grip,
  className,
  children,
  ...props
}: ComponentProps<"li"> & {
  dragging?: boolean;
  deciding?: boolean;
  /** Shown over the glyph on hover, when the order can change. */
  grip?: ReactNode;
}) {
  const glyph = cn("size-icon-sm", grip && "group-hover/queue-item:invisible group-focus-within/queue-item:invisible");
  return (
    <li
      data-slot="message-queue-item"
      data-dragging={dragging || undefined}
      data-deciding={deciding || undefined}
      aria-busy={deciding || undefined}
      className={cn(
        "group/queue-item rounded-control min-h-row fade-in animate-in flex items-center gap-2 ps-1.5 pe-1 text-sm duration-200",
        dragging && "bg-foreground/10 relative z-10",
        className,
      )}
      {...props}
    >
      <span className="text-muted-foreground/70 relative flex size-icon-button-sm shrink-0 items-center justify-center">
        {deciding ? (
          <Spinner className={cn(glyph, "animate-spin motion-reduce:animate-none")} />
        ) : (
          <QueueGlyph className={glyph} />
        )}
        {grip && (
          <span className="invisible absolute inset-0 flex group-hover/queue-item:visible group-focus-within/queue-item:visible">
            {grip}
          </span>
        )}
      </span>
      {children}
    </li>
  );
}
