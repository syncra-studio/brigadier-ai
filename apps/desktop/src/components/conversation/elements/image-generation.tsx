import type { ComponentProps } from "react";

import { cn } from "@/lib/utils";

/**
 * An image being made: a grid of dots pulsing, row by row, over a gradient that sharpens once
 * it's done. From assistant-ui's Image Generation element
 * (https://www.assistant-ui.com/elements/image-generation), sized for the thread's 80px image
 * tile: no caption, size label or regenerate button, the frame's colours left to the caller.
 */

const DOTS = Array.from({ length: 64 }, (_, i) => i);

export function ImageGeneration({
  generating,
  label = "Creating an image",
  className,
  ...props
}: Omit<ComponentProps<"div">, "children"> & {
  generating: boolean;
  /** What a screen reader hears. */
  label?: string;
}) {
  return (
    <div
      data-slot="image-generation"
      data-generating={generating || undefined}
      role="img"
      aria-label={label}
      className={cn("border-border relative size-20 shrink-0 overflow-hidden rounded-lg border", className)}
      {...props}
    >
      <div className="absolute inset-0 grid grid-cols-8 place-items-center p-2" aria-hidden>
        {DOTS.map((dot) => {
          const row = Math.floor(dot / 8);
          const col = dot % 8;
          return (
            <span
              key={dot}
              className={cn(
                "bg-foreground/20 size-0.5 rounded-full transition-opacity duration-500",
                generating ? "animate-pulse motion-reduce:animate-none" : "opacity-0",
              )}
              style={{ animationDelay: `${(row + col) * 90}ms` }}
            />
          );
        })}
      </div>
      <div
        aria-hidden
        className={cn(
          "image-generation-wash absolute inset-0 transition-[opacity,filter] duration-1000 ease-out motion-reduce:transition-none",
          generating ? "opacity-0 blur-xl" : "blur-0 opacity-100",
        )}
      />
    </div>
  );
}
