import type { ReactNode } from "react";

export function SetupPanel({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section data-slot="setup-panel" className="flex flex-col gap-3 text-sm">
      <h1 className="text-lg font-medium">{title}</h1>
      {children}
    </section>
  );
}
