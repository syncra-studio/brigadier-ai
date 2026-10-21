import { useState } from "react";

import { BrainSection } from "@/app/inspector/brain/BrainSection";
import { JOB_KINDS } from "@/app/inspector/brain/overviewText";
import { Spinner } from "@/components/icons";
import { Button } from "@/components/controls";
import { loadBrain, rebuildIndex, runBrainJob } from "@/state/brain";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

type Action = "skeleton" | "enrichment" | "rebuild";

export function DeveloperActions({
  brainKey,
  projectId,
  hasIndex,
}: {
  brainKey: string;
  projectId: string;
  hasIndex: boolean;
}) {
  const [running, setRunning] = useState<Action | null>(null);
  const [result, setResult] = useState<{ tone: "done" | "error"; text: string } | null>(null);
  const run = (action: Action) => {
    setRunning(action);
    setResult(null);
    const started =
      action === "rebuild"
        ? rebuildIndex(projectId).then(() => "Rebuilding the index.")
        : runBrainJob(projectId, action).then((jobId) => `Started ${JOB_KINDS[action].toLowerCase()} ${jobId}.`);
    started
      .then((text) => {
        setResult({ tone: "done", text });
        return loadBrain(brainKey);
      })
      .catch((cause: unknown) => setResult({ tone: "error", text: errorText(cause) }))
      .finally(() => setRunning(null));
  };
  return (
    <BrainSection title="Developer">
      <div className="flex flex-wrap gap-2">
        <Button size="xs" variant="outline" disabled={running !== null} onClick={() => run("skeleton")}>
          {running === "skeleton" && <Spinner className="animate-spin" />}
          Run skeleton pass
        </Button>
        <Button size="xs" variant="outline" disabled={running !== null} onClick={() => run("enrichment")}>
          {running === "enrichment" && <Spinner className="animate-spin" />}
          Enrich now
        </Button>
        <Button
          size="xs"
          variant="outline"
          disabled={running !== null || !hasIndex}
          onClick={() => run("rebuild")}
        >
          {running === "rebuild" && <Spinner className="animate-spin" />}
          Rebuild index
        </Button>
      </div>
      {result && (
        <p role={result.tone === "error" ? "alert" : "status"} className={result.tone === "error" ? "text-destructive" : "text-muted-foreground"}>
          {result.text}
        </p>
      )}
    </BrainSection>
  );
}
