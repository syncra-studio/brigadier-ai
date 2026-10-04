import React from "react";

import type { Gate } from "@/ipc/generated";

/** Old stored gates and full verification add no label. */
export function ScopedChecks({ gate }: { gate: Gate }) {
  return gate.verificationScope?.type === "scoped"
    ? React.createElement("p", { className: "text-xs text-muted-foreground" }, "Scoped checks: small change")
    : null;
}
