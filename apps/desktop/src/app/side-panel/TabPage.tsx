import type { ReactNode } from "react";

/** A job tab's page: one scrolling column at the thread's width. */
export function TabPage({ children }: { children: ReactNode }) {
  return (
    <div data-slot="job-tab" className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div className="max-w-thread mx-auto flex w-full flex-col gap-8 px-5 pt-6 pb-10">{children}</div>
    </div>
  );
}
