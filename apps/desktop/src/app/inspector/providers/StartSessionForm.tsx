import { useState } from "react";

import { ActionError } from "@/app/inspector/providers/ActionError";
import { Picker } from "@/app/inspector/providers/Picker";
import { RecordToggle } from "@/app/inspector/providers/RecordToggle";
import { Section, useAction } from "@/app/inspector/providers/shared";
import { Button, Input, ToggleGroup, ToggleGroupItem } from "@/components/controls";
import { pickFolder } from "@/ipc/client";
import type { ProviderKind, ProviderOverview } from "@/ipc/generated";
import { startRawSession } from "@/state/actions";
import { useApp } from "@/state/store";

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

export function StartSessionForm() {
  const overviews = useApp((s) => s.providers.view?.providers ?? []);
  const [provider, setProvider] = useState<ProviderKind>("claude");
  const [cwd, setCwd] = useState("");
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState("");
  const [access, setAccess] = useState<"acceptEdits" | "readOnly">("acceptEdits");
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
        access: access === "acceptEdits" ? { type: "acceptEdits" } : { type: "readOnly" },
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
              if (value === "acceptEdits" || value === "readOnly") setAccess(value);
            }}
            aria-label="Access"
          >
            <ToggleGroupItem value="acceptEdits" className="text-xs">
              Edit its folder
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
          <RecordToggle record={record} onRecordChange={setRecord} />
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
