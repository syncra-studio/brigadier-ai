import { AgentModelRow } from "@/app/settings/providers/AgentModelRow";
import { SettingsSection } from "@/app/settings/parts";
import { agentState } from "@/app/setup/onboarding/SetupTerminal";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/layout";
import { ChevronRight } from "@/components/icons";
import { SettingsCard, settingsRows } from "@/components/shared";
import type { MergedModel, ModelInfo, ProviderKind, ProviderOverview } from "@/ipc/generated";
import { VENDOR_LABELS } from "@/lib/routing";
import { cn } from "@/lib/utils";
import { modelAvailable } from "@/state/providers";
import { useApp } from "@/state/store";

/** The Models section: a row each, a switch each; older models folded away. */
export function AgentModels({
  provider,
  overview,
  merged,
}: {
  provider: ProviderKind;
  overview: ProviderOverview | undefined;
  merged: readonly MergedModel[] | null;
}) {
  const models = overview?.models?.models ?? [];
  const label = VENDOR_LABELS[provider];
  const state = agentState(overview);
  const current = models.filter((model) => !model.legacy);
  const older = models.filter((model) => model.legacy);
  const settings = useApp((s) => s.settings);
  const onCount = current.filter((model) => modelAvailable(settings, { provider, id: model.id })).length;
  const aside =
    models.length > 0 ? [`${onCount} on`, older.length > 0 && `${older.length} older`].filter(Boolean).join(" · ") : undefined;
  const row = (model: ModelInfo) => (
    <AgentModelRow key={model.id} provider={provider} model={model} merged={merged} />
  );
  return (
    <SettingsSection title="Models" setting="Models" aside={aside}>
      <SettingsCard>
        {models.length === 0 ? (
          <p className="text-foreground/65 px-4 py-3 text-sm">
            {state === "checking"
              ? "Checking…"
              : state === "install"
                ? `Install ${label} to see its models.`
                : state === "signIn"
                  ? `Sign in to ${label} to see its models.`
                  : "Reading the models…"}
          </p>
        ) : (
          current.map(row)
        )}
        {older.length > 0 && (
          <Collapsible className={cn("flex flex-col", settingsRows)}>
            <CollapsibleTrigger
              className="text-foreground/65 hover:text-foreground text-label group flex w-full items-center gap-2.5 px-4 py-3"
            >
              <ChevronRight
                aria-hidden
                className="size-icon-sm transition-transform group-data-[state=open]:rotate-90 motion-reduce:transition-none"
              />
              Older models · {older.length}
            </CollapsibleTrigger>
            <CollapsibleContent className={cn("flex flex-col", settingsRows)}>
              {older.map(row)}
            </CollapsibleContent>
          </Collapsible>
        )}
      </SettingsCard>
    </SettingsSection>
  );
}
