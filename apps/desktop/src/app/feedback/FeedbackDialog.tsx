import { type ReactNode, useEffect, useState } from "react";

import { ErrorLine, errorText } from "@/app/dialogs/fields";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { ArrowUpRight, Bug, Eye, Lightbulb, X } from "@/components/icons";
import { Segmented } from "@/components/ui/segmented";
import { detailsItems, type FeedbackKind } from "@/lib/feedbackUrl";
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
 * from elsewhere), laid out to the user's design with the app's dialog parts: a bug or an idea, written here and opened as a
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
      className="max-w-feedback block gap-0 p-0"
    >
      <div className="flex gap-3.5 pt-6 pe-5 pb-4.5 ps-6">
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <DialogTitle>Send feedback</DialogTitle>
          <DialogDescription>
            Opens a GitHub issue with your note filled in.
          </DialogDescription>
        </div>
        <DialogClose asChild>
          <Button variant="ghost" size="icon-sm" aria-label="Close">
            <X />
          </Button>
        </DialogClose>
      </div>

      <div className="flex flex-col gap-3.5 px-6 pb-5.5">
        <Segmented
          fill
          label="Kind"
          value={kind}
          options={KINDS.map((choice) => ({
            value: choice.value,
            label: (
              <span className="[&_svg]:size-icon-sm inline-flex items-center gap-1.5 align-middle">
                {choice.icon}
                {choice.label}
              </span>
            ),
          }))}
          onChange={setKind}
        />

        <textarea
          value={text}
          onChange={(event) => setText(event.target.value)}
          spellCheck
          autoFocus
          placeholder={ASK[kind]}
          aria-label={ASK[kind]}
          className="placeholder:text-muted-foreground bg-muted/60 focus-visible:bg-background rounded-control h-37 resize-none border border-transparent px-3.5 py-3 text-sm leading-normal transition-colors outline-none"
        />

        {included.length > 0 && (
          <div className="flex flex-col gap-2 pt-1">
            <span className="text-muted-foreground text-xs">Also included</span>
            <ul aria-label="Also included" className="flex flex-wrap gap-1.5">
              {included.map((item) => (
                <Badge asChild key={item} variant="secondary" className="font-mono font-normal">
                  <li>{item}</li>
                </Badge>
              ))}
            </ul>
          </div>
        )}

        <ErrorLine error={error} />
      </div>

      <div className="border-divider flex items-center gap-2 border-t px-6 py-4">
        <span className="text-muted-foreground flex min-w-0 flex-1 items-center gap-1.75 text-xs">
          <Eye className="size-3.25 shrink-0" />
          You’ll review it before it’s posted
        </span>
        <DialogClose asChild>
          <Button variant="ghost" size="sm">
            Cancel
          </Button>
        </DialogClose>
        <Button size="sm" disabled={!text.trim()} onClick={send}>
          Continue on GitHub
          <ArrowUpRight />
        </Button>
      </div>
    </DialogContent>
  );
}
