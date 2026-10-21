import { ErrorLine } from "@/app/shell/dialogs/fields";
import { FeedbackResults, sentLine, sentTitle, stepProgress, unsentLine, useSystemWord } from "@/app/shell/feedback/FeedbackResults";
import { Button } from "@/components/controls";
import { ArrowUpRight, X } from "@/components/icons";
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogTitle } from "@/components/overlays";
import { ProgressLine } from "@/components/shared";
import { openUrl } from "@/ipc/client";
import { NEW_ISSUE_CHOOSER } from "@/lib/feedbackUrl";
import { cn } from "@/lib/utils";
import {
  closeFeedback,
  type FeedbackAccount,
  sendFromDialog,
  setFeedbackText,
  useFeedbackUi,
} from "@/state/feedback";

const ASK = "What’s wrong, or what would you like? Several things at once is fine.";

/**
 * Send feedback (Help › Send Feedback…, the sidebar's help menu, the search palette, or the
 * thread's card's Edit), with the app's dialog parts: the user only writes and sends. Brigadier
 * writes it up as one report per bug or idea and files each on GitHub as them (state/feedback.ts),
 * then lists what it filed, each with its link. "Write it on GitHub yourself" opens the blank
 * new-issue page instead. The X closes it; a send at work carries on and shows when it reopens.
 */
export function FeedbackDialog() {
  const open = useFeedbackUi((s) => s.open);
  return (
    <Dialog open={open} onOpenChange={(next) => !next && closeFeedback()}>
      {open && <Feedback />}
    </Dialog>
  );
}

function Feedback() {
  const text = useFeedbackUi((s) => s.text);
  const sending = useFeedbackUi((s) => s.sending);
  const account = useFeedbackUi((s) => s.account);
  const system = useSystemWord();
  const done = sending.phase === "done" ? sending.results : null;
  const unsent = done?.filter((result) => result.outcome === "failed").length ?? 0;
  const working = sending.phase === "working";
  const error = sending.phase === "failed" ? `${sending.error} Your note is still here.` : null;

  return (
    <DialogContent data-slot="feedback" showCloseButton={false} className="max-w-feedback block gap-0 p-0">
      <div className="flex gap-3.5 pt-6 pe-5 pb-4.5 ps-6">
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <DialogTitle>{!done ? "Send feedback" : unsent ? "Not all of it was sent" : sentTitle(done, "Thanks, it’s sent")}</DialogTitle>
          <DialogDescription>
            {!done
              ? "Say what’s wrong or what you’d like. Brigadier writes it up for its developers."
              : unsent
                ? unsentLine(unsent)
                : sentLine(done)}
          </DialogDescription>
        </div>
        <DialogClose asChild>
          <Button variant="ghost" size="icon-sm" aria-label="Close">
            <X />
          </Button>
        </DialogClose>
      </div>

      <div className="flex flex-col gap-3 px-6 pb-5.5">
        {done ? (
          <FeedbackResults results={done} />
        ) : (
          <textarea
            value={text}
            onChange={(event) => setFeedbackText(event.target.value)}
            readOnly={working}
            spellCheck
            autoFocus
            placeholder={ASK}
            aria-label={ASK}
            className={cn(
              "placeholder:text-muted-foreground bg-muted/60 focus-visible:bg-background rounded-control h-37 resize-none border border-transparent px-3.5 py-3 text-sm leading-normal transition-colors outline-none",
              working && "text-foreground/80",
            )}
          />
        )}
        {sending.phase === "working" && (
          <ProgressLine
            icon={false}
            title="Writing up your reports…"
            {...stepProgress(sending.step, account)}
            className="text-muted-foreground pt-1"
          />
        )}
        {(sending.phase === "write" || sending.phase === "failed") && (
          <p className="text-muted-foreground min-h-4 text-xs">{account && accountLine(account, system)}</p>
        )}
        <ErrorLine error={error} />
      </div>

      <div className="border-divider flex items-center gap-2 border-t py-4 pe-6 ps-4">
        {!done && !working && (
          <Button variant="ghost" size="sm" className="text-muted-foreground px-2" onClick={() => void openUrl(NEW_ISSUE_CHOOSER)}>
            Write it on GitHub yourself
            <ArrowUpRight className="size-icon-sm" />
          </Button>
        )}
        <span className="flex-1" />
        {done && !unsent ? (
          <DialogClose asChild>
            <Button size="sm" autoFocus>
              Done
            </Button>
          </DialogClose>
        ) : (
          <Button size="sm" disabled={working || !text.trim()} onClick={sendFromDialog}>
            {working ? "Sending…" : sending.phase === "failed" || unsent ? "Try again" : "Send"}
          </Button>
        )}
      </div>
    </DialogContent>
  );
}

/** Who it posts as, and what goes with it; or that each report opens in the browser. */
function accountLine({ signedIn, login }: FeedbackAccount, system: string): string {
  if (!signedIn) return "You’re not signed in to GitHub, so each report opens in your browser, filled in.";
  return `Posts as ${login ? `@${login}` : "you"} on GitHub, with your app and ${system} version.`;
}
