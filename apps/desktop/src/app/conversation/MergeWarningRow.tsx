import { QUIET } from "@/app/conversation/changesPills";
import { Button } from "@/components/controls";
import { TriangleExclamationErrorWarning } from "@/components/icons";

/**
 * The Changes card's footer row for something really wrong with the merged code: the warning, the
 * first open finding under it, and a button to the findings.
 */
export function MergeWarningRow({
  problem,
  finding,
  openFindings,
  onOpen,
}: {
  problem: string;
  finding: { id: string; title: string } | null;
  openFindings: number;
  onOpen: () => void;
}) {
  return (
    <div data-slot="merge-warning" className="border-divider flex items-center gap-3 border-t pt-3 pb-3.5 ps-13">
      <TriangleExclamationErrorWarning aria-hidden className="text-warning size-3.5 shrink-0" />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span data-slot="merge-warning-title" className="text-warning text-label font-medium">{problem.replace(/\.$/, "")}</span>
        {finding && (
          <span data-slot="merge-warning-finding" className="text-muted-foreground truncate text-xs">
            {finding.id} · {finding.title}
          </span>
        )}
      </div>
      <Button variant="ghost" size="sm" className={QUIET} data-slot="merge-warning-open" onClick={onOpen}>
        {openFindings > 0 ? "Open finding" : "Open Progress"}
      </Button>
    </div>
  );
}
