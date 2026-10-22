import { useEffect } from "react";
import { create } from "zustand";

import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { openUrl, request, systemName } from "@/ipc/client";
import type { FeedbackWriteUp } from "@/ipc/generated";
import {
  type FeedbackResult,
  type FeedbackStep,
  fileWriteUp,
  NOT_SIGNED_IN,
  retry,
  send,
  writeUp,
} from "@/lib/feedbackSend";
import type { FeedbackDetails, FeedbackKind } from "@/lib/feedbackUrl";
import { useApp } from "@/state/store";

/**
 * Send feedback (the sidebar's foot, Help › Send Feedback…, the thread's feedback card): the
 * user only writes; Brigadier splits it into one report per bug or idea in the shape of its
 * repository's issue forms, looks for an issue it repeats, and files each as the user with
 * their `gh` (a repeat as a comment there). Without a `gh` sign-in, Send signs it in first, in
 * place: its one-time code and Open GitHub, then it files on its own once GitHub says yes. With
 * no writer to run, the note opens in one form, as it used to.
 * `openFeedback` (with a draft, from elsewhere) and `sendFeedback` are what other places use.
 */

/**
 * What the dialog opens with: a text already written (the thread's card's Edit), the kind
 * it was drafted as, and what to do once it is sent.
 */
export type FeedbackDraft = { kind: FeedbackKind; text: string; onSent?: (results: FeedbackResult[]) => void };

/**
 * Signing in to GitHub before a send: starting, its one-time code to enter on `url` (`opened`
 * once Open GitHub was pressed, so it shows it waits), or no GitHub CLI to sign in with.
 */
export type FeedbackSignIn =
  | { step: "starting" }
  | { step: "code"; code: string; url: string; opened: boolean }
  | { step: "noGh" };

/**
 * Where a send is: still being written (nothing sent), signing in to GitHub first, at work on
 * a step, done (each report's result, some maybe failed), or failed before anything was filed
 * (`results` when the reports were written and all failed, so Try again files them again
 * without writing them up anew). A sign-in keeps the `results` it would file again.
 */
export type FeedbackSending =
  | { phase: "write" }
  | { phase: "signIn"; signIn: FeedbackSignIn; results: FeedbackResult[] | null }
  | { phase: "working"; step: FeedbackStep }
  | { phase: "done"; results: FeedbackResult[] }
  | { phase: "failed"; error: string; results: FeedbackResult[] | null };

/** Who Send feedback posts as: signed in to GitHub with `gh` (and as whom), or not. */
export type FeedbackAccount = { signedIn: boolean; login: string | null };

/**
 * The dialog, and the send it started: closing it while it works doesn't stop the send, and
 * opening it again shows where that got to.
 */
export const useFeedbackUi = create<{
  open: boolean;
  draft: FeedbackDraft | null;
  text: string;
  sending: FeedbackSending;
  account: FeedbackAccount | null;
}>(() => ({ open: false, draft: null, text: "", sending: { phase: "write" }, account: null }));

/**
 * Opens Send feedback, empty or with `draft` filled in. A send it was closed on keeps showing:
 * at work, done, or failed with the note still there to send again.
 */
export function openFeedback(draft: FeedbackDraft | null = null): void {
  const { sending } = useFeedbackUi.getState();
  // Closing resets the rest; only a send that went on after the dialog closed is here.
  const busy = sending.phase !== "write";
  useFeedbackUi.setState(
    busy ? { open: true } : { open: true, draft, text: draft?.text ?? "", sending: { phase: "write" } },
  );
  void loadAccount();
}

/**
 * The dialog's send now; one closed on (or before) is left to end on its own, unseen, and one
 * closed before it filed anything (asking who it posts as, signing in) stops there.
 */
let dialogSend = 0;

/** Closes it; a send at work carries on, and shows when it opens again. A sign-in stops. */
export function closeFeedback(): void {
  const { sending } = useFeedbackUi.getState();
  if (sending.phase === "working") {
    useFeedbackUi.setState({ open: false });
    return;
  }
  if (sending.phase === "signIn") cancelSignIn();
  dialogSend += 1;
  useFeedbackUi.setState({ open: false, draft: null, text: "", sending: { phase: "write" } });
}

