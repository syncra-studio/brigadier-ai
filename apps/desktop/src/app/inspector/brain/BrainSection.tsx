import type { ReactNode } from "react";

export function BrainSection({ title, children, aside }: { title: string; children: ReactNode; aside?: ReactNode }) {
  return (
    <section className="flex flex-col gap-2 border-b px-3 py-3 text-xs last:border-b-0">
      <div className="flex items-center gap-2">
        <h2 className="text-muted-foreground flex-1 font-medium">{title}</h2>
        {aside}
      </div>
      {children}
    </section>
  );
}
