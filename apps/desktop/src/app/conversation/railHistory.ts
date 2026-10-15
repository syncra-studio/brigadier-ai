import type { ThreadNode, ThreadTree } from "@/app/conversation/blocks";
import type { RailEntry } from "@/components/assistant-ui/message-rail";

/**
 * The user messages on `full`'s shown branch that `shown` (its folded form) leaves out, oldest
 * first, each with the start of its answer: the message rail marks them above the shown ones.
 */
export function foldedEntries(full: ThreadTree, shown: ThreadTree): RailEntry[] {
  const kept = new Set(shown.nodes.map((node) => node.id));
  const byId = new Map(full.nodes.map((node) => [node.id, node]));
  const path: ThreadNode[] = [];
  const seen = new Set<string>();
  for (let node = full.headId ? byId.get(full.headId) : undefined; node && !seen.has(node.id); node = node.parentId ? byId.get(node.parentId) : undefined) {
    seen.add(node.id);
    path.unshift(node);
  }
  return path.flatMap((node) => {
    if (node.kind !== "user" || kept.has(node.id)) return [];
    const user = node.block.user;
    const text = user?.kind === "pending" ? user.pending.text : (user?.text ?? "");
    const answer = node.block.texts.map((part) => part.text).join(" ").slice(0, 600);
    return [{ id: node.id, text: text.slice(0, 600), answer }];
  });
}
