import { ChevronRight, ShieldCheck } from "@openai/apps-sdk-ui/components/Icon";
import { type ComponentProps, useState } from "react";

import { CodeSpans } from "@/app/conversation/cards/codeSpans";
import {
  type FindingLabel,
  type FindingRow,
  type ReviewRow,
  type ReviewSection,
  useReviewRow,
} from "@/app/conversation/reviewStatus";
import { SummaryRow, SummaryRowButton } from "@/components/assistant-ui/elements/summary-section";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

/**
 * The code reviews' outcome, quietly, in one row of the context card: of the work since the last
 * merge, and of the work that merge took, which stays (a review may still run when the user
 * merges) until the next. The counts are the thread's: what it recorded of each finding
 * (SESSION-FIXES-PLAN.md Q7).
 */
export function ReviewRows({ conversationId }: { conversationId: string }) {
  const row = useReviewRow(conversationId);
  return row && <ReviewSummaryRow row={row} />;
}

/**
 * The review row; a click lists its findings and what became of each, by batch. The list holds
 * about six rows, then scrolls, so the floating card never fills the window.
 */
export function ReviewSummaryRow({ row, defaultOpen = false }: { row: ReviewRow; defaultOpen?: boolean }) {
  const [open, setOpen] = useState(defaultOpen);
  if (row.sections.every((section) => section.findings.length === 0)) {
    return (
      <SummaryRow data-slot="review-line" icon={<ShieldCheck />} muted>
        {row.text}
      </SummaryRow>
    );
  }
  return (
    <>
      <SummaryRowButton
        data-slot="review-line"
        aria-expanded={open}
        title={row.text}
        icon={<ShieldCheck />}
        muted
        onClick={() => setOpen(!open)}
        meta={<ChevronRight aria-hidden className={cn("size-chevron transition-[rotate] duration-200 motion-reduce:transition-none", open && "rotate-90")} />}
      >
        {row.text}
      </SummaryRowButton>
      {open && (
        <div
          data-slot="review-findings"
          className="flex max-h-[calc(var(--spacing-row)*6)] min-w-0 flex-col gap-1 overflow-y-auto pb-2"
        >
          {row.sections.map((section, index) => (
            <FindingSection key={section.label ?? index} section={section} />
          ))}
        </div>
      )}
    </>
  );
}

/** One batch's findings; labelled ("Merged work · 1 open") when the row speaks for two. */
function FindingSection({ section }: { section: ReviewSection }) {
  return (
    <section data-slot="review-section" className="flex min-w-0 flex-col">
      {section.label && (
        <h4 className="text-muted-foreground truncate pt-1 text-xs">
          {`${section.label} · ${section.outcome}`}
        </h4>
      )}
      <ul className="flex min-w-0 flex-col">
        {section.findings.map((finding) => (
          <FindingItem key={finding.id} finding={finding} />
        ))}
      </ul>
    </section>
  );
}

/** Each chip coloured as the app's badges are: open warns, fixed is done, dismissed is quiet. */
const STATUS_BADGE: Record<FindingLabel, ComponentProps<typeof Badge>> = {
  Open: { variant: "warning" },
  New: { variant: "secondary" },
  Fixed: { variant: "success" },
  Dismissed: { variant: "outline", className: "border-divider text-foreground/65" },
};

/**
 * One finding, one compact row: what the thread recorded and how severe, the title (its code
 * spans as code) in two lines at most, and `file:line` at the end. Its id and the thread's words
 * on it (why it stays open or was dismissed) show on hover.
 */
function FindingItem({ finding }: { finding: FindingRow }) {
  return (
    <li
      data-slot="review-finding"
      title={[finding.id, finding.note].filter(Boolean).join(" · ")}
      className="flex min-w-0 items-start gap-1.5 py-1 text-label"
    >
      <Badge data-slot="finding-status" {...STATUS_BADGE[finding.status]}>
        {finding.status}
      </Badge>
      {finding.severity && (
        <Badge data-slot="finding-severity" variant="outline" className="border-divider text-foreground/65">
          {finding.severity}
        </Badge>
      )}
      <span className="text-foreground/90 line-clamp-2 min-w-0 flex-1 wrap-break-word">
        <CodeSpans text={finding.title} />
      </span>
      {finding.location && (
        <span title={finding.location} className="text-muted-foreground max-w-1/3 shrink-0 truncate font-mono text-xs leading-(--text-label--line-height)">
          {finding.location}
        </span>
      )}
    </li>
  );
}
