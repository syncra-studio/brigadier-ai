import { Chat } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, type ReactNode, useEffect, useRef } from "react";
import { useShallow } from "zustand/react/shallow";

import type { Described } from "@/app/conversation/activity/ActivityGroup";
import {
  type PeerName,
  peerLabel,
  peerNameOf,
  receiverHasIt,
  refusedDescribed,
  replyOf,
  sentDescribed,
  stateLine,
  UNKNOWN_SESSION,
} from "@/app/conversation/activity/peerWords";
import type { BlockOrchestratorStep } from "@/app/conversation/blocks";
import { FoldedMarkdown, usePeerName } from "@/app/conversation/peerParts";
import { useViewConversation } from "@/app/conversation/viewContext";
import { ThreadActivity } from "@/components/assistant-ui/elements/thread-activity";
import type { Conversation, OrchestratorStepKind, PeerMessage, Project } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { loadFullText, openConversation } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

/**
 * The sender's row of a message to another session (SESSION-MESSAGING-PLAN.md §6.1): "Messaged
 * API work", the name opening that session at the message; it opens to what it said and how it
 * stands ("Delivered", "Seen", "Replied · Open reply").
 */

type Refused = Extract<OrchestratorStepKind, { type: "messageRefused" }>;

/** What a group needs to word the rows of messages to other sessions. */
export type PeerLookup = {
  records: Readonly<Record<string, PeerMessage>>;
  name: (id: string | undefined) => PeerName | null;
};

const NO_RECORDS: Readonly<Record<string, PeerMessage>> = {};

/** The sessions the message rows among `steps` name. */
function namedIn(steps: readonly BlockOrchestratorStep[], records: Readonly<Record<string, PeerMessage>>): string[] {
  return steps.flatMap(({ kind }) => {
    if (kind.type === "peerSent") return records[kind.messageId]?.peer ?? [];
    if (kind.type === "messageRefused") return kind.to ?? [];
    return [];
  });
}

/**
 * The lookup over the open conversation's records and the sessions `steps` name: it changes
 * only when one of those sessions, or its project, does.
 */
export function usePeerLookup(steps: readonly BlockOrchestratorStep[]): PeerLookup {
  const records = useBoard((s) => s.board?.peerMessages) ?? NO_RECORDS;
  const here = useViewConversation()?.projectId ?? null;
  const ids = namedIn(steps, records);
  const named = useApp(
    useShallow((s) =>
      ids.flatMap((id) => {
        const conversation = s.conversations[id];
        return [conversation, conversation?.projectId ? s.projects[conversation.projectId] : undefined];
      }),
    ),
  );
  const conversations: Record<string, Conversation> = {};
  const projects: Record<string, Project> = {};
  for (let index = 0; index < named.length; index += 2) {
    const conversation = named[index] as Conversation | undefined;
    const project = named[index + 1] as Project | undefined;
    if (conversation) conversations[conversation.id] = conversation;
    if (project) projects[project.id] = project;
  }
  return { records, name: (id) => peerNameOf(conversations, projects, id, here) };
}

/** A message row's words and how it stands; null for a step that isn't one. */
export function describePeerStep(step: BlockOrchestratorStep, peers: PeerLookup): Described | null {
  const { kind } = step;
  if (kind.type === "peerSent") {
    const record = peers.records[kind.messageId];
    const name = peers.name(record?.peer) ?? UNKNOWN_SESSION;
    return sentDescribed(record?.state ?? { type: "pending" }, name);
  }
  if (kind.type === "messageRefused") {
    return refusedDescribed(peers.name(kind.to) ?? (kind.title ? { title: kind.title, project: null } : UNKNOWN_SESSION));
  }
  return null;
}

/** The session's name in a row: a button that opens it (at the message, once it has it). */
const PeerLink: FC<{ id: string; name: PeerName; message?: string }> = ({ id, name, message }) => (
  <button
    type="button"
    data-slot="peer-link"
    title={`Open ${name.title}`}
    onClick={(event) => {
      event.stopPropagation();
      openConversation(id, message ? { at: { message } } : {});
    }}
    className="text-row-object hover:text-foreground focus-visible:text-foreground rounded-control min-w-0 truncate align-bottom"
  >
    {peerLabel(name)}
  </button>
);

/** "Messaged ‹name›": the verb, then the name; the verb shimmers while it goes. */
const RowWords: FC<{ label: string; shown: PeerName; running: boolean; children: ReactNode }> = ({ label, shown, running, children }) => {
  const named = peerLabel(shown);
  const verb = label.endsWith(named) ? label.slice(0, -named.length).trimEnd() : label;
  return (
    <span className="flex min-w-0 items-center gap-1">
      <span className={cn("shrink-0", running && "shimmer")}>{verb}</span>{" "}
      {children}
    </span>
  );
};

/** What a message row opens to: the row recipe's detail, as tall as the message (it folds itself). */
const MESSAGE_DETAIL = "text-foreground/60 flex min-w-0 flex-col gap-1 ps-5.5 pt-1 pb-1 text-sm";

/** How long a row takes to open (the collapsible's animation), with a frame to spare. */
const OPENING_MS = 350;

