import { useAuiState, type TextMessagePartProps, type Unstable_TriggerItem } from "@assistant-ui/react";
import { type FC, type ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { WorkerChip, WorkerGlyph } from "@/app/conversation/WorkerChip";
import type { ChipMention } from "@/components/assistant-ui/elements/composer-chips";
import {
  ComposerMentions,
  type MentionGroup,
} from "@/components/assistant-ui/elements/composer-mentions";
import { MessageInlineImage } from "@/components/assistant-ui/elements/inline-image";
import { InlineImageContext, InlinePasteContext, TaskMentionContext } from "@/components/assistant-ui/file-links";
import { MessagePastedText } from "@/components/assistant-ui/elements/pasted-text";
import { MarkdownBlock } from "@/components/assistant-ui/thread";
import { Chat, File, Folder, Globe } from "@/components/ui/icon";
import type { AttachmentRef, Conversation, Mention } from "@/ipc/generated";
import { normalizedImages } from "@/lib/inlineImages";
import { foldersOf, mentionSections, type MentionRow, type MentionTab, type MentionTarget } from "@/app/conversation/mentionSearch";
import { listFiles, listProjectFiles } from "@/state/actions";
import { useBoard } from "@/state/board";
import { type BrowserTabState, sessionTabs, sessionWith, useSessionTabsOf } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

export type { MentionTarget };

const TASK = /@task-(\d+)\b/g;

/** What a mention leaves in the text, after `@`. */
function label(mention: Mention, targets: readonly MentionTarget[]): string | null {
  switch (mention.type) {
    case "task": {
      const target = targets.find((entry) => entry.id === mention.id);
      return target ? `task-${target.number}` : null;
    }
    case "file":
    case "folder":
      return mention.path;
    case "chat":
      return mention.title;
    case "browserTab":
      return mention.title.trim() || mention.url;
    case "page":
      // Sent from a page's own composer, never written in the text.
      return null;
  }
}

/** A worker the user @-mentioned, by its number: its chip, else the mention as written. */
function TaskByNumber({ number }: { number: number }) {
  const id = useBoard((s) => Object.values(s.board?.tasks ?? {}).find((task) => task.number === number)?.id ?? null);
  return id ? <WorkerChip taskId={id} /> : <>@task-{number}</>;
}

const taskChip = (number: number) => <TaskByNumber number={number} />;

/**
 * The user's own words as they wrote them in Markdown (as the answer renders, without its
 * streaming fade), as one document: each line break kept, the workers they @-mentioned as chips
 * and the images and texts they pasted as chips where they pasted them, never inside code.
 */
export const UserMarkdownText: FC<{ text: string; attachments: readonly AttachmentRef[] }> = ({ text, attachments }) => {
  const normalized = useMemo(() => normalizedImages(text, attachments), [text, attachments]);
  const imageChip = useCallback((n: number) => (
    <MessageInlineImage image={normalized.attachments.find((ref) => ref.inline === n && !ref.pasted)} n={n} />
  ), [normalized]);
  const pasteChip = useCallback((n: number) => (
    <MessagePastedText paste={normalized.attachments.find((ref) => ref.inline === n && ref.pasted)} n={n} />
  ), [normalized]);
  return (
    <TaskMentionContext.Provider value={taskChip}>
      <InlineImageContext.Provider value={imageChip}>
        <InlinePasteContext.Provider value={pasteChip}>
          <MarkdownBlock text={normalized.text} breaks />
        </InlinePasteContext.Provider>
      </InlineImageContext.Provider>
    </TaskMentionContext.Provider>
  );
};

/** A user message's text part, its @-mentioned workers as chips. */
export const UserMessageText: FC<TextMessagePartProps> = ({ text }) => {
  const attachments = useAuiState((s) => s.message.metadata.custom["attachments"]) as AttachmentRef[] | undefined;
  const mentions = useAuiState((s) => s.message.metadata.custom["mentions"]) as Mention[] | undefined;
  return <>
    {pageMentions(mentions).map((page) => page.type === "page" && (
      <span key={page.id} data-slot="page-mention" className="text-muted-foreground mb-1 block text-sm">
        From page: {page.title}
      </span>
    ))}
    <UserMarkdownText text={text} attachments={attachments ?? []} />
  </>;
};

/**
 * Whether `text` has `@name` whole, not as the start of a longer name (`@a.ts` in
 * `@a.tsx`, `@src` in `@src/lib`); a sentence's full stop after it still counts.
 */
function hasMention(text: string, name: string): boolean {
  const token = `@${name}`;
  for (let at = text.indexOf(token); at >= 0; at = text.indexOf(token, at + 1)) {
    if (!/^(?:[\w/-]|\.\w)/u.test(text.slice(at + token.length))) return true;
  }
  return false;
}

/**
 * What a message mentions: the workers it names as `@task-N`, and the files and
 * conversations picked from the menu (`known`) whose `@name` is still in the text.
 */
export function mentionsIn(
  text: string,
  targets: readonly MentionTarget[],
  known: Iterable<Mention>,
): Mention[] {
  const mentions: Mention[] = [];
  const tasks = new Set<string>();
  for (const match of text.matchAll(TASK)) {
    const target = targets.find((entry) => entry.number === Number(match[1]));
    if (target && !tasks.has(target.id)) {
      tasks.add(target.id);
      mentions.push({ type: "task", id: target.id });
    }
  }
  const seen = new Set<string>();
  for (const mention of known) {
    if (mention.type === "task") continue;
    const name = label(mention, targets);
    const key = itemOf(mention, name ?? "").id;
    if (name && !seen.has(key) && hasMention(text, name)) {
      seen.add(key);
      mentions.push(mention);
    }
  }
  return mentions;
}

/**
 * The page a message was sent from, which it keeps when edited: no `@name` in the text
 * stands for it.
 */
export function pageMentions(mentions: readonly Mention[] | undefined): Mention[] {
  return mentions?.filter((mention) => mention.type === "page") ?? [];
}

/** The files and conversations the `@` menu put in the composer, by their `@name`. */
export class MentionMemory {
  private readonly byLabel = new Map<string, Mention>();
  /** The open session's workers, whose `@task-N` chips too. */
  private workers: readonly MentionTarget[] = [];

  setWorkers(workers: readonly MentionTarget[]): void {
    this.workers = workers;
  }

  record(mention: Mention, name: string): void {
    this.byLabel.set(name, mention);
  }

  /** Remembers the files and conversations a message, queued item or draft mentions. */
  recall(mentions: readonly Mention[]): void {
    for (const mention of mentions) {
      if (mention.type === "file" || mention.type === "folder") this.record(mention, mention.path);
      else if (mention.type === "chat") this.record(mention, mention.title);
      else if (mention.type === "browserTab") this.record(mention, mention.title.trim() || mention.url);
    }
  }

  known(): Iterable<Mention> {
    return this.byLabel.values();
  }

  /** Each remembered mention with its `@name`. */
  entries(): Iterable<[string, Mention]> {
    return this.byLabel.entries();
  }

  /**
   * The mention whose name starts at `at` in `text` (just past an `@`): the longest remembered
   * file or conversation, else a worker as `task-N`.
   */
  match(text: string, at: number): ChipMention | null {
    let best: [string, Mention] | null = null;
    for (const entry of this.byLabel) {
      if (text.startsWith(entry[0], at) && entry[0].length > (best?.[0].length ?? 0)) best = entry;
    }
    if (best) return itemOf(best[1], best[0]);
    const task = /^task-(\d+)/.exec(text.slice(at));
    const target = task && this.workers.find((entry) => entry.number === Number(task[1]));
    return target && task ? itemOf({ type: "task", id: target.id }, task[0]) : null;
  }
}

/** A mention's menu item: its id is unique across kinds, its label goes in the text. */
function itemOf(mention: Mention, name: string): Unstable_TriggerItem {
  const id = "path" in mention ? `${mention.type}:${mention.path}`
    : mention.type === "browserTab" ? `${mention.type}:${mention.tab}`
    : `${mention.type}:${mention.id}`;
  return { id, type: mention.type, label: name };
}

/** The mention behind a menu item or chip (see `itemOf`). */
export function mentionOf(item: { id: string; type: string; label: string }): Mention | null {
  const rest = item.id.slice(item.id.indexOf(":") + 1);
  switch (item.type) {
    case "file":
      return { type: "file", path: rest };
    case "folder":
      return { type: "folder", path: rest };
    case "chat":
      return { type: "chat", id: rest, title: item.label };
    case "task":
      return { type: "task", id: rest };
    case "browserTab": {
      // Its page as the tab shows it now; the tab gone, the address is unknown.
      const session = sessionWith(rest);
      const tab = session ? sessionTabs(session).tabs.find((each) => each.id === rest) : undefined;
      return { type: "browserTab", tab: rest, title: item.label, url: tab?.kind === "browser" ? tab.url : "" };
    }
    default:
      return null;
  }
}

/** What a project's listing is keyed by in `useCheckoutFiles`, a session's being its id. */
const PROJECT = "project:";

/**
 * A session checkout's files (or, for a session not started yet, its project folder's),
 * refreshed on worker landings and explicit refresh requests.
 */
export function useCheckoutFiles(
  conversation: Conversation | null,
  refresh = 0,
  projectId: string | null = null,
): { files: string[]; truncated: boolean } | null {
  const id = conversation
    ? conversation.kind === "session" ? conversation.id : null
    : projectId && `${PROJECT}${projectId}`;
  const landed = useBoard((s) =>
    s.board?.conversationId === id && s.board
      ? Object.values(s.board.tasks).filter((task) => task.landed !== null).length
      : 0,
  );
  const [fetched, setFetched] = useState<{
    id: string;
    files: string[];
    truncated: boolean;
  } | null>(null);
  useEffect(() => {
    if (!id) return;
    let live = true;
    (id.startsWith(PROJECT) ? listProjectFiles(id.slice(PROJECT.length)) : listFiles(id))
      .then((list) => {
        if (!live) return;
        setFetched((previous) =>
          previous?.id === id &&
          previous.truncated === list.truncated &&
          previous.files.length === list.files.length &&
          previous.files.every((file, index) => file === list.files[index])
            ? previous
            : { id, ...list },
        );
      })
      .catch((error: unknown) => {
        console.error("listing the checkout's files failed", error);
        if (!live) return;
        // Keep the last good list when a refresh fails; with none, show an empty one.
        setFetched((previous) =>
          previous?.id === id ? previous : { id, files: [], truncated: false },
        );
      });
    return () => {
      live = false;
    };
    // A menu opening explicitly requests a fresh listing even when the checkout is unchanged.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [id, landed, refresh]);
  return fetched && fetched.id === id ? fetched : null;
}

/** A menu row's icon: a worker's glyph, else its kind's. */
function iconOf(mention: Mention): ReactNode {
  switch (mention.type) {
    case "task":
      return <WorkerGlyph taskId={mention.id} />;
    case "folder":
      return <Folder />;
    case "chat":
      return <Chat />;
    case "browserTab":
      return <Globe />;
    default:
      return <File />;
  }
}

/**
 * The `@` menu with what Brigadier can mention: a session's workers (as `@task-N`), its
 * checkout's files and folders, and other chats and sessions, whose latest messages go along.
 * On a new chat, `projectId` names the project a session will start in, for its files.
 */
export const Mentions: FC<{
  conversation: Conversation | null;
  targets: readonly MentionTarget[];
  projectId?: string | null;
}> = ({ conversation, targets, projectId = null }) => {
  const [refresh, setRefresh] = useState(0);
  const onOpen = useCallback(() => setRefresh((value) => value + 1), []);
  const files = useCheckoutFiles(conversation, refresh, projectId);
  const sessionTabsNow = useSessionTabsOf(conversation?.kind === "session" ? conversation.id : null);
  const tabs = useMemo((): MentionTab[] => sessionTabsNow.tabs
    .filter((tab): tab is BrowserTabState => tab.kind === "browser" && Boolean(tab.url))
    .map((tab) => ({ id: tab.id, title: tab.title, url: tab.url })), [sessionTabsNow]);
  const folders = useMemo(() => foldersOf(files?.files ?? []), [files]);
  const chats = useApp(
    useShallow((s) =>
      Object.values(s.conversations)
        .filter(
          (other) =>
            other.id !== conversation?.id && other.lifecycle !== "archived" && !other.sideOf,
        )
        .toSorted((a, b) => b.updatedAtMs - a.updatedAtMs),
    ),
  );
  const search = useCallback(
    (query: string): MentionGroup[] => {
      const option = (row: MentionRow) => ({
        item: itemOf(row.mention, row.label),
        icon: iconOf(row.mention),
        name: row.name,
        detail: row.detail,
        trailing: row.trailing,
      });
      return mentionSections(query, { workers: targets, tabs, files: files?.files ?? [], folders, chats }).map(
        (section) => ({ title: section.title, options: section.rows.map(option), note: section.note }),
      );
    },
    [targets, tabs, files, folders, chats],
  );

  return <ComposerMentions search={search} onOpen={onOpen} />;
};
