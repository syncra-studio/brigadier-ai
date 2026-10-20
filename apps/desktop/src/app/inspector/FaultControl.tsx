import { useId, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ChevronDown } from "@/components/ui/icon";
import { Input } from "@/components/ui/input";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { request } from "@/ipc/client";
import type { FaultTarget, ProviderKind } from "@/ipc/generated";
import { PROVIDERS, VENDOR_LABELS } from "@/lib/routing";
import { useBoard } from "@/state/board";
import { selectedConversation, useApp } from "@/state/store";

/** Windows to offer before a provider's own are known. */
const USUAL_WINDOWS: Record<ProviderKind, string[]> = {
  claude: ["five_hour", "seven_day"],
  codex: ["primary", "secondary"],
};

/** A radio menu behind an outline button. */
function Choice({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: string;
  options: Array<{ value: string; label: string }>;
  onChange: (value: string) => void;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button size="xs" variant="outline" aria-label={label} className="max-w-full justify-self-start">
          <span className="truncate">
            {options.find((option) => option.value === value)?.label ?? "Pick…"}
          </span>
          <ChevronDown />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="max-w-sm">
        <DropdownMenuRadioGroup value={value} onValueChange={onChange}>
          {options.map((option) => (
            <DropdownMenuRadioItem key={option.value} value={option.value}>
              <span className="truncate">{option.label}</span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/**
 * Development builds only: makes a provider refuse work as if a usage window ran out, for one
 * task's worker or one conversation's model, after its next N tool calls. Everything after
 * that is the real fallback path.
 */
export function FaultControl() {
  const id = useId();
  const tasks = useBoard((s) => s.board?.tasks);
  const conversations = useApp((s) => s.conversations);
  const providers = useApp((s) => s.providers.view?.providers);
  const [kind, setKind] = useState<FaultTarget["type"]>("task");
  const [taskId, setTaskId] = useState("");
  const [conversationId, setConversationId] = useState(
    () => selectedConversation(useApp.getState())?.id ?? "",
  );
  const [provider, setProvider] = useState<ProviderKind>("claude");
  const [windowId, setWindowId] = useState("five_hour");
  const [minutes, setMinutes] = useState("30");
  const [after, setAfter] = useState("3");
  const inject = useAction();
  const [sent, setSent] = useState<string | null>(null);

  const taskOptions = Object.values(tasks ?? {})
    .toSorted((a, b) => a.number - b.number)
    .map((task) => ({ value: task.id, label: `task-${task.number} · ${task.title} (${task.state})` }));
  const conversationOptions = Object.values(conversations)
    .filter((conversation) => conversation.lifecycle !== "archived" && conversation.sideOf === null)
    .toSorted((a, b) => b.updatedAtMs - a.updatedAtMs)
    .map((conversation) => ({ value: conversation.id, label: conversation.title }));
  const overview = providers?.find((entry) => entry.provider === provider);
  const known = [
    ...(overview?.usage?.windows.map((state) => state.window.id) ?? []),
    ...(overview?.quota?.windows.map((entry) => entry.id) ?? []),
    ...USUAL_WINDOWS[provider],
  ];
  const windowOptions = [...new Set(known)].map((value) => ({ value, label: value }));

  const resetIn = Number(minutes);
  const toolCalls = Number(after);
  const target: FaultTarget | null =
    kind === "task"
      ? // Only a task of the open board: a pick left from another conversation doesn't count.
        taskId && taskOptions.some((option) => option.value === taskId)
        ? { type: "task", taskId }
        : null
      : conversationId
        ? { type: "conversation", conversationId }
        : null;
  const valid =
    target !== null &&
    windowId !== "" &&
    Number.isInteger(resetIn) &&
    resetIn >= 1 &&
    Number.isInteger(toolCalls) &&
    toolCalls >= 0;

  const submit = () => {
    if (!target || !valid) return;
    setSent(null);
    inject.run(async () => {
      await request({
        method: "debugInjectLimit",
        target,
        provider,
        window: windowId,
        resetInMinutes: resetIn,
        afterToolCalls: toolCalls,
      });
      setSent(
        `${VENDOR_LABELS[provider]}'s ${windowId} limit armed${toolCalls > 0 ? ` for after ${toolCalls} tool calls` : ""}, resetting in ${resetIn} min.`,
      );
    });
  };

  return (
    <section className="flex flex-col gap-2 border-b px-3 py-3 text-xs">
      <h3 className="text-xs font-medium">Inject a usage limit (development builds)</h3>
      <div className="grid grid-cols-[auto_1fr] items-center gap-x-3 gap-y-2">
        <span className="text-muted-foreground">For</span>
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <ToggleGroup
            type="single"
            size="sm"
            variant="outline"
            spacing="tight"
            aria-label="Target"
            value={kind}
            onValueChange={(value) => value && setKind(value as FaultTarget["type"])}
          >
            <ToggleGroupItem value="task" className="text-xs">
              Task
            </ToggleGroupItem>
            <ToggleGroupItem value="conversation" className="text-xs">
              Conversation
            </ToggleGroupItem>
          </ToggleGroup>
          {kind === "task" ? (
            taskOptions.length > 0 ? (
              <Choice label="Task" value={taskId} options={taskOptions} onChange={setTaskId} />
            ) : (
              <span className="text-muted-foreground">Open a session with workers.</span>
            )
          ) : (
            <Choice
              label="Conversation"
              value={conversationId}
              options={conversationOptions}
              onChange={setConversationId}
            />
          )}
        </div>
        <span className="text-muted-foreground">Provider</span>
        <ToggleGroup
          type="single"
          size="sm"
          variant="outline"
          spacing="tight"
          aria-label="Provider"
          value={provider}
          onValueChange={(value) => {
            if (!value) return;
            setProvider(value as ProviderKind);
            setWindowId(USUAL_WINDOWS[value as ProviderKind][0] ?? "");
          }}
        >
          {PROVIDERS.map((entry) => (
            <ToggleGroupItem key={entry} value={entry} className="text-xs">
              {VENDOR_LABELS[entry]}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
        <span className="text-muted-foreground">Window</span>
        <Choice label="Window" value={windowId} options={windowOptions} onChange={setWindowId} />
        <label htmlFor={`${id}-reset`} className="text-muted-foreground">
          Resets in (min)
        </label>
        <Input
          id={`${id}-reset`}
          type="number"
          min={1}
          step={1}
          value={minutes}
          className="max-w-2xs"
          onChange={(event) => setMinutes(event.target.value)}
        />
        <label htmlFor={`${id}-after`} className="text-muted-foreground">
          After tool calls
        </label>
        <Input
          id={`${id}-after`}
          type="number"
          min={0}
          step={1}
          value={after}
          className="max-w-2xs"
          onChange={(event) => setAfter(event.target.value)}
        />
      </div>
      <div className="flex items-center gap-2">
        <Button size="xs" variant="outline" disabled={!valid || inject.busy} onClick={submit}>
          Inject limit
        </Button>
        <span className="text-muted-foreground">With 0 tool calls, the limit hits at once.</span>
      </div>
      {sent && !inject.error && <p className="text-muted-foreground">{sent}</p>}
      {inject.error && <p className="text-destructive">{inject.error}</p>}
    </section>
  );
}
