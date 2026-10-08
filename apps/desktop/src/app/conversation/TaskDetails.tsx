import { Branch, BranchAlt } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { MarkdownBlock } from "@/components/assistant-ui/thread";
import { ArtifactDialog } from "@/app/conversation/ArtifactDialog";
import { DiffStatView, Section, short } from "@/app/conversation/cards/common";
import { FileList } from "@/app/conversation/FileList";
import { useAction } from "@/app/conversation/useAction";
import { RouteSections } from "@/app/conversation/cards/RouteDetails";
import { useWorkerText, WorkerChip } from "@/app/conversation/WorkerChip";
import { mono } from "@/components/assistant-ui/elements/surfaces";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { ArtifactRef, KeptWork, RestoreOutcome, Task } from "@/ipc/generated";
import { useModelGroups } from "@/lib/setup";
import { cn } from "@/lib/utils";
import { restoreKeptWork } from "@/state/actions";
import { useBoard } from "@/state/board";

function ArtifactButtons({
  artifacts,
  onOpen,
}: {
  artifacts: readonly ArtifactRef[];
  onOpen: (artifact: ArtifactRef) => void;
}) {
  return (
    <div className="flex flex-wrap gap-1.5">
      {artifacts.map((artifact) => (
        <Button key={artifact.id} size="xs" variant="outline" onClick={() => onOpen(artifact)}>
          {artifact.title}
          <span className="text-muted-foreground">{artifact.kind}</span>
        </Button>
      ))}
    </div>
  );
}

function ReportLine({ text }: { text: string }) { return useWorkerText(text); }
function ReportLines({ items }: { items: readonly string[] }) {
  if (!items.length) return <p className="text-muted-foreground text-xs">None.</p>;
  return <ul className="flex list-disc flex-col gap-0.5 ps-4 text-sm">
    {items.map((text, index) => <li key={index}><ReportLine text={text} /></li>)}
  </ul>;
}

/** Unfinished work saved as a patch (its branch is gone), with "Restore as branch". */
function KeptPatch({
  taskId,
  kept,
  target,
  onOpen,
}: {
  taskId: string;
  kept: Extract<KeptWork, { type: "diff" }>;
  target: string | null;
  onOpen: (artifact: ArtifactRef) => void;
}) {
  const action = useAction();
  const [outcome, setOutcome] = useState<RestoreOutcome | null>(null);
  const restore = () =>
    action.run(async () => setOutcome(await restoreKeptWork(taskId)));
  return (
    <div className="flex flex-col gap-1.5">
      <p className="text-xs">
        Saved as a patch, not a branch: it could not be kept as a clean commit on{" "}
        {target ? <span className="font-mono">{target}</span> : "the target branch"} (for
        example, it overlaps uncommitted changes you let workers see).
      </p>
      <div className="flex flex-wrap items-center gap-1.5">
        <ArtifactButtons artifacts={[kept.artifact]} onOpen={onOpen} />
        {kept.restored === null && target && (
          <Button size="xs" variant="outline" disabled={action.busy} onClick={restore}>
            Restore as branch
          </Button>
        )}
      </div>
      {kept.restored !== null && (
        <p className={mono}>Restored as {kept.restored}</p>
      )}
      {outcome?.type === "conflicts" && (
        <p role="alert" className="text-destructive text-xs">
          It can't be restored on {target}: it conflicts with {target} in{" "}
          {outcome.paths.join(", ")}. Nothing was created.
        </p>
      )}
      {outcome?.type === "failed" && (
        <p role="alert" className="text-destructive text-xs">
          It can't be restored: {outcome.reason} Nothing was created.
        </p>
      )}
      {action.error && (
        <p role="alert" className="text-destructive text-xs">
          {action.error}
        </p>
      )}
    </div>
  );
}

/**
 * What a worker set out to do and produced: model, workspace, report, outputs, commit.
 * Under its thread in the workers panel (`inThread`), the thread already shows the model, the
 * report's summary and the transcript.
 */
