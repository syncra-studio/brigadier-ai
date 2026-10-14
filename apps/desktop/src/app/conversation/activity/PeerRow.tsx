import { Chat, Clock } from "@openai/apps-sdk-ui/components/Icon";
import type { FC, ReactNode } from "react";
import { useShallow } from "zustand/react/shallow";

import type { Described } from "@/app/conversation/activity/ActivityGroup";
import {
  type Exchange,
  exchangeDescribed,
  exchangeWords,
  type PeerName,
  peerNameOf,
  type PeerRef,
  receiverHasIt,
  refusedDescribed,
  UNKNOWN_SESSION,
} from "@/app/conversation/activity/peerWords";
import { type BlockOrchestratorStep, heldForWait, type PeerSentStep } from "@/app/conversation/blocks";
import { usePeerName } from "@/app/conversation/peerParts";
import { useViewConversation } from "@/app/conversation/viewContext";
import { ThreadActivity } from "@/components/assistant-ui/elements/thread-activity";
import type { Conversation, OrchestratorStepKind, PeerMessage, Project } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { openConversation } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

/**
 * The sender's row of its messages to other sessions and its waits for their replies
 * (SESSION-MESSAGING-PLAN.md §6.1), one line: "Sending message to Docs", then "Waiting for Docs
 * to reply" while the turn waits, then "Sent message to Docs, waited for its reply". Each name
 * opens that session at the message; the row itself opens to nothing.
 */

type Refused = Extract<OrchestratorStepKind, { type: "messageRefused" }>;

/** What a group needs to word the rows of messages to other sessions. */
export type PeerLookup = {
  records: Readonly<Record<string, PeerMessage>>;
  name: (id: string | undefined) => PeerName | null;
};

const NO_RECORDS: Readonly<Record<string, PeerMessage>> = {};

/** The records a row's waits took replies with, or wait for: by the call, or by its request. */
function waitRecords(kind: PeerSentStep, records: Readonly<Record<string, PeerMessage>>): { taken: PeerMessage[]; asked: PeerMessage[] } {
  const calls = new Set(kind.waits.map((wait) => wait.itemId));
  const requests = new Set(kind.waits.flatMap((wait) => (wait.requestId ? [wait.requestId] : [])));
  const all = Object.values(records);
  return {
    taken: all.filter((record) => !record.outgoing && record.hold?.type === "taken" && calls.has(record.hold.itemId)),
    asked: all.filter((record) => record.outgoing && !record.replyTo && !record.automatic && requests.has(record.requestId ?? "")),
  };
}

