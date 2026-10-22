import { lazy, Suspense } from "react";

import { ComposerStandIn } from "@/app/conversation/ComposerStandIn";
import type { ComposerInputProps } from "@/components/conversation/ComposerEditor";
import { whenRevealed } from "@/lib/splash";

// Loaded once the startup screen has gone: the plain field stands in until then, and on a slow
// machine loading and mounting the editor would hold up startup.
const ComposerEditor = lazy(() =>
  whenRevealed().then(() => import("@/components/conversation/ComposerEditor")),
);

/**
 * The text field: assistant-ui's Lexical input with mention/command chips (its own chunk, so
 * it doesn't hold up startup), a plain field on the same composer text until it loads.
 */
export function ComposerInput(props: ComposerInputProps) {
  const { placeholder, autoFocus } = props;
  return (
    <div className="relative min-w-0">
      <ComposerField placeholder={placeholder} autoFocus={autoFocus} />
    </div>
  );
}

function ComposerField(props: ComposerInputProps) {
  const { placeholder, autoFocus } = props;
  return (
    <Suspense fallback={<ComposerStandIn placeholder={placeholder} autoFocus={autoFocus} />}>
      <ComposerEditor {...props} />
    </Suspense>
  );
}
