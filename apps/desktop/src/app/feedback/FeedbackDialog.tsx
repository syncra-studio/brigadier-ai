import { ArrowUpRight, Bug, Eye, Lightbulb, X } from "@openai/apps-sdk-ui/components/Icon";
import { type ReactNode, useEffect, useState } from "react";

import { ErrorLine, errorText } from "@/app/dialogs/fields";
import { GhostButton, LightButton } from "@/app/routing/parts";
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { detailsItems, type FeedbackKind } from "@/lib/feedbackUrl";
import { cn } from "@/lib/utils";
import { closeFeedback, feedbackDetails, type FeedbackDraft, sendFeedback, useFeedbackUi } from "@/state/feedback";

const KINDS = [
  { value: "bug", label: "Something’s wrong", icon: <Bug /> },
  { value: "idea", label: "An idea", icon: <Lightbulb /> },
] as const satisfies readonly { value: FeedbackKind; label: string; icon: ReactNode }[];

const ASK: Record<FeedbackKind, string> = {
  bug: "What happened, and what did you expect?",
  idea: "What are you trying to do, and what gets in the way?",
};

/**
 * Send feedback (Help › Send Feedback…, the sidebar's help menu, the search palette, or a draft
 * from elsewhere), drawn to the user's design: a bug or an idea, written here and opened as a
 * new issue on GitHub, filled in, with the versions it carries shown as chips. Continue opens it
 * in the browser and closes the dialog; the user reads it there before posting.
 */
export function FeedbackDialog() {
  const open = useFeedbackUi((s) => s.open);
  const draft = useFeedbackUi((s) => s.draft);
  return (
    <Dialog open={open} onOpenChange={(next) => !next && closeFeedback()}>
      {open && <Feedback draft={draft} />}
    </Dialog>
  );
}

function Feedback({ draft }: { draft: FeedbackDraft | null }) {
  const [kind, setKind] = useState<FeedbackKind>(draft?.kind ?? "bug");
  const [text, setText] = useState(draft?.text ?? "");
  const [included, setIncluded] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    void feedbackDetails().then((found) => live && setIncluded(detailsItems(found)));
    return () => {
      live = false;
    };
  }, []);

  const send = () => {
    setError(null);
    sendFeedback(kind, text).then(
      () => {
        draft?.onSent?.();
        closeFeedback();
      },
      (cause: unknown) => setError(errorText(cause)),
    );
  };

  return (
    <DialogContent
      data-slot="feedback"
      showCloseButton={false}
      className="bg-settings-surface border-settings-border shadow-design-dialog max-w-feedback block gap-0 rounded-design-dialog border p-0 ring-0"
    >
      <div className="flex gap-3.5 pt-6 pe-5 pb-4.5 ps-6">
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <DialogTitle className="text-dialog-title text-settings-text font-medium">Send feedback</DialogTitle>
          <DialogDescription className="text-settings-small text-settings-text-secondary">
            Opens a GitHub issue with your note filled in.
          </DialogDescription>
        </div>
        <DialogClose
          aria-label="Close"
          className="text-settings-text-secondary hover:text-settings-text hover:bg-settings-raised flex size-7 shrink-0 items-center justify-center rounded-full transition-colors"
        >
          <X className="size-3.75" />
        </DialogClose>
      </div>

      <div className="flex flex-col gap-3.5 px-6 pb-5.5">
        <div
          role="radiogroup"
          aria-label="Kind"
          className="bg-settings-inset border-settings-divider flex gap-0.75 rounded-full border p-0.75"
        >
          {KINDS.map((choice) => {
            const active = choice.value === kind;
            return (
              <button
                key={choice.value}
                type="button"
                role="radio"
                aria-checked={active}
                data-state={active ? "on" : "off"}
                onClick={() => setKind(choice.value)}
                className={cn(
                  "text-settings-small flex h-7.5 flex-1 items-center justify-center gap-1.75 rounded-full border font-medium transition-colors [&_svg]:size-3.5",
                  active
                    ? "bg-settings-raised border-settings-ring text-settings-text"
                    : "text-settings-text-secondary hover:text-settings-text border-transparent",
                )}
              >
                {choice.icon}
                {choice.label}
              </button>
            );
          })}
        </div>

        <textarea
          value={text}
          onChange={(event) => setText(event.target.value)}
          spellCheck
          autoFocus
          placeholder={ASK[kind]}
          aria-label={ASK[kind]}
          className="bg-settings-inset border-settings-border focus-visible:border-settings-accent text-settings-body text-settings-text placeholder:text-settings-text-tertiary rounded-design-row h-37 resize-none border px-3.5 py-3 leading-normal outline-none"
        />

        {included.length > 0 && (
          <div className="flex flex-col gap-2 pt-1">
            <span className="text-settings-meta text-settings-text-tertiary">Also included</span>
            <ul aria-label="Also included" className="flex flex-wrap gap-1.5">
              {included.map((item) => (
                <li
                  key={item}
                  className="bg-feedback-chip text-settings-badge text-settings-text-secondary rounded-design-badge px-2 py-1 font-mono"
                >
                  {item}
                </li>
              ))}
            </ul>
          </div>
        )}

        <ErrorLine error={error} />
      </div>

      <div className="border-settings-divider flex items-center gap-2 border-t px-6 py-4">
        <span className="text-settings-meta text-settings-text-tertiary flex min-w-0 flex-1 items-center gap-1.75">
          <Eye className="size-3.25 shrink-0" />
          You’ll review it before it’s posted
        </span>
        <DialogClose asChild>
          <GhostButton>Cancel</GhostButton>
        </DialogClose>
        <LightButton disabled={!text.trim()} onClick={send} className="gap-1.5 ps-3.5 pe-3">
          Continue on GitHub
          <ArrowUpRight className="size-3.25" />
        </LightButton>
      </div>
    </DialogContent>
  );
}