/** The note as typed; once it changes, a failed send's reports are written up from it anew. */
export function setFeedbackText(text: string): void {
  const { sending } = useFeedbackUi.getState();
  useFeedbackUi.setState(
    sending.phase === "failed" && sending.results ? { text, sending: { ...sending, results: null } } : { text },
  );
}

/** The ask for who it posts as under way, which a send waits for when it doesn't know yet. */
let accountAsked: Promise<void> | null = null;

/** Asks the daemon who it posts as (each time it opens: a sign-in may have changed). */
function loadAccount(): Promise<void> {
  accountAsked ??= request({ method: "feedbackAccount" })
    .then(
      ({ signedIn, login }) => useFeedbackUi.setState({ account: { signedIn, login: login ?? null } }),
      (cause: unknown) => console.warn("feedback: who it posts as is unknown", cause),
    )
    .finally(() => {
      accountAsked = null;
    });
  return accountAsked;
}

/** The account, asked for once when nothing asked yet (the thread's card). */
export function useFeedbackAccount(): FeedbackAccount | null {
  const account = useFeedbackUi((s) => s.account);
  useEffect(() => {
    if (!useFeedbackUi.getState().account) void loadAccount();
  }, []);
  return account;
}

/** Why a sign-in to GitHub didn't sign in, in the daemon's plain words. */
class SignInError extends Error {}

/** Stops the GitHub sign-in at work (its dialog closed, its card dismissed). */
export function cancelSignIn(): void {
  void request({ method: "cancelGitHubSignIn" }).catch((cause: unknown) => console.warn("feedback: stopping the sign-in failed", cause));
}

/**
 * Signs `gh` in to GitHub, showing each step with `show`: its code until GitHub says yes.
 * False when there is no GitHub CLI (the step says so; Try again starts over); a sign-in that
 * didn't sign in throws a `SignInError`.
 */
async function signIn(show: (step: FeedbackSignIn) => void): Promise<boolean> {
  show({ step: "starting" });
  try {
    const { signIn: started } = await request({ method: "startGitHubSignIn" });
    if (started.state === "noGh") {
      show({ step: "noGh" });
      return false;
    }
    if (started.state === "code") {
      show({ step: "code", code: started.code, url: started.url, opened: false });
      await request({ method: "waitGitHubSignIn" });
    }
  } catch (cause) {
    throw new SignInError(cause instanceof Error && cause.message ? cause.message : "Signing in to GitHub didn’t finish. Try again.");
  }
  useFeedbackUi.setState({ account: { signedIn: true, login: null } });
  void loadAccount();
  return true;
}

/** Open GitHub: the sign-in's page, and from then on it shows it waits. */
function openSignIn(sending: FeedbackSending, set: (next: FeedbackSending) => void): void {
  if (sending.phase !== "signIn" || sending.signIn.step !== "code") return;
  void openUrl(sending.signIn.url);
  set({ ...sending, signIn: { ...sending.signIn, opened: true } });
}

/** The dialog's Open GitHub. */
export function openDialogSignIn(): void {
  openSignIn(useFeedbackUi.getState().sending, (next) => useFeedbackUi.setState({ sending: next }));
}

/**
 * Whether a send has to sign in first: known to be signed out, or still at a sign-in step;
 * `null` when who it posts as isn't known yet (see `signInOnceKnown`).
 */
function signInFirst(sending: FeedbackSending): boolean | null {
  if (sending.phase === "signIn") return true;
  const account = useFeedbackUi.getState().account;
  return account ? !account.signedIn : null;
}

/**
 * Whether a send has to sign in first, once the daemon said who it posts as; when it couldn't
 * tell, the sign-in finds out (signed in already, it goes straight on).
 */
async function signInOnceKnown(sending: FeedbackSending): Promise<boolean> {
  await loadAccount();
  return signInFirst(sending) ?? true;
}

