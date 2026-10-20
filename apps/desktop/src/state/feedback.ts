import { useEffect } from "react";
import { create } from "zustand";

import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { request, systemName } from "@/ipc/client";
import type { FeedbackWriteUp } from "@/ipc/generated";
import { type FeedbackResult, type FeedbackStep, fileWriteUp, retry, send, writeUp } from "@/lib/feedbackSend";
import type { FeedbackDetails, FeedbackKind } from "@/lib/feedbackUrl";
import { useApp } from "@/state/store";

/**
 * Send feedback (the sidebar's foot, Help › Send Feedback…, the thread's feedback card): the
 * user only writes; Brigadier splits it into one report per bug or idea in the shape of its
 * repository's issue forms, looks for an issue it repeats, and files each as the user with
 * their `gh` (a repeat as a comment there). Without a `gh` sign-in each report's form opens in
 * the browser, filled in. With no writer to run, the note opens in one form, as it used to.
 * `openFeedback` (with a draft, from elsewhere) and `sendFeedback` are what other places use.
 */

/**
 * What the dialog opens with: a text already written (the thread's card's Edit), the kind
 * it was drafted as, and what to do once it is sent.
 */
export type FeedbackDraft = { kind: FeedbackKind; text: string; onSent?: (results: FeedbackResult[]) => void };

/**
 * Where a send is: still being written (nothing sent), at work on a step, done (each report's
 * result, some maybe failed), or failed before anything was filed (`results` when the reports
 * were written and all failed, so Try again files them again without writing them up anew).
 */
export type FeedbackSending =
  | { phase: "write" }
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

/** Opens Send feedback, empty or with `draft` filled in (a send at work or done keeps showing). */
export function openFeedback(draft: FeedbackDraft | null = null): void {
  const { sending } = useFeedbackUi.getState();
  const busy = sending.phase === "working" || sending.phase === "done";
  useFeedbackUi.setState(
    busy ? { open: true } : { open: true, draft, text: draft?.text ?? "", sending: { phase: "write" } },
  );
  void loadAccount();
}

/** Closes it; a send at work carries on, and shows when it opens again. */
export function closeFeedback(): void {
  const { sending } = useFeedbackUi.getState();
  if (sending.phase === "working") useFeedbackUi.setState({ open: false });
  else useFeedbackUi.setState({ open: false, draft: null, text: "", sending: { phase: "write" } });
}

export function setFeedbackText(text: string): void {
  useFeedbackUi.setState({ text });
}

/** Asks the daemon who it posts as (each time it opens: a sign-in may have changed). */
async function loadAccount(): Promise<void> {
  try {
    const { signedIn, login } = await request({ method: "feedbackAccount" });
    useFeedbackUi.setState({ account: { signedIn, login: login ?? null } });
  } catch (cause) {
    console.warn("feedback: who it posts as is unknown", cause);
  }
}

/** The account, asked for once when nothing asked yet (the thread's card). */
export function useFeedbackAccount(): FeedbackAccount | null {
  const account = useFeedbackUi((s) => s.account);
  useEffect(() => {
    if (!useFeedbackUi.getState().account) void loadAccount();
  }, []);
  return account;
}

/** Sends the dialog's note (or files again what failed), keeping where it is in the store. */
export function sendFromDialog(): void {
  const { text, draft, sending } = useFeedbackUi.getState();
  const failed = sending.phase === "done" || sending.phase === "failed" ? sending.results : null;
  const set = (next: FeedbackSending) => useFeedbackUi.setState({ sending: next });
  set({ phase: "working", step: failed ? { step: "filing", count: failed.length } : { step: "writing" } });
  const sent = failed
    ? retryFeedback(failed)
    : sendFeedback(text, draft?.kind, (step) => set({ phase: "working", step }));
  sent.then(
    (results) => {
      set(outcomeOf(results));
      if (!failed && results.some((result) => result.outcome !== "failed")) draft?.onSent?.(results);
    },
    (cause: unknown) => {
      console.warn("feedback: sending failed", cause);
      set({ phase: "failed", error: COULDNT_REACH, results: failed });
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

function setOffer(id: string, next: { written?: FeedbackWriteUp | null; sending: FeedbackSending }): void {
  useFeedbackOffers.setState({ [id]: { ...useFeedbackOffers.getState()[id], ...next } });
}

/** Writes the offer `id`'s `text` up as reports, once. */
export function draftOffer(id: string, text: string): void {
  if (useFeedbackOffers.getState()[id]) return;
  setOffer(id, { sending: { phase: "working", step: { step: "writing" } } });
  void writeUp(text).then((written) => setOffer(id, { written, sending: { phase: "write" } }));
}

/** Sends what the offer `id` shows (or files again what failed); `onSent` once any was filed. */
export function sendOffer(id: string, text: string, kind: FeedbackKind, onSent: () => void): void {
  const offer = useFeedbackOffers.getState()[id];
  const written = offer?.written ?? null;
  const failed = offer?.sending.phase === "done" || offer?.sending.phase === "failed" ? offer.sending.results : null;
  setOffer(id, { sending: { phase: "working", step: { step: "filing", count: written?.reports.length || 1 } } });
  (failed ? retryFeedback(failed) : fileFeedback(text, kind, written)).then(
    (results) => {
      const next = outcomeOf(results);
      setOffer(id, { sending: next });
      if (next.phase === "done") onSent();
    },
    (cause: unknown) => {
      console.warn("feedback: sending the thread's reports failed", cause);
      setOffer(id, { sending: { phase: "failed", error: COULDNT_REACH, results: failed } });
    },
  );
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

let systemLookup: Promise<string | null> | null = null;

/** What goes with feedback now: this app's version, the system, and the agent CLIs found. */
export async function feedbackDetails(): Promise<FeedbackDetails> {
  systemLookup ??= systemName().catch(() => {
    systemLookup = null;
    return null;
  });
  const { info, providers } = useApp.getState();
  const agents = (providers.view?.providers ?? []).flatMap(({ provider, status }) =>
    status?.version ? [`${PROVIDER_LABELS[provider]} ${status.version}`] : [],
  );
  return { version: info?.version ?? null, system: await systemLookup, agents };
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
  return send(await feedbackDetails(), text, kind, onStep);
}

/** Files a write-up made first (the thread's card), or with none, the note in one form. */
export async function fileFeedback(
  text: string,
  kind: FeedbackKind,
  written: FeedbackWriteUp | null,
): Promise<FeedbackResult[]> {
  return fileWriteUp(await feedbackDetails(), text, kind, written);
}

/** Sends the reports that failed again; the others stay as they were. */
export async function retryFeedback(results: readonly FeedbackResult[]): Promise<FeedbackResult[]> {
  return retry(await feedbackDetails(), results);
}
