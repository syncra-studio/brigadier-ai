import { BrowserIcon } from "@/app/settings/BrowserIcon";
import { BrowserStatus } from "@/app/settings/BrowserStatus";
import { Badge, Button } from "@/components/controls";
import type { ImportSource } from "@/ipc/generated";
import { importedBadge, rowView } from "@/state/browserImport";

/** One browser in Import Browser Data: its icon, name and badge, where it stands, its button. */
export function BrowserRow({
  source,
  nowMs,
  disabled,
  onAction,
}: {
  source: ImportSource;
  nowMs: number;
  disabled: boolean;
  onAction: () => void;
}) {
  const view = rowView(source);
  return (
    <div data-slot="import-row" data-browser={source.key} className="flex items-center gap-3 px-4 py-3">
      <BrowserIcon source={source} />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <div className="flex min-w-0 items-center gap-2">
          <span className="text-label truncate font-medium">{source.name}</span>
          {source.record && (
            <Badge data-slot="import-badge" variant="secondary" className="font-normal">
              {importedBadge(source.record, nowMs)}
            </Badge>
          )}
        </div>
        <div className="text-foreground/65 flex min-w-0 flex-wrap items-center gap-x-3 gap-y-0.5 text-xs">
          <BrowserStatus tone={view.tone} status={view.status} />
          {view.details.map((detail) => (
            <span key={detail} data-slot="import-detail" className="min-w-0">
              {detail}
            </span>
          ))}
        </div>
      </div>
      {view.label && (
        <Button variant="soft" size="sm" disabled={disabled || view.action === "none"} onClick={onAction}>
          {view.label}
        </Button>
      )}
    </div>
  );
}
