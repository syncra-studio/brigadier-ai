import type { Described } from "@/app/conversation/activity/ActivityGroup";
import type { Conversation, ItemStatus, PeerMessage, PeerState, Project } from "@/ipc/generated";

/**
 * How messages between sessions read in the thread (SESSION-MESSAGING-PLAN.md §6): the sender's
 * row of what it sent and how it waited, the receiver's caption, the words of a request that
 * waits for a reply.
 */

/** Another session as a message names it: its title, and its project when that isn't this one's. */
export type PeerName = { title: string; project: string | null };

/** A session the app no longer has (deleted): named plainly. */
export const UNKNOWN_SESSION: PeerName = { title: "a session", project: null };

/**
 * A session a row names: its id and title, and the message the row opens it at, if any; `gone`
 * once the app no longer has it (deleted), named plainly with nothing to open.
 */
export type PeerRef = { session: string; title: string; message?: string; gone?: boolean };

/** A row's words: plain text, and the sessions it names, each a link. */
export type WordPart = string | PeerRef;

/** A message the row sent: who to, and how it stands. */
export type PeerSend = PeerRef & { state: PeerState };

/**
 * What one row of messages and waits stands at (SESSION-MESSAGING-PLAN.md §6.1): what it sent,
 * how its waits for the replies stand, whose replies they took, and, while one runs, whom it
 * still waits for.
 */
export type Exchange = {
  sends: readonly PeerSend[];
  waits: readonly ItemStatus[];
  received: readonly PeerRef[];
  waitingOn: readonly PeerRef[];
  /**
   * Just sent, with nothing after it yet in the running turn: it still reads as sending, so the
   * wait for the reply that usually comes next takes the row over without a line in between.
   */
  going?: boolean;
};

/** "A", "A and B", "A, B and C": each session once. */
function named(refs: readonly PeerRef[]): WordPart[] {
  const once = refs.filter((ref, index) => refs.findIndex((other) => other.session === ref.session) === index);
  return once.flatMap((ref, index): WordPart[] => {
    if (index === 0) return [ref];
    return [index === once.length - 1 ? " and " : ", ", ref];
  });
}

/** "message" or "messages", by how many there are. */
function messages(sends: readonly unknown[]): string {
  return sends.length === 1 ? "message" : "messages";
}

/** Whether a message went (or goes now) to its receiver. */
export function went(state: PeerState): boolean {
  return state.type === "pending" || state.type === "delivered" || state.type === "seen" || state.type === "replied";
}

/** The words of a row of messages and waits, and how it stands: "Sent message to Docs, waited for its reply". */
export function exchangeWords({ sends, waits, received, waitingOn, going = false }: Exchange): { parts: WordPart[]; status: ItemStatus; waiting: boolean } {
  const asking = sends.filter((send) => send.state.type === "waitingForOk");
  if (asking.length > 0) return { parts: ["Waiting for your OK to message ", ...named(asking)], status: "completed", waiting: false };
  const sending = sends.filter((send) => send.state.type === "pending");
  if (sending.length > 0) return { parts: [`Sending ${messages(sending)} to `, ...named(sending)], status: "inProgress", waiting: false };
  const sent = sends.filter((send) => went(send.state));
  if (going && waits.length === 0 && sent.length > 0 && sent.length === sends.length) {
    return { parts: [`Sending ${messages(sent)} to `, ...named(sent)], status: "inProgress", waiting: false };
  }
  if (waits.includes("inProgress")) {
    const on = waitingOn.length > 0 ? waitingOn : sent;
    return { parts: on.length > 0 ? ["Waiting for ", ...named(on), " to reply"] : ["Waiting for replies"], status: "inProgress", waiting: true };
  }
  const clauses: WordPart[][] = [];
  if (sent.length > 0) clauses.push([`Sent ${messages(sent)} to `, ...named(sent)]);
  const declined = sends.filter((send) => send.state.type === "declined");
  if (declined.length > 0) clauses.push([`Didn’t send ${messages(declined)} to `, ...named(declined)]);
  const failed = sends.filter((send) => send.state.type === "refused" || send.state.type === "dropped");
  if (failed.length > 0) clauses.push([`Couldn’t send ${messages(failed)} to `, ...named(failed)]);
  if (waits.length > 0) clauses.push(waited(sent, received, waits.at(-1) === "failed", sends.length === 0));
  const parts = clauses.flatMap((clause, index) => (index === 0 ? clause : [", ", ...lower(clause)]));
  return { parts, status: "completed", waiting: false };
}

/** The clause of a row's waits once they are over. */
function waited(sent: readonly PeerRef[], received: readonly PeerRef[], failed: boolean, alone: boolean): WordPart[] {
  const sessions = (refs: readonly PeerRef[]) => new Set(refs.map((ref) => ref.session));
  if (received.length === 0) return failed ? ["Couldn’t wait for a reply"] : ["Waited; no reply yet"];
  const all = sessions(received);
  if (!alone && [...sessions(sent)].every((session) => all.has(session))) {
    return [all.size === 1 ? "Waited for its reply" : "Waited for their replies"];
  }
  return all.size === 1 ? ["Waited for a reply from ", ...named(received)] : ["Waited for replies from ", ...named(received)];
}

