import { Picker, type PickerOption } from "@/app/inspector/providers/Picker";
import { ToggleGroup, ToggleGroupItem } from "@/components/controls";

export type BrainView = "overview" | "search" | "graph";

/** The Brain tab's top bar: which Brain, and which view of it. */
export function BrainToolbar({
  brainKey,
  options,
  onBrainChange,
  view,
  onViewChange,
}: {
  brainKey: string;
  options: readonly PickerOption[];
  onBrainChange: (key: string) => void;
  view: BrainView;
  onViewChange: (view: BrainView) => void;
}) {
  return (
  <div className="flex shrink-0 items-center gap-2 border-b px-3 py-2 text-xs">
    <Picker label="Brain" value={brainKey} options={options} onChange={onBrainChange} />
    <span className="flex-1" />
    <ToggleGroup
      type="single"
      size="sm"
      variant="outline"
      value={view}
      onValueChange={(value) => {
        if (value === "overview" || value === "search" || value === "graph") onViewChange(value);
      }}
      aria-label="Brain view"
    >
      <ToggleGroupItem value="overview" className="text-xs">
        Overview
      </ToggleGroupItem>
      <ToggleGroupItem value="search" className="text-xs">
        Search
      </ToggleGroupItem>
      <ToggleGroupItem value="graph" className="text-xs">
        Graph
      </ToggleGroupItem>
    </ToggleGroup>
  </div>
  );
}
