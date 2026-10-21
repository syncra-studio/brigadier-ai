import type { FC, ReactNode } from "react";

/** The capsule above the composer, a pill of pills: a run's or a request's progress and diff. */
export const Capsule: FC<{ children: ReactNode }> = ({ children }) => (
  // In the floating footer's flow, above the composer: the thread's padding and the scroll
  // button make room for it.
  <div className="pointer-events-none flex justify-center pb-1.5">
    <div
      data-slot="composer-capsule"
      className="max-w-full border-border/80 bg-background/70 text-muted-foreground rounded-capsule animate-in fade-in slide-in-from-bottom-1 pointer-events-auto flex items-center gap-2 border px-3 py-1.5 text-xs backdrop-blur-sm duration-150 motion-reduce:animate-none"
    >
      {children}
    </div>
  </div>
);