/** A send's results, the account marked signed out when filing found it so (Try again signs in). */
function filed(results: FeedbackResult[]): FeedbackResult[] {
  if (results.some((result) => result.outcome === "failed" && result.reason === NOT_SIGNED_IN)) {
    useFeedbackUi.setState({ account: { signedIn: false, login: null } });
  }
  return results;
}

/** Sends the dialog's note (or files again what failed), keeping where it is in the store. */
export function sendFromDialog(): void {
  const { text, draft, sending } = useFeedbackUi.getState();
  const failed = sending.phase === "done" || sending.phase === "failed" || sending.phase === "signIn" ? sending.results : null;
  const mine = ++dialogSend;
  const set = (next: FeedbackSending) => {
    if (mine === dialogSend) useFeedbackUi.setState({ sending: next });
  };
  const run = async (): Promise<FeedbackResult[] | null> => {
    const first = signInFirst(sending) ?? (await signInOnceKnown(sending));
    if (first && !(await signIn((step) => set({ phase: "signIn", signIn: step, results: failed })))) return null;
    // Closed (or sent again) while it asked or signed in: this send stops there.
    if (mine !== dialogSend) return null;
    set({ phase: "working", step: failed ? { step: "filing", count: failed.length } : { step: "writing" } });
    return filed(await (failed ? retryFeedback(failed) : sendFeedback(text, draft?.kind, (step) => set({ phase: "working", step }))));
  };
  run().then(
    (results) => {
      if (!results) return;
      set(outcomeOf(results));
      // A Try again that filed some counts too; the caller tells a first send from a later one.
      if (mine === dialogSend && results.some((result) => result.outcome !== "failed")) draft?.onSent?.(results);
    },
    (cause: unknown) => {
      console.warn("feedback: sending failed", cause);
      set({ phase: "failed", error: cause instanceof SignInError ? cause.message : COULDNT_REACH, results: failed });
    },
  );
}

/**
 * The thread's feedback cards, by offer id: the reports written from the offer (`null` when no
 * writer could run: its own text goes in one form) and where sending them is. Kept for the
 * app's run, so a card written up once isn't written again when the thread shows it anew.
 */
export const useFeedbackOffers = create<
  Record<string, { written?: FeedbackWriteUp | null; sending: FeedbackSending } | undefined>
>(() => ({}));

/** Each card's send now, by offer id; a sign-in dismissed with its card is left to end, unseen. */
const offerSends: Record<string, number | undefined> = {};

function setOffer(id: string, next: { written?: FeedbackWriteUp | null; sending: FeedbackSending }): void {
  useFeedbackOffers.setState({ [id]: { ...useFeedbackOffers.getState()[id], ...next } });
}

/** Writes the offer `id`'s `text` up as reports, once. */
export function draftOffer(id: string, text: string): void {
  if (useFeedbackOffers.getState()[id]) return;
  setOffer(id, { sending: { phase: "working", step: { step: "writing" } } });
  void writeUp(text).then((written) => setOffer(id, { written, sending: { phase: "write" } }));
}

/**
 * Sends what the offer `id` shows (or files again what failed), signing in to GitHub first
 * when it has to; `onSent` with what became of them all once one was filed, again when a Try
 * again files the rest.
 */
export function sendOffer(id: string, text: string, kind: FeedbackKind, onSent: (results: FeedbackResult[]) => void): void {
  const offer = useFeedbackOffers.getState()[id];
  const sending = offer?.sending ?? { phase: "write" };
  const written = offer?.written ?? null;
  const failed = sending.phase === "done" || sending.phase === "failed" || sending.phase === "signIn" ? sending.results : null;
  const mine = (offerSends[id] ?? 0) + 1;
  offerSends[id] = mine;
  const set = (next: FeedbackSending) => {
    if (offerSends[id] === mine) setOffer(id, { sending: next });
  };
  const run = async (): Promise<FeedbackResult[] | null> => {
    const first = signInFirst(sending) ?? (await signInOnceKnown(sending));
    if (first && !(await signIn((step) => set({ phase: "signIn", signIn: step, results: failed })))) return null;
    // Dismissed (or sent again) while it asked or signed in: this send stops there.
    if (offerSends[id] !== mine) return null;
    set({ phase: "working", step: { step: "filing", count: failed?.length || written?.reports.length || 1 } });
    return filed(await (failed ? retryFeedback(failed) : fileFeedback(text, kind, written)));
  };
  run().then(
    (results) => {
      if (!results) return;
      const next = outcomeOf(results);
      set(next);
      if (next.phase === "done") onSent(results);
    },
    (cause: unknown) => {
      console.warn("feedback: sending the thread's reports failed", cause);
      set({ phase: "failed", error: cause instanceof SignInError ? cause.message : COULDNT_REACH, results: failed });
    },
  );
}

