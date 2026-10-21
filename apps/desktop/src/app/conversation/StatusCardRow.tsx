import type { FC, ReactNode } from "react";

/** One of the status card's rows: its label, then what it says. */
export const StatusCardRow: FC<{ label: string; children: ReactNode }> = ({ label, children }) => (
  <>
    <dt className="text-muted-foreground">{label}</dt>
    <dd className="flex min-w-0 items-center gap-2">{children}</dd>
  </>
);
