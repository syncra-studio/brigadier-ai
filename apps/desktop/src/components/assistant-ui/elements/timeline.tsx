import type { ComponentProps, ReactNode } from "react";

import { mono } from "@/components/assistant-ui/elements/surfaces";
import { cn } from "@/lib/utils";

/**
 * Events in order, joined by a line (assistant-ui's timeline element,
 * https://www.assistant-ui.com/elements, on Brigadier's tokens): what happened, what happens
 * now and what is still to come, each with its time and a line of detail. An event may carry
 * more behind it (`more`), shown under its detail.
 */

export type TimelineWhen = "past" | "now" | "future";

export interface TimelineEvent {
  id: string;
  when: TimelineWhen;
  /** "02:14", or empty for what hasn't happened. */
  time: string;
  title: ReactNode;
  detail?: ReactNode;
  more?: ReactNode;
}

export function Timeline({
  events,
  className,
  ...props
}: Omit<ComponentProps<"ol">, "children"> & { events: readonly TimelineEvent[] }) {
  // Without a time on any event, the dots line up with the text before it instead.
  const timed = events.some((event) => event.time);
  return (
    <ol data-slot="timeline" className={cn("flex w-full min-w-0 flex-col", className)} {...props}>
      {events.map((event, index) => {
        const last = index === events.length - 1;
        return (
          <li
            key={event.id}
            aria-current={event.when === "now" ? "step" : undefined}
            className="flex min-w-0 gap-2"
          >
            {timed && (
              <span
                className={cn(
                  mono,
                  "w-11 shrink-0 pt-0.5 text-end tabular-nums",
                  event.when === "future" ? "text-muted-foreground/60" : "text-muted-foreground",
                )}
              >
                {event.time}
              </span>
            )}
            <span aria-hidden className="flex w-3 shrink-0 flex-col items-center">
              <span
                className={cn(
                  "mt-1.5 size-2 shrink-0 rounded-full",
                  event.when === "now" && "bg-foreground ring-foreground/15 ring-4",
                  event.when === "past" && "bg-foreground/40",
                  event.when === "future" && "border-foreground/25 border",
                )}
              />
              {!last && (
                <span
                  className={cn("w-px flex-1", event.when === "future" ? "bg-foreground/10" : "bg-foreground/20")}
                />
              )}
            </span>
            <div className={cn("flex min-w-0 flex-1 flex-col gap-0.5", !last && "pb-3")}>
              <span
                className={cn(
                  "text-sm wrap-anywhere",
                  event.when === "future" ? "text-muted-foreground" : "text-foreground/90",
                  event.when === "now" && "font-medium",
                )}
              >
                {event.title}
              </span>
              {event.detail && (
                <span className="text-muted-foreground text-xs leading-relaxed wrap-anywhere">{event.detail}</span>
              )}
              {event.more}
            </div>
          </li>
        );
      })}
    </ol>
  );
}
