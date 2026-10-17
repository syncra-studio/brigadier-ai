import { useEffect, useState } from "react";

import { ErrorLine, errorText } from "@/app/dialogs/fields";
import { FeedbackDialog as FeedbackForm } from "@/components/assistant-ui/elements/feedback-dialog";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { detailsLine, type FeedbackKind } from "@/lib/feedbackUrl";
import { closeFeedback, feedbackDetails, type FeedbackDraft, sendFeedback, useFeedbackUi } from "@/state/feedback";

const KINDS = [
  { value: "bug", label: "Something’s wrong" },
  { value: "idea", label: "An idea" },
] as const satisfies readonly { value: FeedbackKind; label: string }[];

const ASK: Record<FeedbackKind, string> = {
  bug: "What happened, and what did you expect?",
  idea: "What are you trying to do, and what gets in the way?",
};

/**
 * Send feedback (the sidebar's foot, Help › Send Feedback…, or a draft from elsewhere): a bug
 * or an idea, written here and opened as a new issue on GitHub, filled in, to finish and post
 * there.
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
  const [details, setDetails] = useState<string | undefined>();
  const [sent, setSent] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    void feedbackDetails().then((found) => live && setDetails(detailsLine(found) || undefined));
    return () => {
      live = false;
    };
  }, []);

  const send = () => {
    setError(null);
    sendFeedback(kind, text).then(
      () => setSent(true),
      (cause: unknown) => setError(errorText(cause)),
    );
  };

  return (
    <DialogContent data-slot="feedback" className="rounded-question max-w-md bg-transparent p-0 ring-0">
      <DialogDescription className="sr-only">
        It opens as a new issue on GitHub, filled in. You can edit it there before you post.
      </DialogDescription>
      <FeedbackForm
        className="p-5 pe-12"
        heading={<DialogTitle className="text-base font-medium">Send feedback</DialogTitle>}
        kinds={KINDS}
        kind={kind}
        note={text}
        placeholder={ASK[kind]}
        details={details}
        sent={sent}
        sentText="Opened on GitHub: finish it there and post it."
        submitLabel="Open on GitHub"
        error={<ErrorLine error={error} />}
        onKindChange={setKind}
        onNoteChange={setText}
        onSubmit={send}
      />
    </DialogContent>
  );
}
