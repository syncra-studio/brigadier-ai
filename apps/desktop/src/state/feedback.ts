import { create } from "zustand";

import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { systemName } from "@/ipc/client";
import { type FeedbackResult, retry, send } from "@/lib/feedbackSend";
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
 * What the dialog opens with: a kind and a text already written, to read and change, and what
 * to do once it is sent.
 */
export type FeedbackDraft = { kind: FeedbackKind; text: string; onSent?: () => void };

export const useFeedbackUi = create<{ open: boolean; draft: FeedbackDraft | null }>(() => ({
  open: false,
  draft: null,
}));

/** Opens Send feedback, empty or with `draft` filled in. */
export function openFeedback(draft: FeedbackDraft | null = null): void {
  useFeedbackUi.setState({ open: true, draft });
}

export function closeFeedback(): void {
  useFeedbackUi.setState({ open: false, draft: null });
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

/** What became of one report (see lib/feedbackSend.ts). */
export type { FeedbackResult };

/**
 * Sends the user's note: written up as reports and filed (the thread's card passes the `kind`
 * it drafted, which only the no-writer fallback uses). Resolves to what became of each report.
 */
export async function sendFeedback(text: string, kind?: FeedbackKind): Promise<FeedbackResult[]> {
  return send(await feedbackDetails(), text, kind);
}

/** Sends the reports that failed again; the others stay as they were. */
export async function retryFeedback(results: readonly FeedbackResult[]): Promise<FeedbackResult[]> {
  return retry(await feedbackDetails(), results);
}
