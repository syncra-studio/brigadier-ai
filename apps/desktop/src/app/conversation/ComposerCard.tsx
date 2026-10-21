import type { ReactNode, Ref } from "react";

/** The composer's card: lifted, an inner hairline for an edge, and no focus ring. */
export function ComposerCard({ ref, children }: { ref?: Ref<HTMLDivElement>; children: ReactNode }) {
  return (
    <div
      ref={ref}
      data-slot="aui_composer-shell"
      className="@container/composer bg-composer rounded-composer shadow-hairline relative flex w-full cursor-text flex-col gap-1 p-2 backdrop-blur-lg"
    >
      {children}
    </div>
  );
}
