import type { ReactNode } from "react";

import { count } from "@/app/inspector/brain/overviewText";
import { Spinner } from "@/components/icons";
import type { IndexStatus } from "@/ipc/generated";

export function IndexState({ index }: { index: IndexStatus }): ReactNode {
  switch (index.state.type) {
    case "new":
      return "not scanned yet";
    case "scanning":
      return (
        <span className="inline-flex items-center gap-1.5">
          <Spinner className="size-icon-xs animate-spin" />
          scanning {count.format(index.state.done)} of {count.format(index.state.total)}
        </span>
      );
    case "ready":
      return <span className="text-success">ready</span>;
    case "failed":
      return <span className="text-destructive">failed</span>;
  }
}
