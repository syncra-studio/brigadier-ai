import type { Described } from "@/app/conversation/activity/ActivityGroup";
import type { StepWords } from "@/app/conversation/activity/words";
import type { PeerMessage, PeerState } from "@/ipc/generated";

/**
 * How messages between sessions read in the thread (SESSION-MESSAGING-PLAN.md §6): the sender's
 * row and its state line, the receiver's caption, the live words of a request that waits for a
 * reply.
 */

/** Another session as a message names it: its title, and its project when that isn't this one's. */
export type PeerName = { title: string; project: string | null };

/** A session the app no longer has (deleted): named plainly. */
export const UNKNOWN_SESSION: PeerName = { title: "a session", project: null };

/** "API work", or "API work in Website" for a session of another project. */
export function peerLabel(name: PeerName): string {
  return name.project ? `${name.title} in ${name.project}` : name.title;
}

/** Why a message never reached its receiver, in plain words. */
function droppedWords(reason: string): string {
  if (reason === "deleted") return "Couldn’t deliver: the session was deleted";
  if (reason === "archived") return "Couldn’t deliver: the session was archived";
  return `Couldn’t deliver: ${reason}`;
}

/** The words of a row whose verb is `verb` and whose object is the session. */
function words(kind: StepWords["kind"], verb: string, name: PeerName, phrase?: string): StepWords {
  const done = `${verb} ${peerLabel(name)}`;
  return { kind, doing: done, done, object: name.title, ...(phrase && { phrase }) };
}

/** The sender's row of a message, by how it stands. */
export function sentDescribed(state: PeerState, name: PeerName): Described {
  switch (state.type) {
    case "waitingForOk":
      return { words: words("message", "Waiting for your OK to message", name), status: "completed" };
    case "pending":
      return { words: { ...words("message", "Messaging", name), done: `Messaged ${peerLabel(name)}` }, status: "inProgress" };
    case "delivered":
    case "seen":
    case "replied":
      return { words: words("message", "Messaged", name), status: "completed" };
    case "declined":
      return { words: words("other", "Didn’t message", name, "Didn’t message a session"), status: "completed" };
    case "refused":
    case "dropped":
      return { words: words("other", "Couldn’t message", name, "Couldn’t message a session"), status: "completed" };
  }
}

/** A send the engine refused before recording it: "Couldn't message ‹title›". */
export function refusedDescribed(name: PeerName): Described {
  return { words: words("other", "Couldn’t message", name, "Couldn’t message a session"), status: "completed" };
}

/** The state line under a sent message's text; `reply` when "Open reply" can follow it. */
export function stateLine(state: PeerState): { text: string; reply: boolean } {
  switch (state.type) {
    case "waitingForOk":
      return { text: "Waiting for your OK", reply: false };
    case "pending":
      return { text: "Sending", reply: false };
    case "delivered":
      return { text: "Delivered", reply: false };
    case "seen":
      return { text: "Seen", reply: false };
    case "replied":
      return { text: "Replied", reply: true };
    case "declined":
      return { text: "You didn’t send it", reply: false };
    case "refused":
      return { text: state.reason, reply: false };
    case "dropped":
      return { text: droppedWords(state.reason), reply: false };
  }
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

/** The caption over a received message: "From API work · Website", "Reply from API work". */
export function receivedCaption(name: PeerName, reply: boolean, earlier: boolean): string {
  if (!reply) return name.project ? `From ${name.title} · ${name.project}` : `From ${name.title}`;
  return earlier ? `Reply from ${name.title} · to your earlier message` : `Reply from ${name.title}`;
}

/** "Waiting for API work", "Waiting for API work and Docs". */
export function waitingWords(names: readonly string[]): string {
  if (names.length <= 1) return `Waiting for ${names[0] ?? "a reply"}`;
  return `Waiting for ${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;
}

/** The reply that answers `message`, if one came: an incoming record naming it. */
export function replyOf(message: PeerMessage, records: Readonly<Record<string, PeerMessage>>): PeerMessage | null {
  return Object.values(records).find((record) => !record.outgoing && record.replyTo === message.messageId) ?? null;
}
