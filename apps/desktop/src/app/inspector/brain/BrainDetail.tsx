import type { ReactNode } from "react";

export function BrainDetail({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="min-w-0 truncate text-end tabular-nums">{children}</dd>
    </>
  );
}
