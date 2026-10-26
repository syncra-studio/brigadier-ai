import type { BoardDigest, ThreadNode } from "@/app/conversation/blocks";
import { cliThreadTree } from "@/app/conversation/cliBlocks";
import type { Conversation, ThreadView } from "@/ipc/generated";
import { loadConversation } from "@/state/actions";
import { loadThreadView, useCliThread } from "@/state/cliThread";
import { useApp } from "@/state/store";

/** A message on a thread's shown branch, as the thread reads it: its words, and for an answer its id. */
export type BranchMessage = { role: "user" | "assistant"; texts: string[]; answerId: string | null };

const EMPTY_VIEW: ThreadView = { entries: [], turns: [] };
const EMPTY_DIGEST: BoardDigest = {
  tasks: {},
  questions: {},
  orchestratorSteps: [],
  machineSteps: [],
  decisions: [],
  compactions: {},
};

/**
 * The shown branch of a conversation that may not be open (a sidebar row's Fork and Copy as
 * Markdown): its messages and the CLI's thread are read first when the app doesn't hold them,
 * then built as its open view builds them, oldest first.
 */
export async function storedBranch(conversation: Conversation): Promise<BranchMessage[]> {
  const id = conversation.id;
  await Promise.all([
    useApp.getState().threads[id] ? null : loadConversation(id),
    useCliThread.getState().views[id] ? null : loadThreadView(id),
  ]);
  const thread = useApp.getState().threads[id];
  const tree = cliThreadTree(
    useCliThread.getState().views[id] ?? EMPTY_VIEW,
    id,
    thread?.items ?? [],
    thread?.fullText ?? {},
    [],
    EMPTY_DIGEST,
    { state: "idle", error: null },
    conversation.kind === "session",
  );
  const byId = new Map(tree.nodes.map((node) => [node.id, node]));
  const path: ThreadNode[] = [];
  const seen = new Set<string>();
  for (let node = tree.headId ? byId.get(tree.headId) : undefined; node && !seen.has(node.id); node = node.parentId ? byId.get(node.parentId) : undefined) {
    seen.add(node.id);
    path.push(node);
  }
  return path.toReversed().map(({ kind, block }): BranchMessage => {
    if (kind === "user") {
      const user = block.user;
      const text = user?.kind === "pending" ? user.pending.text : (user?.text ?? "");
      return { role: "user", texts: text ? [text] : [], answerId: null };
    }
    // The answer, not an update line under it (as the open view's items read it).
    const answerId = (block.texts.findLast((text) => !text.update) ?? block.texts.at(-1))?.messageId ?? null;
    return { role: "assistant", texts: block.texts.map((text) => text.text), answerId };
  });
}

/** The thread on its shown branch as Markdown, for "Copy as Markdown". */
export function branchMarkdown(title: string, messages: readonly Pick<BranchMessage, "role" | "texts">[]): string {
  const parts = [`# ${title}`];
  for (const message of messages) {
    const text = message.texts.filter(Boolean).join("\n\n").trim();
    if (!text) continue;
    parts.push(`## ${message.role === "user" ? "User" : "Assistant"}`, text);
  }
  return `${parts.join("\n\n")}\n`;
}

/** The branch's latest answer, what Fork forks from; null before there is one. */
export function latestAnswer(messages: readonly BranchMessage[]): string | null {
  return messages.findLast((message) => message.answerId !== null)?.answerId ?? null;
}
