import { useEffect, useState } from "react";

import { ActionError } from "@/app/inspector/providers/ActionError";
import { FixtureList } from "@/app/inspector/providers/FixtureList";
import { RawSessionView } from "@/app/inspector/providers/RawSessionView";
import { SessionList } from "@/app/inspector/providers/SessionList";
import {
  PROVIDER_LABELS,
  QuotaWindows,
  Section,
  useAction,
} from "@/app/inspector/providers/shared";
import { StartSessionForm } from "@/app/inspector/providers/StartSessionForm";
import { Badge, Button } from "@/components/controls";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/layout";
import { ChevronRight, Reload } from "@/components/icons";
import type { ProviderOverview } from "@/ipc/generated";
import { formatDateTime } from "@/lib/format";
import { loadProviders, refreshProviders, simulateUsageLimit } from "@/state/actions";
import { useApp } from "@/state/store";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Raw provider sessions, provider state and replay fixtures (Phase 2 adapter debugging). */
export function ProvidersTab() {
  const selected = useApp((s) => s.providers.selected);
  const loaded = useApp((s) => s.providers.view !== null);
  const connected = useApp((s) => s.connection.status === "connected");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!connected) return;
    loadProviders().then(
      () => setError(null),
      (err: unknown) => setError(errorText(err)),
    );
  }, [connected]);

  if (!loaded) {
    return (
      <p className="text-muted-foreground p-4 text-xs">
        {error ?? "Loading providers…"}
      </p>
    );
  }
  if (selected) return <RawSessionView id={selected} />;
  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <ProviderSection />
      <StartSessionForm />
      <SessionList />
      <FixtureList />
    </div>
  );
}

function ProviderSection() {
  const providers = useApp((s) => s.providers.view?.providers ?? []);
  const refresh = useAction();
  return (
    <Section
      title="Providers"
      actions={
        <Button
          size="xs"
          variant="ghost"
          disabled={refresh.busy}
          onClick={() => void refresh.run(refreshProviders)}
        >
          <Reload />
          Refresh
        </Button>
      }
    >
      <ActionError error={refresh.error} />
      {providers.map((overview) => (
        <ProviderCard key={overview.provider} overview={overview} />
      ))}
    </Section>
  );
}

function ProviderCard({ overview }: { overview: ProviderOverview }) {
  const { status, models, quota } = overview;
  const simulate = useAction();
  const ready = status?.loggedIn === true;
  return (
    <div className="bg-card flex flex-col gap-2 rounded-lg border p-3 text-xs">
      <div className="flex items-center gap-2">
        <span className="text-sm font-medium">{PROVIDER_LABELS[overview.provider]}</span>
        {status === null ? (
          <Badge variant="secondary">checking…</Badge>
        ) : ready ? (
          <Badge variant="success">ready</Badge>
        ) : (
          <Badge variant="warning">{status.path ? "not logged in" : "not installed"}</Badge>
        )}
        <span className="text-muted-foreground min-w-0 flex-1 truncate text-end">
          {[status?.version, status?.authMethod, status?.plan].filter(Boolean).join(" · ")}
        </span>
      </div>
      {status?.guidance && <p className="text-warning">{status.guidance}</p>}
      {overview.error && <p className="text-destructive">{overview.error}</p>}
      {quota && <QuotaWindows quota={quota} />}
      {models && (
        <Collapsible>
          <CollapsibleTrigger className="text-muted-foreground hover:text-foreground group flex items-center gap-1">
            <ChevronRight className="size-icon-xs transition-transform group-data-[state=open]:rotate-90" />
            {models.models.length} models · listed {formatDateTime(models.fetchedAtMs)}
          </CollapsibleTrigger>
          <CollapsibleContent>
            <ul data-selectable className="mt-1 flex flex-col gap-1">
              {models.models.map((model) => (
                <li key={model.id} className="flex items-baseline gap-2">
                  <span className="font-mono">{model.id}</span>
                  <span className="text-muted-foreground min-w-0 flex-1 truncate">
                    {model.displayName}
                    {model.efforts.length > 0 && ` · ${model.efforts.join("/")}`}
                    {model.isDefault && " · default"}
                  </span>
                </li>
              ))}
            </ul>
          </CollapsibleContent>
        </Collapsible>
      )}
      <div className="flex items-center gap-2">
        <span className="text-muted-foreground flex-1">
          {overview.checkedAtMs === null
            ? "Not checked yet"
            : `Checked ${formatDateTime(overview.checkedAtMs)}`}
        </span>
        {/* Development builds only: the daemon answers it in debug builds alone. */}
        {import.meta.env.DEV && (
          <Button
            size="xs"
            variant="outline"
            disabled={simulate.busy}
            onClick={() => void simulate.run(() => simulateUsageLimit(overview.provider))}
          >
            Simulate usage limit
          </Button>
        )}
      </div>
      <ActionError error={simulate.error} />
    </div>
  );
}
