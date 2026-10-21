import { Actions, Head } from "@/app/shell/dialogs/removeProjectParts";
import { Button } from "@/components/controls";
import type { RemovalBranch, RemoveProjectReport } from "@/ipc/generated";
import { formatBytes } from "@/lib/format";

function branchKey(branch: RemovalBranch): string {
  return `${branch.repo}\n${branch.name}`;
}

/** What stayed behind: unmerged branches (with the command that deletes each) and failures. */
export function Removed({
  name,
  report,
  onOpenChange,
}: {
  name: string;
  report: RemoveProjectReport;
  onOpenChange: (open: boolean) => void;
}) {
  const kept = report.keptBranches.filter((branch) => !branch.merged);
  return (
    <>
      <div className="flex flex-col gap-5">
        <Head title={`“${name}” was removed`} beside>
          {report.brainTrashedBytes > 0
            ? `Its Brain (${formatBytes(report.brainTrashedBytes)}) is in the Trash.`
            : "Its repository was not touched."}
        </Head>
        {kept.length > 0 && (
          <section aria-labelledby="removed-kept" className="flex flex-col gap-1.5">
            <h3 id="removed-kept" className="text-foreground text-label font-medium">
              Branches kept with unmerged work
            </h3>
            <ul className="flex flex-col gap-2 text-sm">
              {kept.map((branch) => (
                <li key={branchKey(branch)} className="flex flex-col gap-0.5">
                  <span className="leading-4.25">{branch.name}</span>
                  <code className="text-muted-foreground text-xs leading-4.25 break-all select-text">{branch.command}</code>
                </li>
              ))}
            </ul>
          </section>
        )}
        {report.failures.length > 0 && (
          <section aria-labelledby="removed-failures" className="flex flex-col gap-1.5">
            <h3 id="removed-failures" className="text-foreground text-label font-medium">
              Not done
            </h3>
            <ul className="text-destructive flex flex-col gap-1 text-sm">
              {report.failures.map((failure) => (
                <li key={failure}>{failure}</li>
              ))}
            </ul>
          </section>
        )}
      </div>
      <Actions>
        <Button type="button" onClick={() => onOpenChange(false)}>
          Done
        </Button>
      </Actions>
    </>
  );
}