/** The offer `id`'s Open GitHub. */
export function openOfferSignIn(id: string): void {
  const offer = useFeedbackOffers.getState()[id];
  if (offer) openSignIn(offer.sending, (next) => setOffer(id, { sending: next }));
}

/** The offer `id` was dismissed: a send not filing yet stops, a sign-in it started too. */
export function dismissOffer(id: string): void {
  const phase = useFeedbackOffers.getState()[id]?.sending.phase;
  if (phase === "working") return;
  if (phase === "signIn") cancelSignIn();
  offerSends[id] = (offerSends[id] ?? 0) + 1;
}

/** The offer `id` was sent from the dialog (its Edit): the card shows what was filed. */
export function offerSent(id: string, results: FeedbackResult[]): void {
  setOffer(id, { sending: { phase: "done", results } });
}

/** What the dialog and card say when nothing could be filed. */
export const COULDNT_REACH = "Couldn’t reach GitHub.";

/** Done, unless every report failed: then nothing was sent. */
export function outcomeOf(results: FeedbackResult[]): FeedbackSending {
  const failed = results.filter((result) => result.outcome === "failed");
  if (failed.length === 0 || failed.length < results.length) return { phase: "done", results };
  const reason = failed[0]?.outcome === "failed" ? failed[0].reason : "";
  return { phase: "failed", error: reason ? `Couldn’t send it: ${reason}.` : COULDNT_REACH, results };
}

let system: string | null = null;

/**
 * Looks up this computer's system and processor, once, at startup beside the app's info: Send
 * feedback says it from its first frame instead of changing its words as it arrives (#470).
 */
export async function lookUpSystem(): Promise<void> {
  system = await systemName().catch(() => null);
}

/** What goes with feedback now: this app's version, the system, and the agent CLIs found. */
export function feedbackDetails(): FeedbackDetails {
  const { info, providers } = useApp.getState();
  const agents = (providers.view?.providers ?? []).flatMap(({ provider, status }) =>
    status?.version ? [`${PROVIDER_LABELS[provider]} ${status.version}`] : [],
  );
  return { version: info?.version ?? null, system, agents };
}

/** The details to send, the system asked for again if startup couldn't tell. */
async function sentDetails(): Promise<FeedbackDetails> {
  if (system === null) await lookUpSystem();
  return feedbackDetails();
}

/** What became of one report, and a send's step (see lib/feedbackSend.ts). */
export type { FeedbackResult, FeedbackStep };
export { writeUp as writeUpFeedback };

/**
 * Sends the user's note: written up as reports and filed (the thread's card passes the `kind`
 * it drafted, which only the no-writer fallback uses). Resolves to what became of each report.
 */
export async function sendFeedback(
  text: string,
  kind?: FeedbackKind,
  onStep?: (step: FeedbackStep) => void,
): Promise<FeedbackResult[]> {
  return send(await sentDetails(), text, kind, onStep);
}

/** Files a write-up made first (the thread's card), or with none, the note in one form. */
export async function fileFeedback(
  text: string,
  kind: FeedbackKind,
  written: FeedbackWriteUp | null,
): Promise<FeedbackResult[]> {
  return fileWriteUp(await sentDetails(), text, kind, written);
}

/** Sends the reports that failed again; the others stay as they were. */
export async function retryFeedback(results: readonly FeedbackResult[]): Promise<FeedbackResult[]> {
  return retry(await sentDetails(), results);
}
