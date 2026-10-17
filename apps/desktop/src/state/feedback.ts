import { create } from "zustand";

import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { openUrl, systemName } from "@/ipc/client";
import { type FeedbackDetails, type FeedbackKind, feedbackUrl } from "@/lib/feedbackUrl";
import { useApp } from "@/state/store";

/**
 * Send feedback (the sidebar's foot, Help › Send Feedback…): what the user writes opens as a new
 * issue in Brigadier's repository, in their browser, filled in from the repository's own issue
 * forms. Nothing is sent from the app: they read it, finish the form and post it there.
 * `openFeedback` (with a draft, from elsewhere) and `sendFeedback` are what other places use.
 */

/** What the dialog opens with: a kind and a text already written, to read and change. */
export type FeedbackDraft = { kind: FeedbackKind; text: string };

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

/** Opens `text` as a new issue in the user's browser, filled in with the details; resolves to its link. */
export async function sendFeedback(kind: FeedbackKind, text: string): Promise<string> {
  const url = feedbackUrl(kind, text, await feedbackDetails());
  await openUrl(url);
  return url;
}

