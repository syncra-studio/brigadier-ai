import { SettingsSelect } from "@/components/controls";
import { Filter } from "@/components/icons";
import type { HistoryFilter } from "@/ipc/generated";

const FILTERS = [
  { value: "all", label: "All" },
  { value: "agent", label: "Agent" },
  { value: "other", label: "Other" },
] as const satisfies readonly { value: HistoryFilter; label: string }[];

/** Browsing history's filter: All, Agent or Other. */
export function FilterMenu({ value, onChange }: { value: HistoryFilter; onChange: (value: HistoryFilter) => void }) {
  return (
    <SettingsSelect
      label="Filter browsing history"
      value={value}
      options={FILTERS}
      onChange={onChange}
      icon={<Filter aria-hidden className="size-icon-sm shrink-0" />}
      className="w-32"
    />
  );
}
