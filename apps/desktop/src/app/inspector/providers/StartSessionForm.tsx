import { useState } from "react";

import { ActionError } from "@/app/inspector/providers/ActionError";
import { RecordToggle } from "@/app/inspector/providers/RecordToggle";
import { Section, useAction } from "@/app/inspector/providers/shared";
import { Button, Input, ToggleGroup, ToggleGroupItem } from "@/components/controls";
import { ModelSelector } from "@/components/shared";
import { pickFolder } from "@/ipc/client";
import type { ModelChoice } from "@/ipc/generated";
import { startRawSession } from "@/state/actions";
import { useApp } from "@/state/store";
import { PLAIN_FIELD } from "@/lib/plainTyping";
import { useModelGroups } from "@/lib/setup";
import { cliFacts, PROVIDERS } from "@/lib/providerKinds";

export function StartSessionForm() {
  const overviews = useApp((s) => s.providers.view?.providers ?? []);
  const groups = useModelGroups();
  // No model: the CLI's own default.
  const [choice, setChoice] = useState<ModelChoice>({ provider: PROVIDERS[0]!, model: null, effort: null });
  const [cwd, setCwd] = useState("");
  const [access, setAccess] = useState<"acceptEdits" | "readOnly">("acceptEdits");
  const [approvals, setApprovals] = useState<"delegated" | "declineAll">("delegated");
  const [record, setRecord] = useState(false);
  const start = useAction();

  const overview = overviews.find((entry) => entry.provider === choice.provider);
  const canStart = cwd.trim().startsWith("/") && overview?.status?.loggedIn === true;

  const submit = () =>
    void start.run(() =>
      startRawSession({
        provider: choice.provider,
        cwd: cwd.trim(),
        model: choice.model,
        effort: choice.effort,
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
          <ModelSelector
            groups={groups}
            value={choice}
            onChange={setChoice}
            agentDefault={(provider) => cliFacts(provider).vendor}
            noFast
          />
        </div>
        <div className="flex items-center gap-2">
          <Input
            aria-label="Working directory"
            placeholder="Working directory (absolute path)"
            value={cwd}
            onChange={(event) => setCwd(event.target.value)}
            className="h-control-sm min-w-0 flex-1 font-mono text-xs"
            {...PLAIN_FIELD}
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
