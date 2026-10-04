import { useAction } from "@/app/conversation/useAction";
import {
  SettingsButton,
  SettingsCard,
  SettingsPage,
  SettingsRow,
  SettingsSection,
  SettingsSelect,
  selectTrigger,
  SwitchSetting,
} from "@/app/settings/parts";
import {
  findModel,
  ModelSelector,
  type ModelGroup,
} from "@/components/assistant-ui/elements/model-selector";
import { Badge } from "@/components/ui/badge";
import type { ModelChoice, PermissionLevel } from "@/ipc/generated";
import {
  NEVER_PUSHES_NOTE,
  builtInDefault,
  modelName,
  PERMISSION_DETAILS,
  PERMISSION_LABELS,
  PERMISSION_LEVELS,
  useAvailableModelGroups,
  useModelGroups,
  withChoice,
} from "@/lib/setup";
import { cn } from "@/lib/utils";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

/** The Conversations page's rows, for the page and for Settings search. */
export const CONVERSATIONS_ROWS = {
  orchestrator: {
    label: "Orchestrator model",
    description: "Used by projects that have not remembered their own.",
  },
  chat: { label: "Chat model", description: "Used by new Chats." },
  shortReplies: {
    label: "Short replies",
    description:
      "The orchestrator answers in a few plain lines, overnight runs included. Turn off for fuller answers.",
  },
  permission: {
    label: "Default permission level",
    description: `A project remembers its own, which wins over this. ${NEVER_PUSHES_NOTE}`,
  },
  contextUsage: {
    label: "Show context window usage",
    description: "A ring by the model picker in the composer shows how full the model's context is.",
  },
  fullAccessNotice: {
    label: "Show the Full access notice",
    description:
      "A card above the composer while a conversation's workers run without the OS sandbox.",
  },
} as const;

export function ConversationsPage() {
  const groups = useAvailableModelGroups();
  const orchestrator = useApp((s) => s.settings.defaultOrchestrator);
  const chatModel = useApp((s) => s.settings.defaultChatModel);
  const permission = useApp((s) => s.settings.defaultPermission);
  const automatic = builtInDefault(groups);

  return (
    <SettingsPage
      title="Conversations"
      description="Defaults new sessions and chats start from. A project remembers its own choices, which win over these."
    >
      <SettingsSection title="Models">
        <SettingsCard>
          <DefaultModelRow
            setting="defaultOrchestrator"
            label={CONVERSATIONS_ROWS.orchestrator.label}
            description={
              orchestrator
                ? CONVERSATIONS_ROWS.orchestrator.description
                : `Automatic: the first signed-in agent's default (${modelName(groups, automatic)}).`
            }
            groups={groups}
            value={orchestrator}
            fallback={automatic}
          />
          <DefaultModelRow
            setting="defaultChatModel"
            label={CONVERSATIONS_ROWS.chat.label}
            description={
              chatModel
                ? CONVERSATIONS_ROWS.chat.description
                : "Automatic: the same as the orchestrator model."
            }
            groups={groups}
            value={chatModel}
            fallback={orchestrator ?? automatic}
          />
        </SettingsCard>
      </SettingsSection>

      <SettingsSection title="Replies">
        <SettingsCard>
          <SwitchSetting setting="shortReplies" row={CONVERSATIONS_ROWS.shortReplies} />
        </SettingsCard>
      </SettingsSection>

      <SettingsSection title="Permissions">
        <SettingsCard>
          <PermissionRow value={permission} />
        </SettingsCard>
      </SettingsSection>

      <SettingsSection title="Composer">
        <SettingsCard>
          <SwitchSetting setting="showContextUsage" row={CONVERSATIONS_ROWS.contextUsage} />
          <SwitchSetting setting="showFullAccessNotice" row={CONVERSATIONS_ROWS.fullAccessNotice} />
        </SettingsCard>
      </SettingsSection>
    </SettingsPage>
  );
}

/** A default model: "Automatic" (null) or a picked agent, model and effort. */
function DefaultModelRow({
  setting,
  label,
  description,
  groups,
  value,
  fallback,
}: {
  setting: "defaultOrchestrator" | "defaultChatModel";
  label: string;
  description: string;
  groups: readonly ModelGroup[];
  value: ModelChoice | null;
  fallback: ModelChoice;
}) {
  const save = useAction();
  const all = useModelGroups();
  const change = (choice: ModelChoice | null) => save.run(() => setSetting(setting, choice));
  // A saved model made unavailable on Providers stays saved; new conversations skip it.
  const gone = value !== null && findModel(all, value) !== null && findModel(groups, value) === null;
  return (
    <SettingsRow
      label={label}
      description={
        gone ? `${description} It isn't available now, so new conversations use another model.` : description
      }
      error={save.error}
    >
      {value && <SettingsButton onClick={() => change(null)}>Use automatic</SettingsButton>}
      <ModelSelector
        label={label}
        groups={value ? withChoice(groups, all, value) : groups}
        value={value ?? fallback}
        defaultChoice={fallback}
        onChange={change}
        className={cn(selectTrigger, value && "text-foreground")}
      />
    </SettingsRow>
  );
}

function PermissionRow({ value }: { value: PermissionLevel }) {
  const save = useAction();
  return (
    <SettingsRow
      label={CONVERSATIONS_ROWS.permission.label}
      description={`${PERMISSION_DETAILS[value]}. ${CONVERSATIONS_ROWS.permission.description}`}
      error={save.error}
    >
      <SettingsSelect<PermissionLevel>
        label={CONVERSATIONS_ROWS.permission.label}
        value={value}
        options={PERMISSION_LEVELS.map((level) => ({
          value: level,
          label:
            level === "fullAccess" ? (
              <Badge className="bg-full-access/15 text-full-access">{PERMISSION_LABELS[level]}</Badge>
            ) : (
              PERMISSION_LABELS[level]
            ),
          hint: `${PERMISSION_DETAILS[level]}.`,
        }))}
        onChange={(level) => save.run(() => setSetting("defaultPermission", level))}
      />
    </SettingsRow>
  );
}