export function TaskDetails({
  task,
  model,
  inThread = false,
  detailsPage = false,
}: {
  task: Task;
  model: string;
  inThread?: boolean;
  detailsPage?: boolean;
}) {
  const [artifact, setArtifact] = useState<ArtifactRef | null>(null);
  const groups = useModelGroups();
  // The worker this one reviews or works on, while the board has it.
  const subject = useBoard((s) =>
    task.subject && s.board?.tasks[task.subject] ? task.subject : null,
  );
  const { report, candidate, workspace, kept } = task;
  const reportSummary = useWorkerText(report?.summary ?? "");

  return (
    <>
      <RouteSections task={task} model={model} groups={groups} inThread={detailsPage ? false : inThread} />

      {(workspace || subject !== null) && (
        <Section title="Workspace">
          {subject !== null && (
            <p className="flex min-w-0 items-center gap-1.5 text-xs">
              <span className="shrink-0">{task.kind === "review" ? "Reviews" : "Works on"}</span>
              <WorkerChip taskId={subject} />
            </p>
          )}
          {workspace?.branch && (
            <p className={cn(mono, "flex items-center gap-1.5")}>
              <Branch aria-label="Branch" className="size-icon-xs shrink-0" />
              <span className="min-w-0">
                {workspace.branch}
                {workspace.target && ` → ${workspace.target}`}
                {workspace.base && ` (from ${short(workspace.base)})`}
              </span>
            </p>
          )}
          {workspace?.worktree && (
            <p className={cn(mono, "text-muted-foreground flex items-center gap-1.5")}>
              <BranchAlt aria-label="Worktree" className="size-icon-xs shrink-0" />
              <span className="min-w-0 truncate">{workspace.worktree}</span>
            </p>
          )}
        </Section>
      )}

      {report ? (
        <>
          {(!inThread || detailsPage) && (
            <Section title="Report">
              <MarkdownBlock text={reportSummary} />
            </Section>
          )}
          <Section title="Changes">
            <ReportLines items={report.changes} />
          </Section>
          <Section title="Decisions">
            <ReportLines items={report.decisions} />
          </Section>
          <Section title="Verification">
            <ReportLines items={report.verification} />
          </Section>
          {report.doneWhen.length > 0 && (
            <Section title="Done when">
              <ReportLines items={report.doneWhen} />
            </Section>
          )}
          {report.openQuestions.length > 0 && (
            <Section title="Open questions">
              <ReportLines items={report.openQuestions} />
            </Section>
          )}
          {report.risks.length > 0 && (
            <Section title="Risks">
              <ReportLines items={report.risks} />
            </Section>
          )}
          {report.needsUser.length > 0 && (
            <Section title="Needs you">
              <ReportLines items={report.needsUser} />
            </Section>
          )}
          {report.verdict && (
            <Section title="Verdict">
              <Badge variant={report.verdict === "approve" ? "success" : "warning"}>
                {report.verdict === "approve" ? "Approved" : "Changes requested"}
              </Badge>
            </Section>
          )}
          {report.artifacts.length > 0 && (
            <Section title="Artifacts">
              <FileList artifacts={report.artifacts} onView={setArtifact} />
            </Section>
          )}
        </>
      ) : (
        !inThread && <p className="text-muted-foreground text-xs">No report yet.</p>
      )}

      {task.outputs.length > 0 && (
        <Section title="Outputs">
          <FileList artifacts={task.outputs} onView={setArtifact} />
        </Section>
      )}

      {candidate && (
        <Section title="Candidate commit">
          <p className="text-sm">
            <span className="font-mono">{short(candidate.commit)}</span> {candidate.message}
          </p>
          <p className={cn(mono, "text-muted-foreground")}>on {short(candidate.onto)}</p>
          <DiffStatView stat={candidate.diffStat} />
          {candidate.excluded.length > 0 && (
            <div className="flex flex-col gap-0.5">
              <p className="text-xs">Left out by the litter guard:</p>
              <ul className={cn(mono, "flex flex-col gap-0.5")}>
                {candidate.excluded.map((file) => (
                  <li key={file.path}>
                    {file.path} <span className="text-muted-foreground">— {file.reason}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}
          {candidate.diff && (
            <ArtifactButtons artifacts={[candidate.diff]} onOpen={setArtifact} />
          )}
        </Section>
      )}

      {task.landed && (
        <Section title="Landed">
          <p className={mono}>
            {task.landed}
            {workspace?.target && ` on ${workspace.target}`}
          </p>
        </Section>
      )}

      {kept && (
        <Section title="Kept work">
          {kept.type === "branch" ? (
            <p className={mono}>
              {kept.branch} at {short(kept.commit)}
            </p>
          ) : (
            <KeptPatch
              taskId={task.id}
              kept={kept}
              target={workspace?.target ?? null}
              onOpen={setArtifact}
            />
          )}
        </Section>
      )}

      <ArtifactDialog artifact={artifact} onOpenChange={(next) => !next && setArtifact(null)} />
    </>
  );
}
