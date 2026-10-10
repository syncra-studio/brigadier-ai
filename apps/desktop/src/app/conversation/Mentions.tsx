import { useAuiState, type TextMessagePartProps, type Unstable_TriggerItem } from "@assistant-ui/react";
import { Chat, File } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, Fragment, useCallback, useEffect, useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { InlineImageText } from "@/app/conversation/InlineImage";
import { WorkerChip, WorkerGlyph } from "@/app/conversation/WorkerChip";
import type { ChipMention } from "@/components/assistant-ui/elements/composer-chips";
import {
  ComposerMentions,
  type MentionOption,
} from "@/components/assistant-ui/elements/composer-mentions";
import type { AttachmentRef, Conversation, Mention } from "@/ipc/generated";
import { listFiles } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

/** A worker the composer can @-mention, `title` its name as the user reads it. */
export type MentionTarget = { id: string; number: number; title: string; state: string };

/** Files matching a query that the menu lists; only a handful show at once. */
const FILE_ROWS = 8;
/** Other conversations the menu lists. */
const CHAT_ROWS = 5;

const TASK = /@task-(\d+)\b/g;

/** What a mention leaves in the text, after `@`. */
function label(mention: Mention, targets: readonly MentionTarget[]): string | null {
  switch (mention.type) {
    case "task": {
      const target = targets.find((entry) => entry.id === mention.id);
      return target ? `task-${target.number}` : null;
    }
    case "file":
      return mention.path;
    case "chat":
      return mention.title;
  }
}

/**
 * A message's text with the workers it @-mentions as their chips; a `@task-N` the session has
 * no worker for stays text.
 */
export const MentionText: FC<{ text: string }> = ({ text }) => {
  const pieces = text.split(TASK);
  // Odd pieces are the numbers `split` captured; each is its worker's id, or "" for none.
  const ids = useBoard(
    useShallow((s) => {
      if (pieces.length === 1) return [];
      const byNumber = new Map(Object.values(s.board?.tasks ?? {}).map((task) => [task.number, task.id]));
      return pieces.map((piece, index) => (index % 2 === 1 ? (byNumber.get(Number(piece)) ?? "") : ""));
    }),
  );
  if (pieces.length === 1) return <>{text}</>;
  return (
    <>
      {pieces.map((piece, index) => {
        if (index % 2 === 0) return <Fragment key={index}>{piece}</Fragment>;
        const id = ids[index];
        return id ? <WorkerChip key={index} taskId={id} /> : <Fragment key={index}>@task-{piece}</Fragment>;
      })}
    </>
  );
};

/** A user message's text part, its @-mentioned workers as chips. */
export const UserMessageText: FC<TextMessagePartProps> = ({ text }) => {
  const attachments = useAuiState((s) => s.message.metadata.custom["attachments"]) as AttachmentRef[] | undefined;
  return <InlineImageText text={text} attachments={attachments ?? []} Text={MentionText} />;
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
    const key = `${mention.type}:${mention.type === "file" ? mention.path : mention.id}`;
    if (name && !seen.has(key) && hasMention(text, name)) {
      seen.add(key);
      mentions.push(mention);
    }
  }
  return mentions;
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
      if (mention.type === "file") this.record(mention, mention.path);
      else if (mention.type === "chat") this.record(mention, mention.title);
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
  const id = mention.type === "file" ? `file:${mention.path}` : `${mention.type}:${mention.id}`;
  return { id, type: mention.type, label: name };
}

/** The mention behind a menu item or chip (see `itemOf`). */
export function mentionOf(item: { id: string; type: string; label: string }): Mention | null {
  const rest = item.id.slice(item.id.indexOf(":") + 1);
  switch (item.type) {
    case "file":
      return { type: "file", path: rest };
    case "chat":
      return { type: "chat", id: rest, title: item.label };
    case "task":
      return { type: "task", id: rest };
    default:
      return null;
  }
}

/** How well a file's path matches: its name starting with the query first. */
function fileRank(path: string, query: string): number {
  const lower = path.toLowerCase();
  const name = lower.slice(lower.lastIndexOf("/") + 1);
  if (name.startsWith(query)) return 0;
  if (name.includes(query)) return 1;
  return lower.includes(query) ? 2 : 3;
}

/** A session checkout's files, refreshed on worker landings and explicit refresh requests. */
export function useCheckoutFiles(
  conversation: Conversation | null,
  refresh = 0,
): { files: string[]; truncated: boolean } | null {
  const id = conversation?.kind === "session" ? conversation.id : null;
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
    listFiles(id)
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

/**
 * The `@` menu with what Brigadier can mention: a session's workers (as `@task-N`) and its
 * checkout's files, and other chats and sessions, whose latest messages go along.
 */
export const Mentions: FC<{
  conversation: Conversation;
  targets: readonly MentionTarget[];
}> = ({ conversation, targets }) => {
  const [refresh, setRefresh] = useState(0);
  const onOpen = useCallback(() => setRefresh((value) => value + 1), []);
  const files = useCheckoutFiles(conversation, refresh);
  const chats = useApp(
    useShallow((s) =>
      Object.values(s.conversations)
        .filter(
          (other) =>
            other.id !== conversation.id && other.lifecycle !== "archived" && !other.sideOf,
        )
        .toSorted((a, b) => b.updatedAtMs - a.updatedAtMs),
    ),
  );
  const search = useCallback(
    (query: string): MentionOption[] => {
      const lower = query.toLowerCase();
      const options: MentionOption[] = [];
      const add = (mention: Mention, name: string, option: Omit<MentionOption, "item">) => {
        options.push({ item: itemOf(mention, name), ...option });
      };
      for (const target of targets) {
        const name = `task-${target.number}`;
        if (!name.includes(lower) && !target.title.toLowerCase().includes(lower)) continue;
        // By its title, as everywhere the user reads it; `task-N` is what it leaves in the text.
        add({ type: "task", id: target.id }, name, {
          icon: <WorkerGlyph taskId={target.id} />,
          name: target.title,
          trailing: target.state,
        });
      }
      if (lower && files) {
        const matches = files.files
          .map((path) => ({ path, rank: fileRank(path, lower) }))
          .filter((entry) => entry.rank < 3)
          .toSorted((a, b) => a.rank - b.rank || a.path.length - b.path.length)
          .slice(0, FILE_ROWS);
        for (const { path } of matches) {
          const slash = path.lastIndexOf("/");
          add({ type: "file", path }, path, {
            icon: <File />,
            name: path.slice(slash + 1),
            detail: slash > 0 ? path.slice(0, slash) : undefined,
          });
        }
      }
      const shown = chats.filter((chat) => chat.title.toLowerCase().includes(lower));
      for (const chat of shown.slice(0, CHAT_ROWS)) {
        add({ type: "chat", id: chat.id, title: chat.title }, chat.title, { icon: <Chat /> });
      }
      return options;
    },
    [targets, files, chats],
  );

  const hint = useMemo(
    () => (query: string) =>
      !query && files && files.files.length > 0 ? "Type to search for files" : null,
    [files],
  );

  return <ComposerMentions search={search} hint={hint} onOpen={onOpen} />;
};
