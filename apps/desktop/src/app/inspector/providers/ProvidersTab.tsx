import { ChevronRight, Reload } from "@openai/apps-sdk-ui/components/Icon";
import { type ReactNode, useEffect, useState } from "react";

import { Picker } from "@/app/inspector/providers/Picker";
import { RawSessionView } from "@/app/inspector/providers/RawSessionView";
import {
  PROVIDER_LABELS,
  QuotaWindows,
  STATE_VARIANTS,
} from "@/app/inspector/providers/shared";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { Input } from "@/components/ui/input";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { pickFolder } from "@/ipc/client";
import type { ProviderKind, ProviderOverview, RawSession } from "@/ipc/generated";
import { formatDateTime } from "@/lib/format";
import {
  loadProviders,
  refreshProviders,
  replayFixture,
  selectRawSession,
  simulateUsageLimit,
  startRawSession,
} from "@/state/actions";
import { useApp } from "@/state/store";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Runs an action, keeping its error for display. */
function useAction() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, run };
}

function ActionError({ error }: { error: string | null }) {
  if (!error) return null;
  return <p className="text-destructive text-xs">{error}</p>;
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

function Section({
  title,
  actions,
  children,
}: {
  title: string;
  actions?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="flex flex-col gap-2 border-b px-3 py-3">
      <div className="flex items-center gap-2">
        <h3 className="flex-1 text-xs font-medium">{title}</h3>
        {actions}
      </div>
      {children}
    </section>
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

/** Model choices for a provider: the CLI's default, then its live list. */
function modelOptions(overview: ProviderOverview | undefined) {
  return [
    { value: "", label: "CLI default" },
    ...(overview?.models?.models ?? []).map((model) => ({
      value: model.id,
      label: model.id,
      hint: model.displayName,
    })),
  ];
}

function StartSessionForm() {
  const overviews = useApp((s) => s.providers.view?.providers ?? []);
  const [provider, setProvider] = useState<ProviderKind>("claude");
  const [cwd, setCwd] = useState("");
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState("");
  const [access, setAccess] = useState<"workspace" | "readOnly">("workspace");
  const [approvals, setApprovals] = useState<"delegated" | "declineAll">("delegated");
  const [record, setRecord] = useState(false);
  const start = useAction();

  const overview = overviews.find((entry) => entry.provider === provider);
  const chosen = overview?.models?.models.find((entry) => entry.id === model);
  const efforts = chosen?.efforts ?? [];
  const canStart = cwd.trim().startsWith("/") && overview?.status?.loggedIn === true;

  const submit = () =>
    void start.run(() =>
      startRawSession({
        provider,
        cwd: cwd.trim(),
        model: model || null,
        effort: effort || null,
        access: access === "workspace" ? { type: "workspace", extraRoots: [] } : { type: "readOnly" },
        approvals,
        record,
      }),
    );

  return (
    <Section title="Start a raw session">
      <form
        className="flex flex-col gap-2 text-xs"
        onSubmit={(event) => {
          event.preventDefault();
          if (canStart) submit();
        }}
      >
        <div className="flex items-center gap-2">
          <ToggleGroup
            type="single"
            size="sm"
            variant="outline"
            value={provider}
            onValueChange={(value) => {
              if (value === "claude" || value === "codex") {
                setProvider(value);
                setModel("");
                setEffort("");
              }
            }}
            aria-label="Provider"
          >
            <ToggleGroupItem value="claude" className="text-xs">
              Claude Code
            </ToggleGroupItem>
            <ToggleGroupItem value="codex" className="text-xs">
              Codex
            </ToggleGroupItem>
          </ToggleGroup>
          <Picker
            label="Model"
            value={model}
            options={modelOptions(overview)}
            onChange={(value) => {
              setModel(value);
              setEffort("");
            }}
          />
          <Picker
            label="Reasoning effort"
            value={effort}
            options={[
              { value: "", label: "Default effort" },
              ...efforts.map((level) => ({ value: level, label: level })),
            ]}
            onChange={setEffort}
            disabled={efforts.length === 0}
          />
        </div>
        <div className="flex items-center gap-2">
          <Input
            aria-label="Working directory"
            placeholder="Working directory (absolute path)"
            value={cwd}
            onChange={(event) => setCwd(event.target.value)}
            className="h-control-sm min-w-0 flex-1 font-mono text-xs"
            spellCheck={false}
          />
          <Button
            type="button"
            size="xs"
            variant="outline"
            onClick={() =>
              void pickFolder(cwd.trim()).then((folder) => {
                if (folder) setCwd(folder);
              })
            }
          >
            Browse…
          </Button>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <ToggleGroup
            type="single"
            size="sm"
            variant="outline"
            value={access}
            onValueChange={(value) => {
              if (value === "workspace" || value === "readOnly") setAccess(value);
            }}
            aria-label="Access"
          >
            <ToggleGroupItem value="workspace" className="text-xs">
              Workspace
            </ToggleGroupItem>
            <ToggleGroupItem value="readOnly" className="text-xs">
              Read-only
            </ToggleGroupItem>
          </ToggleGroup>
          <ToggleGroup
            type="single"
            size="sm"
            variant="outline"
            value={approvals}
            onValueChange={(value) => {
              if (value === "delegated" || value === "declineAll") setApprovals(value);
            }}
            aria-label="Approvals"
          >
            <ToggleGroupItem value="delegated" className="text-xs">
              Policy + me
            </ToggleGroupItem>
            <ToggleGroupItem value="declineAll" className="text-xs">
              Decline all
            </ToggleGroupItem>
          </ToggleGroup>
          <Button
            type="button"
            size="xs"
            variant={record ? "secondary" : "ghost"}
            aria-pressed={record}
            onClick={() => setRecord(!record)}
          >
            {record ? "Recording" : "Record"}
          </Button>
          <span className="flex-1" />
          <Button type="submit" size="xs" disabled={!canStart || start.busy}>
            Start
          </Button>
        </div>
        <ActionError error={start.error} />
      </form>
    </Section>
  );
}

function sessionTitle(session: RawSession): string {
  switch (session.source.type) {
    case "live":
      return session.cwd ?? "";
    case "replay":
    case "simulation":
      return session.source.title;
  }
}

function SessionList() {
  const sessions = useApp((s) => s.providers.view?.sessions ?? []);
  return (
    <Section title="Raw sessions">
      {sessions.length === 0 ? (
        <p className="text-muted-foreground text-xs">
          None yet. Start one above{import.meta.env.DEV ? ", replay a fixture or simulate a usage limit" : " or replay a fixture"}.
        </p>
      ) : (
        <ul className="flex flex-col">
          {sessions.map((session) => (
            <li key={session.id}>
              <button
                type="button"
                onClick={() => selectRawSession(session.id)}
                className="hover:bg-accent h-row-sm rounded-control flex w-full items-center gap-2 px-2 text-start text-xs"
              >
                <span className="w-20 shrink-0">{PROVIDER_LABELS[session.provider]}</span>
                <Badge variant={STATE_VARIANTS[session.state]}>{session.state}</Badge>
                {session.source.type !== "live" && (
                  <Badge variant="outline">{session.source.type}</Badge>
                )}
                <span className="text-muted-foreground min-w-0 flex-1 truncate font-mono">
                  {sessionTitle(session)}
                </span>
                <span className="text-muted-foreground shrink-0">
                  {formatDateTime(session.createdAtMs)}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </Section>
  );
}

function FixtureList() {
  const fixtures = useApp((s) => s.providers.view?.fixtures ?? []);
  const replay = useAction();
  return (
    <Section title="Replay fixtures">
      <ActionError error={replay.error} />
      {fixtures.length === 0 ? (
        <p className="text-muted-foreground text-xs">
          No fixtures. Sessions started with Record are saved here.
        </p>
      ) : (
        <ul className="flex flex-col gap-1">
          {fixtures.map((fixture) => (
            <li key={fixture.id} className="flex items-center gap-2 text-xs">
              <span className="w-20 shrink-0">{PROVIDER_LABELS[fixture.provider]}</span>
              <span className="min-w-0 flex-1 truncate" title={fixture.id}>
                {fixture.title}
                <span className="text-muted-foreground">
                  {" "}
                  · {fixture.lines} lines
                  {fixture.cliVersion && ` · v${fixture.cliVersion}`}
                  {fixture.id.startsWith("builtin:") ? " · built in" : " · recorded"}
                </span>
              </span>
              <Button
                size="xs"
                variant="outline"
                disabled={replay.busy}
                onClick={() => void replay.run(() => replayFixture(fixture.id))}
              >
                Replay
              </Button>
            </li>
          ))}
        </ul>
      )}
    </Section>
  );
}
