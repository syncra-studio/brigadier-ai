import type { ReactNode } from "react";

/**
 * The answer its request came to before a follow-up joined it: in view under its run's fold, as
 * the last one's is, with its own actions (their time on hover).
 */
export function JoinedAnswer({ children }: { children: ReactNode }) {
  return (
    <div data-slot="joined-answer" className="group/joined pt-activity flex min-w-0 flex-col">
      {children}
    </div>
  );
}
