import { createContext, useContext, useEffect, useState } from "react";

import { OfferCard, OfferError } from "@/app/conversation/OfferCard";
import { useAction } from "@/app/conversation/useAction";
import { FeedbackResults, KindIcon, sentLine, sentToBrowser, stepProgress, useSystemWord } from "@/app/shell/feedback/FeedbackResults";
import { OPENS } from "@/components/conversation";
import { Button } from "@/components/controls";
import { Check, ChevronRightAlt, FeedbackGlyph } from "@/components/icons";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/layout";
import { ProgressLine } from "@/components/shared";
import type { FeedbackReport } from "@/ipc/generated";
import { type FeedbackKind, reportLines } from "@/lib/feedbackUrl";
import { answerFeedbackOffer } from "@/state/actions";
import { useBoard } from "@/state/board";
import {
  type FeedbackAccount,
  type FeedbackSending,
  draftOffer,
  offerSent,
  openFeedback,
  sendOffer,
  useFeedbackAccount,
  useFeedbackOffers,
} from "@/state/feedback";

/** The card's buttons other than Send: ghost, their words grey. */
const QUIET = "text-muted-foreground";

/**
 * The thread's offer of reports to Brigadier's developers (its `offer_feedback`, when the user
 * seemed annoyed with Brigadier or asked to report something), written up from the
 * conversation as Send feedback writes a note up: one report per bug or idea, each opening to
 * what will be posted, a repeat marked "Already reported". Send files them (as the user, else
 * in the browser), Edit opens Send feedback with the note filled in, Dismiss puts it away.
 * Nothing leaves the app before a click; once sent it lists what was filed.
 */
export function FeedbackCard({
  reports,
  account,
  sending,
  sent,
  error,
  onSend,
  onEdit,
  onDismiss,
}: {
  /** As written up; `null` while it is being written. */
  reports: readonly FeedbackReport[] | null;
  account: FeedbackAccount | null;
  sending: FeedbackSending;
  /** Answered as sent (in this card, from the dialog, or before the app last started). */
  sent: boolean;
  error?: string | null;
  onSend: () => void;
  onEdit: () => void;
  onDismiss: () => void;
}) {
  if (sent || sending.phase === "done") {
    const results = sending.phase === "done" ? sending.results : null;
    return (
      <OfferCard
        label="Sent to Brigadier’s developers"
        kind="feedbackOffered"
        icon={<Check className="text-success" />}
        title="Sent. Thank you"
        line={results && sentToBrowser(results) ? sentLine(results) : "You can follow them on GitHub."}
      >
        {results && <FeedbackResults results={results} />}
      </OfferCard>
    );
  }
  const count = reports?.length ?? 0;
  const line = [count > 1 ? `${count} reports written from this chat` : "Written from this chat", account && postsAs(account)]
    .filter(Boolean)
    .join(" · ");
  if (sending.phase === "working" || !reports) {
    const step = sending.phase === "working" ? sending.step : { step: "writing" as const };
    const { description, done, total } = stepProgress(step, account);
    return (
      <OfferCard
        label="Sending to Brigadier’s developers"
        kind="feedbackOffered"
        icon={<FeedbackGlyph />}
        title={step.step === "writing" ? "Writing up your reports…" : "Sending your reports…"}
        line={line}
      >
        <ProgressLine icon={false} title={description} done={done} total={total} className="text-muted-foreground" />
      </OfferCard>
    );
  }
  const failed = sending.phase === "failed";
  return (
    <OfferCard
      label="Send these to Brigadier’s developers?"
      kind="feedbackOffered"
      icon={<FeedbackGlyph />}
      title={count > 1 ? "Send these to Brigadier’s developers?" : "Send this to Brigadier’s developers?"}
      line={line}
      actions={
        <>
          <Button size="sm" onClick={onSend}>
            {failed ? "Try again" : "Send"}
          </Button>
          <Button variant="ghost" size="sm" className={QUIET} onClick={onEdit}>
            Edit
          </Button>
          <Button variant="ghost" size="sm" className={QUIET} onClick={onDismiss}>
            Dismiss
          </Button>
        </>
      }
    >
      <ul aria-label="Reports" className="bg-foreground/5 rounded-settings flex flex-col gap-1 p-1.5">
        {reports.map((report, index) => (
          <ReportRow key={`${index}-${report.title}`} report={report} />
        ))}
      </ul>
      {(failed || error) && <OfferError>{failed ? `${sending.error} Try again in a moment.` : error}</OfferError>}
    </OfferCard>
  );
}

