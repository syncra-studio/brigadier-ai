import type { ReactNode } from "react";

import { Spinner } from "@/components/icons";
import type { EmbedderState as EmbedderStateValue } from "@/ipc/generated";
import { formatBytes } from "@/lib/format";

export function EmbedderState({ state }: { state: EmbedderStateValue }): ReactNode {
  switch (state.type) {
    case "notInstalled":
      return "not installed";
    case "downloading":
      return (
        <span className="inline-flex items-center gap-1.5">
          <Spinner className="size-icon-xs animate-spin" />
          downloading {formatBytes(state.received)}
          {state.total > 0 && ` of ${formatBytes(state.total)}`}
        </span>
      );
    case "installed":
      return "installed, not loaded";
    case "loading":
      return "loading";
    case "loaded":
      return <span className="text-success">loaded</span>;
    case "failed":
      return <span className="text-destructive">failed</span>;
  }
}
