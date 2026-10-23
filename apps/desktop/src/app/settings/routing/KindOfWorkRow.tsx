import { ROUTED_KINDS } from "@/app/settings/routing/words";
import { SettingsSelect } from "@/components/controls";
import { RowText } from "@/components/shared";
import type { TaskCategory } from "@/ipc/generated";
import { PLAIN_KIND_NAMES } from "@/lib/routing";

/** Model strength's first row: which kind of work the models are ranked for. */
export function KindOfWorkRow({
  category,
  onCategory,
}: {
  category: TaskCategory;
  onCategory: (category: TaskCategory) => void;
}) {
  return (
    <div className="border-divider flex items-center gap-6 border-y py-3.5">
      <RowText truncate quiet title="Kind of work" line="Which work to rank the models for" />
      <SettingsSelect
        label="The kind of work the models are ranked for"
        value={category}
        options={ROUTED_KINDS.map((kind) => ({ value: kind, label: PLAIN_KIND_NAMES[kind] }))}
        onChange={onCategory}
        align="end"
      />
    </div>
  );
}
