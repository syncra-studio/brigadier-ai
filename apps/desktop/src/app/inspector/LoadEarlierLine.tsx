import { Button } from "@/components/controls";

/** The orchestrator log's line under the meters when older entries are still unloaded. */
export function LoadEarlierLine({ loading, onLoad }: { loading: boolean; onLoad: () => void }) {
  return (
    <p className="text-muted-foreground">
      Totals count from the oldest loaded entry.{" "}
      <Button size="xs" variant="ghost" disabled={loading} onClick={onLoad}>
        Load earlier entries
      </Button>
    </p>
  );
}