/** Who the reports go out as, after the card's line. */
function postsAs({ signedIn, login }: FeedbackAccount): string {
  if (!signedIn) return "opens in your browser";
  return login ? `posts as @${login}` : "posts as you on GitHub";
}

/** One report as written: its kind, title and what it is; opened, the words that will be posted. */
function ReportRow({ report }: { report: FeedbackReport }) {
  const repeat = report.duplicate;
  const system = useSystemWord();
  return (
    <Collapsible asChild>
      <li className="flex flex-col">
        <CollapsibleTrigger className="group hover:bg-foreground/5 rounded-control flex min-w-0 items-center gap-2 px-1.5 py-1 text-start">
          <KindIcon kind={report.kind} />
          <span className="text-label min-w-0 flex-1 truncate">{report.title}</span>
          <span className="text-muted-foreground shrink-0 text-xs">
            {repeat ? "Already reported" : report.kind === "bug" ? "Bug" : "Idea"}
          </span>
          <ChevronRightAlt
            aria-hidden
            className="text-muted-foreground size-chevron shrink-0 transition-[rotate] group-data-[state=open]:rotate-90 motion-reduce:transition-none"
          />
        </CollapsibleTrigger>
        <CollapsibleContent className={OPENS}>
          <div className="text-muted-foreground flex flex-col gap-1 ps-7.5 pe-1.5 pt-1 pb-1 text-xs leading-normal">
            {repeat ? (
              <p className="whitespace-pre-line">{repeat.comment}</p>
            ) : (
              reportLines(report).map((each) => <p key={each}>{each}</p>)
            )}
            {(repeat || report.kind === "bug") && (
              <p className="text-foreground/70">
                {repeat ? `Goes on #${repeat.number} as a comment` : `Adds your app and ${system} version`}
              </p>
            )}
          </div>
        </CollapsibleContent>
      </li>
    </Collapsible>
  );
}

/**
 * Whether nothing of the request a step renders under runs any more (done, stopped or failed):
 * the offer waits for it, so it never comes mid-turn.
 */
export const TurnEnded = createContext(false);

/** The offer's own text as its one report, for when no writer could write it up. */
function asReport(kind: FeedbackKind, title: string, body: string): FeedbackReport {
  return {
    kind,
    title,
    area: "",
    steps: "",
    expected: "",
    actual: kind === "bug" ? body : "",
    problem: kind === "idea" ? body : "",
    today: "",
    wanted: "",
    duplicate: null,
  };
}

/**
 * The offer `id` as the thread made it, answered through the daemon: written up once the turn
 * has ended, Send files the reports, Edit opens Send feedback filled in (sending there counts),
 * Dismiss puts it away for good.
 */
export function FeedbackOffer({ id, kind, title, body }: { id: string; kind: FeedbackKind; title: string; body: string }) {
  const ended = useContext(TurnEnded);
  const conversationId = useBoard((s) => s.board?.conversationId ?? null);
  const answered = useBoard((s) => {
    const step = s.board?.orchestratorSteps.find((each) => each.kind.type === "feedbackAnswered" && each.kind.id === id);
    return step?.kind.type === "feedbackAnswered" ? step.kind.sent : null;
  });
  const offer = useFeedbackOffers((s) => s[id]);
  const account = useFeedbackAccount();
  const [answer, setAnswer] = useState<boolean | null>(null);
  const action = useAction();
  const said = answered ?? answer;
  const text = `${title}\n\n${body}`;
  const open = ended && conversationId !== null && said === null;
  useEffect(() => {
    if (open) draftOffer(id, text);
  }, [open, id, text]);
  if (!ended || !conversationId || said === false) return null;
  const answerWith = (value: boolean) => answerFeedbackOffer(conversationId, id, value).then(() => setAnswer(value));
  const written = offer?.written;
  return (
    <FeedbackCard
      reports={written === undefined ? null : written?.reports.length ? written.reports : [asReport(kind, title, body)]}
      account={account}
      sending={offer?.sending ?? { phase: "write" }}
      sent={said === true}
      error={action.error}
      onSend={() => sendOffer(id, text, kind, () => void action.run(() => answerWith(true)))}
      onEdit={() =>
        openFeedback({
          kind,
          text,
          onSent: (results) => {
            offerSent(id, results);
            void action.run(() => answerWith(true));
          },
        })
      }
      onDismiss={() => void action.run(() => answerWith(false))}
    />
  );
}
