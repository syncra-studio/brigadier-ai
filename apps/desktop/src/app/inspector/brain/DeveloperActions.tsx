import { useState } from "react";

import { BrainSection } from "@/app/inspector/brain/BrainSection";
import { Spinner } from "@/components/icons";
import { Button } from "@/components/controls";
import { loadBrain, rebuildIndex } from "@/state/brain";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function DeveloperActions({
  brainKey,
  projectId,
  hasIndex,
}: {
  brainKey: string;
  projectId: string;
  hasIndex: boolean;
}) {
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<{ tone: "done" | "error"; text: string } | null>(null);
  const run = () => {
    setRunning(true);
    setResult(null);
    rebuildIndex(projectId)
      .then(() => {
        setResult({ tone: "done", text: "Rebuilding the index." });
        return loadBrain(brainKey);
      })
      .catch((cause: unknown) => setResult({ tone: "error", text: errorText(cause) }))
      .finally(() => setRunning(false));
  };
  return (
    <BrainSection title="Developer">
      <div className="flex flex-wrap gap-2">
        <Button size="xs" variant="outline" disabled={running || !hasIndex} onClick={run}>
          {running && <Spinner className="animate-spin" />}
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