/** A clause after the first: its first word in lower case. */
function lower(clause: WordPart[]): WordPart[] {
  const [first, ...rest] = clause;
  return typeof first === "string" ? [first.charAt(0).toLowerCase() + first.slice(1), ...rest] : clause;
}

/** A row's words as one line of text. */
export function plainWords(parts: readonly WordPart[]): string {
  return parts.map((part) => (typeof part === "string" ? part : part.title)).join("");
}

/** A row's words and how it stands, as a group reads them. */
export function exchangeDescribed(exchange: Exchange): Described {
  const { parts, status, waiting } = exchangeWords(exchange);
  const label = plainWords(parts);
  const sent = exchange.sends.some((send) => went(send.state) || send.state.type === "waitingForOk");
  const failed = exchange.sends.length > 0 && !sent;
  const object = parts.find((part): part is PeerRef => typeof part !== "string")?.title;
  return {
    words: {
      kind: failed ? "other" : "message",
      doing: label,
      done: label,
      ...(object !== undefined && { object }),
      ...(failed && { phrase: "Couldn’t message a session" }),
      ...(waiting && { waiting }),
    },
    status,
  };
}

/** A send refused before anything was recorded: "Couldn't send message to Billing". */
export function refusedDescribed(name: PeerName): Described {
  const label = `Couldn’t send message to ${name.title}`;
  return { words: { kind: "other", doing: label, done: label, object: name.title, phrase: "Couldn’t message a session" }, status: "completed" };
}

/** How a message the thread's own `brigadier send` gave another session stands, for its row. */
export type SessionSendState = "sending" | "sent" | "declined" | "refused";

/** Where a session's `brigadier send` stands: running, sent, the user's Deny, or failed. */
export function sessionSendState(status: ItemStatus, refused: string | null): SessionSendState {
  if (status === "inProgress") return "sending";
  if (status === "declined") return "declined";
  return status === "failed" || refused !== null ? "refused" : "sent";
}

/**
 * The words before the session a `brigadier send` row names: "Messaging Docs", then "Messaged
 * Docs" (the message follows, a step fainter), "Didn't send message to Docs", "Couldn't send
 * message to Docs".
 */
export function sessionSendWords(state: SessionSendState): string {
  switch (state) {
    case "sending":
      return "Messaging ";
    case "sent":
      return "Messaged ";
    case "declined":
      return "Didn’t send message to ";
    case "refused":
      return "Couldn’t send message to ";
  }
}

/** A `brigadier send` row's words and how it stands, as a group reads them: "Messaged Docs". */
export function sessionSentDescribed(name: PeerName, state: SessionSendState): Described {
  if (state === "refused" || state === "declined") {
    const label = `${sessionSendWords(state)}${name.title}`;
    return { words: { kind: "other", doing: label, done: label, object: name.title, phrase: state === "declined" ? "Didn’t message a session" : "Couldn’t message a session" }, status: "completed" };
  }
  return {
    words: { kind: "message", doing: `Messaging ${name.title}`, done: `Messaged ${name.title}`, object: name.title },
    status: state === "sending" ? "inProgress" : "completed",
  };
}

/**
 * Whether the row's name opens the receiver at the message itself: only once the receiver has
 * it (a message waiting for the user's OK, declined, or refused before it went has none there).
 */
export function receiverHasIt(state: PeerState): boolean {
  switch (state.type) {
    case "pending":
    case "delivered":
    case "seen":
    case "replied":
    case "dropped":
      return true;
    case "waitingForOk":
    case "declined":
    case "refused":
      return false;
  }
}

/** The caption over a message another session sent: "Sent by API work from another session". */
export function sentByCaption(name: PeerName): string {
  return `Sent by ${name.title} from another session`;
}

/** The reply that answers `message`, if one came: an incoming record naming it. */
export function replyOf(message: PeerMessage, records: Readonly<Record<string, PeerMessage>>): PeerMessage | null {
  return Object.values(records).find((record) => !record.outgoing && record.replyTo === message.messageId) ?? null;
}

/**
 * Another session as this one names it: its title, and its project when that isn't `here`'s;
 * null once the app no longer has it (deleted).
 */
export function peerNameOf(
  conversations: Readonly<Record<string, Conversation>>,
  projects: Readonly<Record<string, Project>>,
  id: string | undefined,
  here: string | null,
): PeerName | null {
  const conversation = id === undefined ? undefined : conversations[id];
  if (!conversation) return null;
  const project = conversation.projectId && conversation.projectId !== here ? projects[conversation.projectId] : undefined;
  return { title: conversation.title, project: project?.name ?? null };
}
