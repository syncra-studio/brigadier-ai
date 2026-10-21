import { StepCounter } from "@/app/conversation/StepCounter";

/** The review bar's steps: the change in view among the file's (⌃ 1 of 2 ⌄), then the file among the rest (‹ 1 of 3 Files ›). */
export function ReviewSteps({
  current,
  count,
  paths,
  path,
  onStep,
  onMove,
}: {
  current: number;
  count: number;
  paths: readonly string[];
  path: string;
  onStep: (by: 1 | -1) => void;
  onMove: (step: 1 | -1) => void;
}) {
  return (
    <>
      {count > 0 && <StepCounter data-slot="review-changes" noun="change" vertical at={current} count={count} onStep={onStep} />}
      <span className="bg-border h-3 w-px" />
      <StepCounter
        data-slot="review-files"
        noun="file"
        at={paths.indexOf(path)}
        count={paths.length}
        unit={paths.length === 1 ? "File" : "Files"}
        onStep={onMove}
      />
    </>
  );
}