/**
 * A row opened inside a group's scrolling list follows its opening, so its state line shows: the
 * list scrolls just enough, never past the row's own top, and clears its faded bottom edge.
 */
function keepInList(row: HTMLElement): void {
  let list: HTMLElement | null = null;
  for (let holder = row.parentElement; holder && holder.dataset["slot"] !== "aui_thread-viewport"; holder = holder.parentElement) {
    if (/auto|scroll/.test(getComputedStyle(holder).overflowY)) {
      list = holder;
      break;
    }
  }
  if (!list) return;
  const scroller = list;
  const fade = Number.parseFloat(getComputedStyle(scroller).getPropertyValue("--spacing-group-fade")) * Number.parseFloat(getComputedStyle(document.documentElement).fontSize) || 0;
  const until = performance.now() + OPENING_MS;
  const follow = () => {
    const box = scroller.getBoundingClientRect();
    const rect = row.getBoundingClientRect();
    const by = Math.min(rect.bottom + fade - box.bottom, rect.top - box.top);
    if (by > 0) scroller.scrollTop += by;
    if (performance.now() < until) requestAnimationFrame(follow);
  };
  requestAnimationFrame(follow);
}

/** One sent message's row, from its record. */
export const PeerSentRow: FC<{ step: BlockOrchestratorStep; messageId: string }> = ({ step, messageId }) => {
  const record = useBoard((s) => s.board?.peerMessages[messageId]);
  const here = useViewConversation()?.id ?? null;
  const name = usePeerName(record?.peer);
  const reply = useBoard((s) => (record && s.board ? replyOf(record, s.board.peerMessages) : null));
  const full = useApp((s) => (here ? s.threads[here]?.fullText[messageId] : undefined));
  const blob = record?.blob ?? null;
  const ref = useRef<HTMLDivElement>(null);
  // A long message keeps its text in the blob store: its row shows all of it once read.
  useEffect(() => {
    if (here && blob && full === undefined) void loadFullText(here, messageId, blob).catch((error: unknown) => console.error(error));
  }, [here, blob, full, messageId]);
  if (!record) return null;
  const shown = name ?? UNKNOWN_SESSION;
  const described = sentDescribed(record.state, shown);
  const running = described.status === "inProgress";
  const label = running ? described.words.doing : described.words.done;
  const line = stateLine(record.state);
  const text = full ?? record.text;
  return (
    <ThreadActivity
      data-slot="orchestrator-step"
      data-kind="message"
      data-peer-message={messageId}
      data-state={record.state.type}
      data-at={step.position}
      ref={ref}
      onOpenChange={(open) => {
        if (open && ref.current) keepInList(ref.current);
      }}
      detail={running ? undefined : (
        <div data-slot="peer-sent-detail" className="flex min-w-0 flex-col gap-1.5">
          <FoldedMarkdown text={text} className="text-foreground/80" />
          <p data-slot="peer-state" className="text-muted-foreground text-xs">
            {line.text}
            {line.reply && reply && here && (
              <>
                {" · "}
                <button
                  type="button"
                  data-slot="peer-open-reply"
                  onClick={() => openConversation(here, { at: { message: reply.messageId } })}
                  className="text-link hover:text-link/80"
                >
                  Open reply
                </button>
              </>
            )}
          </p>
        </div>
      )}
      detailClassName={MESSAGE_DETAIL}
    >
      <Chat aria-hidden className="size-4 shrink-0" />
      <RowWords label={label} shown={shown} running={running}>
        {name ? (
          <PeerLink id={record.peer} name={name} {...(receiverHasIt(record.state) && { message: messageId })} />
        ) : (
          <span className="text-row-object min-w-0 truncate">{peerLabel(shown)}</span>
        )}
      </RowWords>
    </ThreadActivity>
  );
};

/** A send refused before anything was recorded: "Couldn't message ‹name›", opening to why. */
export const PeerRefusedRow: FC<{ kind: Refused }> = ({ kind }) => {
  const name = usePeerName(kind.to);
  const shown = name ?? (kind.title ? { title: kind.title, project: null } : UNKNOWN_SESSION);
  const described = refusedDescribed(shown);
  return (
    <ThreadActivity
      data-slot="orchestrator-step"
      data-kind="message"
      data-state="refused"
      detail={<p data-slot="peer-state" className="text-muted-foreground text-xs wrap-break-word">{kind.reason}</p>}
    >
      <Chat aria-hidden className="size-4 shrink-0" />
      <RowWords label={described.words.done} shown={shown} running={false}>
        {name && kind.to ? (
          <PeerLink id={kind.to} name={name} />
        ) : (
          <span className="text-row-object min-w-0 truncate">{peerLabel(shown)}</span>
        )}
      </RowWords>
    </ThreadActivity>
  );
};

/** A lead's step that is a message row, rendered; null for any other step. */
export function peerStepRow(step: BlockOrchestratorStep): ReactNode {
  const { kind } = step;
  if (kind.type === "peerSent") return <PeerSentRow step={step} messageId={kind.messageId} />;
  if (kind.type === "messageRefused") return <PeerRefusedRow kind={kind} />;
  return null;
}