/** The sessions the message rows among `steps` name. */
function namedIn(steps: readonly BlockOrchestratorStep[], records: Readonly<Record<string, PeerMessage>>): string[] {
  return steps.flatMap(({ kind }) => {
    if (kind.type === "peerSent") {
      const { taken, asked } = waitRecords(kind, records);
      return [...kind.messageIds.flatMap((id) => records[id]?.peer ?? []), ...taken.map((record) => record.peer), ...asked.map((record) => record.peer)];
    }
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

/**
 * What a row of messages and waits stands at, from the records: each message sent, the replies
 * its waits took (each naming the message it answers, which the other session has), and whom a
 * running wait still waits for.
 */
export function exchangeOf(kind: PeerSentStep, peers: PeerLookup, going = false): Exchange {
  const { records } = peers;
  const ref = (record: PeerMessage, message?: string): PeerRef => {
    const name = peers.name(record.peer);
    return {
      session: record.peer,
      title: (name ?? UNKNOWN_SESSION).title,
      ...(message !== undefined && { message }),
      ...(!name && { gone: true }),
    };
  };
  const sent = kind.messageIds.flatMap((id) => records[id] ?? []);
  const replies = Object.values(records).filter((record) => !record.outgoing && heldForWait(record));
  const answered = (record: PeerMessage) => replies.some((reply) => reply.replyTo === record.messageId);
  const { taken, asked } = waitRecords(kind, records);
  const received = sent.length > 0
    ? sent.filter(answered).map((record) => ref(record, record.messageId))
    : taken.map((reply) => ref(reply, reply.replyTo));
  const due = (sent.length > 0 ? sent : asked).filter((record) =>
    !answered(record) && (record.state.type === "pending" || record.state.type === "delivered" || record.state.type === "seen") && !record.settled,
  );
  return {
    sends: sent.map((record) => ({ ...ref(record, receiverHasIt(record.state) ? record.messageId : undefined), state: record.state })),
    waits: kind.waits.map((wait) => wait.status),
    received,
    waitingOn: due.map((record) => ref(record, record.messageId)),
    going,
  };
}

/** A message row's words and how it stands (`going`: see `Exchange`); null for a step that isn't one. */
export function describePeerStep(step: BlockOrchestratorStep, peers: PeerLookup, going = false): Described | null {
  const { kind } = step;
  if (kind.type === "peerSent") return exchangeDescribed(exchangeOf(kind, peers, going));
  if (kind.type === "messageRefused") {
    return refusedDescribed(peers.name(kind.to) ?? (kind.title ? { title: kind.title, project: null } : UNKNOWN_SESSION));
  }
  return null;
}

/** A session a row names: a button that opens it (at the message, once it has it). */
const PeerLink: FC<{ peer: PeerRef }> = ({ peer }) => (
  <button
    type="button"
    data-slot="peer-link"
    title={`Open ${peer.title}`}
    onClick={(event) => {
      event.stopPropagation();
      openConversation(peer.session, peer.message ? { at: { message: peer.message } } : {});
    }}
    className="text-row-object hover:text-foreground focus-visible:text-foreground rounded-control align-bottom"
  >
    {peer.title}
  </button>
);

/** Why the row's messages didn't go, for its tooltip. */
function failures(records: readonly PeerMessage[]): string | undefined {
  const reasons = records.flatMap((record) => (record.state.type === "refused" || record.state.type === "dropped" ? [record.state.reason] : []));
  return reasons.length > 0 ? reasons.join("\n") : undefined;
}

/** How the row stands, for its `data-state`. */
function rowState(exchange: Exchange, status: string, waiting: boolean): string {
  if (waiting) return "waiting";
  if (status === "inProgress") return "sending";
  if (exchange.sends.some((send) => send.state.type === "waitingForOk")) return "waitingForOk";
  return exchange.waits.length > 0 ? "waited" : (exchange.sends[0]?.state.type ?? "waited");
}

/** One row of messages and waits, from their records. */
export const PeerSentRow: FC<{ step: BlockOrchestratorStep; kind: PeerSentStep; going: boolean }> = ({ step, kind, going }) => {
  const peers = usePeerLookup([step]);
  const exchange = exchangeOf(kind, peers, going);
  const { parts, status, waiting } = exchangeWords(exchange);
  if (kind.messageIds.length > 0 && exchange.sends.length === 0) return null;
  const running = status === "inProgress";
  const Icon = waiting ? Clock : Chat;
  const tip = failures(kind.messageIds.flatMap((id) => peers.records[id] ?? []));
  return (
    <ThreadActivity
      data-slot="orchestrator-step"
      data-kind="message"
      data-peer-message={kind.messageIds.join(" ") || undefined}
      data-state={rowState(exchange, status, waiting)}
      data-at={step.position}
      {...(tip && { title: tip })}
    >
      <Icon aria-hidden className="size-4 shrink-0" />
      <span data-slot="peer-words" className="min-w-0 truncate">
        {parts.map((part, index) =>
          typeof part === "string" ? (
            <span key={index} className={cn(running && "shimmer")}>{part}</span>
          ) : part.gone ? (
            <span key={index} className="text-row-object">{part.title}</span>
          ) : (
            <PeerLink key={index} peer={part} />
          ),
        )}
      </span>
    </ThreadActivity>
  );
};

/** A send refused before anything was recorded: "Couldn't send message to ‹name›", why in its tooltip. */
export const PeerRefusedRow: FC<{ kind: Refused }> = ({ kind }) => {
  const name = usePeerName(kind.to);
  const title = (name ?? (kind.title ? { title: kind.title, project: null } : UNKNOWN_SESSION)).title;
  return (
    <ThreadActivity data-slot="orchestrator-step" data-kind="message" data-state="refused" title={kind.reason}>
      <Chat aria-hidden className="size-4 shrink-0" />
      <span data-slot="peer-words" className="min-w-0 truncate">
        Couldn’t send message to{" "}
        {name && kind.to ? <PeerLink peer={{ session: kind.to, title }} /> : <span className="text-row-object">{title}</span>}
      </span>
    </ThreadActivity>
  );
};

/** A lead's step that is a message row, rendered (`going`: see `Exchange`); null for any other step. */
export function peerStepRow(step: BlockOrchestratorStep, going = false): ReactNode {
  const { kind } = step;
  if (kind.type === "peerSent") return <PeerSentRow step={step} kind={kind} going={going} />;
  if (kind.type === "messageRefused") return <PeerRefusedRow kind={kind} />;
  return null;
}
