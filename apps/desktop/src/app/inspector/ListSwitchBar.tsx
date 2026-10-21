import { ToggleGroup, ToggleGroupItem } from "@/components/controls";

export type OrchestratorList = "injections" | "rebirths";

/** The orchestrator log's switch between its context injections and its rebirths. */
export function ListSwitchBar({
  list,
  onListChange,
  injections,
  rebirths,
}: {
  list: OrchestratorList;
  onListChange: (list: OrchestratorList) => void;
  injections: number;
  rebirths: number;
}) {
  return (
    <div className="text-muted-foreground flex shrink-0 items-center gap-2 border-b px-3 py-1.5 text-xs">
      <ToggleGroup
        type="single"
        size="sm"
        variant="outline"
        value={list}
        onValueChange={(value) => {
          if (value === "injections" || value === "rebirths") onListChange(value);
        }}
        aria-label="Orchestrator log"
      >
        <ToggleGroupItem value="injections" className="text-xs">
          Context injections · {injections}
        </ToggleGroupItem>
        <ToggleGroupItem value="rebirths" className="text-xs">
          Rebirths · {rebirths}
        </ToggleGroupItem>
      </ToggleGroup>
      {list === "injections" && <span className="flex-1 text-end">newest first, by CLI generation</span>}
    </div>
  );
}
