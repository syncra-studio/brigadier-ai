import { ContextRing } from "@/components/conversation";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

/** The context ring left of the model picker, once the model has said how full it is. */
export function ComposerContextRing() {
  const show = useApp((s) => s.settings.showContextUsage);
  const usage = useBoard((s) => s.board?.context ?? null);
  if (!show || !usage) return null;
  return <ContextRing usage={usage} />;
}
