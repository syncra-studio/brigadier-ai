import { useAction } from "@/app/conversation/useAction";
import { ROW_LINE, ROW_TITLE } from "@/app/settings/providers/parts";
import { bestFor } from "@/app/settings/routing/models";
import { SettingsSwitch } from "@/app/settings/parts";
import { ErrorLine } from "@/components/controls";
import { InfoCircle } from "@/components/icons";
import type { MergedModel, ModelInfo, ProviderKind } from "@/ipc/generated";
import { isExpensive, TIER_LABELS } from "@/lib/routing";
import { cn } from "@/lib/utils";
import { modelAvailable, setModelAvailable } from "@/state/providers";
import { useApp } from "@/state/store";

/** The line under a model Brigadier gives no tasks to whatever the user chooses. */
const EXPENSIVE_NOTE =
  "Expensive: Brigadier uses it only where you pick it, in a chat, a session, your order or a rule";

/**
 * What a model's row shows and does: its switch, and its line (Expensive for a model Brigadier
 * never gives tasks, else how good it is and what for, else the agent's own words).
 */
function useModelRow(provider: ProviderKind, model: ModelInfo, merged: readonly MergedModel[] | null) {
  const ref = { provider, id: model.id };
  const available = useApp((s) => modelAvailable(s.settings, ref));
  const toggle = useAction();
  const known = merged?.find((entry) => entry.provider === provider && entry.id === model.id);
  const description =
    known && known.tier !== "unrated"
      ? [TIER_LABELS[known.tier], bestFor(known)].filter(Boolean).join(" · ")
      : model.description || null;
  return {
    available,
    toggle,
    expensive: isExpensive(model),
    description,
    setAvailable: (next: boolean) => toggle.run(() => setModelAvailable(ref, next)),
  };
}

/** A model on an agent's page: its name and id, its line, its switch. */
export function AgentModelRow({
  provider,
  model,
  merged,
}: {
  provider: ProviderKind;
  model: ModelInfo;
  merged: readonly MergedModel[] | null;
}) {
  const { available, toggle, expensive, description, setAvailable } = useModelRow(provider, model, merged);
  const text = ROW_LINE;
  return (
    <div data-slot="provider-model" data-setting={model.displayName} className="flex items-center gap-6 px-4 py-3">
      <div className="flex min-w-0 flex-1 flex-col gap-1.25">
        <span className="flex min-w-0 flex-wrap items-center gap-x-2.5 gap-y-1">
          <span className={ROW_TITLE}>{model.displayName}</span>
          <span className="text-foreground/65 font-mono text-xs whitespace-pre">
            {model.id}
            {model.isDefault && "  ·  default"}
          </span>
        </span>
        {expensive ? (
          <span className={cn(text, "flex items-center gap-1.5")}>
            <InfoCircle aria-hidden className="size-icon-xs shrink-0" />
            {EXPENSIVE_NOTE}
          </span>
        ) : (
          description && <span className={text}>{description}</span>
        )}
        <ErrorLine error={toggle.error} />
      </div>
      <SettingsSwitch
        label={`Use ${model.displayName}`}
        checked={available}
        disabled={toggle.busy}
        onCheckedChange={setAvailable}
      />
    </div>
  );
}
