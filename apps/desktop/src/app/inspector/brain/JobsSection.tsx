import { BrainSection } from "@/app/inspector/brain/BrainSection";
import { JobRow } from "@/app/inspector/brain/JobRow";
import type { BrainJob } from "@/ipc/generated";

export function JobsSection({ jobs, now }: { jobs: readonly BrainJob[]; now: number }) {
  return (
    <BrainSection title="Brain jobs">
      {jobs.length === 0 ? (
        <p className="text-muted-foreground">No skeleton pass or enrichment has run yet.</p>
      ) : (
        <ul className="divide-border flex flex-col divide-y">
          {jobs.map((job) => (
            <JobRow key={job.id} job={job} now={now} />
          ))}
        </ul>
      )}
    </BrainSection>
  );
}
