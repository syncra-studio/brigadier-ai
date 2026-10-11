import { useEffect, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { errorText } from "@/app/dialogs/fields";
import { MemoriesDialog } from "@/app/settings/MemoriesDialog";
import {
  SettingsAdvanced,
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
import type { ModelChoice, Node, PermissionLevel } from "@/ipc/generated";
import {
  builtInDefault,
  modelName,
  PERMISSION_LABELS,
  PERMISSION_LEVELS,
  useAvailableModelGroups,
  useModelGroups,
  withChoice,
} from "@/lib/setup";
import { cn } from "@/lib/utils";
import { listMemories } from "@/state/brain";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

/**
 * What each level lets a session's lead and workers do, then what it risks, in plain words: the
 * permission menu's line under each level.
 */
export const LEVEL_COPY: Record<PermissionLevel, string> = {
  askForApproval:
    "Works in a sandbox, without the internet, and asks you before each plan and anything outside it. The safest level: work waits for you while you’re away.",
  approveForMe:
    "The same sandbox with the internet; a reviewer approves plans and steps outside it for you. The reviewer can be wrong, and the project’s data can be sent out.",
  fullAccess:
    "No sandbox: runs commands like your own terminal, without asking. A wrong command or planted instructions can delete or leak any of your files.",
};

const PERMISSION_LABEL = "Default permission level";

/** A level's row for search: what it allows and risks, found at the permission row. */
const levelRow = (level: PermissionLevel) => ({
  label: PERMISSION_LABELS[level],
  description: LEVEL_COPY[level],
  target: PERMISSION_LABEL,
});

/** The Configuration page's rows, for the page and for Settings search. */
export const CONFIGURATION_ROWS = {
  sessionModel: {
    label: "Model for sessions",
    description: "Choose the model that leads new sessions, their orchestrator",
  },
  chatModel: { label: "Model for chats", description: "Choose the model new chats start with" },
  permission: {
    label: PERMISSION_LABEL,
    description: "Choose how much new sessions may do without asking",
  },
  askForApproval: levelRow("askForApproval"),
  approveForMe: levelRow("approveForMe"),
  fullAccess: levelRow("fullAccess"),
  reviewWork: {
    label: "Review the work",
    description: "Choose whether another AI model reviews each change",
  },
  shortReplies: {
    label: "Short replies",
    description: "Choose whether sessions answer in a few plain lines",
  },
  memories: {
    label: "Memories",
    description: "What Brigadier remembers about you, used by every chat and session",
  },
  fullAccessNotice: {
    label: "Show the Full access notice",
    description:
      "A card above the composer while a conversation's workers run without the OS sandbox",
  },
  enrichBrain: {
    label: "Use spare quota to deepen the Brain",
    description:
      "Before a usage window resets with quota left, a cheap model studies your projects further",
  },
} as const;

const ADVANCED_LABELS = [
  CONFIGURATION_ROWS.fullAccessNotice.label,
  CONFIGURATION_ROWS.enrichBrain.label,
];

/**
 * How Brigadier works for the user: one card of the defaults new sessions and chats start with
 * (models, permission level, reviews, reply length), what Brigadier remembers, then display and
 * Brain extras under Advanced. Each row says in a line what it chooses; its menu explains the
 * choices.
 */
export function ConfigurationPage() {
  const groups = useAvailableModelGroups();
  const orchestrator = useApp((s) => s.settings.defaultOrchestrator);
  const chatModel = useApp((s) => s.settings.defaultChatModel);
  const permission = useApp((s) => s.settings.defaultPermission);
  const automatic = builtInDefault(groups);

  return (
    <SettingsPage
      title="Configuration"
      description="Choose how Brigadier works on your tasks. A project’s own model and level win over these."
    >
      <SettingsCard>
        <DefaultModelRow
          setting="defaultOrchestrator"
          row={CONFIGURATION_ROWS.sessionModel}
          hint="The first signed-in agent’s default model."
          groups={groups}
          value={orchestrator}
          fallback={automatic}
        />
        <DefaultModelRow
          setting="defaultChatModel"
          row={CONFIGURATION_ROWS.chatModel}
          hint="The same model as new sessions."
          groups={groups}
          value={chatModel}
          fallback={orchestrator ?? automatic}
        />
        <PermissionRow value={permission} />
        <SwitchSetting setting="reviewWork" row={CONFIGURATION_ROWS.reviewWork} />
        <SwitchSetting setting="shortReplies" row={CONFIGURATION_ROWS.shortReplies} />
      </SettingsCard>

      <SettingsSection title="Memory">
        <SettingsCard>
          <MemoriesRow />
        </SettingsCard>
      </SettingsSection>

      <SettingsAdvanced
        description="What the composer shows, and spare quota for the Brain."
        labels={ADVANCED_LABELS}
      >
        <SettingsSection title="Composer">
          <SettingsCard>
            <SwitchSetting
              setting="showFullAccessNotice"
              row={CONFIGURATION_ROWS.fullAccessNotice}
            />
          </SettingsCard>
        </SettingsSection>
        <SettingsSection title="Brain">
          <SettingsCard>
            <SwitchSetting setting="enrichBrain" row={CONFIGURATION_ROWS.enrichBrain} />
          </SettingsCard>
        </SettingsSection>
      </SettingsAdvanced>
    </SettingsPage>
  );
}

/**
 * A default model: "Automatic (‹what it picks now›)", the menu's first choice, or a picked
 * agent, model and effort.
 */
function DefaultModelRow({
  setting,
  row,
  hint,
  groups,
  value,
  fallback,
}: {
  setting: "defaultOrchestrator" | "defaultChatModel";
  row: { label: string; description: string };
  /** What Automatic picks, under it in the menu. */
  hint: string;
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
      label={row.label}
      description={gone ? "Not available now, so new ones use another model" : row.description}
      error={save.error}
    >
      <ModelSelector
        label={row.label}
        groups={value ? withChoice(groups, all, value) : groups}
        value={value ?? fallback}
        defaultChoice={fallback}
        onChange={change}
        automatic={{
          chosen: value === null,
          label: `Automatic (${modelName(groups, fallback)})`,
          hint,
          onChoose: () => change(null),
        }}
        className={cn(selectTrigger, "text-foreground")}
      />
    </SettingsRow>
  );
}

function PermissionRow({ value }: { value: PermissionLevel }) {
  const save = useAction();
  return (
    <SettingsRow
      label={CONFIGURATION_ROWS.permission.label}
      description={CONFIGURATION_ROWS.permission.description}
      error={save.error}
    >
      <SettingsSelect<PermissionLevel>
        label={CONFIGURATION_ROWS.permission.label}
        value={value}
        options={PERMISSION_LEVELS.map((level) => ({
          value: level,
          label:
            level === "fullAccess" ? (
              <Badge className="bg-full-access/15 text-full-access">{PERMISSION_LABELS[level]}</Badge>
            ) : (
              PERMISSION_LABELS[level]
            ),
          hint: LEVEL_COPY[level],
        }))}
        onChange={(level) => save.run(() => setSetting("defaultPermission", level))}
      />
    </SettingsRow>
  );
}

/** How many memories Brigadier keeps, and the way to see and forget them. */
function MemoriesRow() {
  const [memories, setMemories] = useState<Node[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    let current = true;
    listMemories().then(
      (list) => current && setMemories(list),
      (cause: unknown) => current && setError(errorText(cause)),
    );
    return () => {
      current = false;
    };
  }, []);

  const count =
    memories === null ? null : memories.length === 0 ? "nothing saved yet" : `${memories.length} saved`;
  return (
    <>
      <SettingsRow
        label={
          <>
            {CONFIGURATION_ROWS.memories.label}
            {count && <span className="text-foreground/65 font-normal"> · {count}</span>}
          </>
        }
        // Search and links find it by its name, whatever the count says.
        setting={CONFIGURATION_ROWS.memories.label}
        description={CONFIGURATION_ROWS.memories.description}
        error={error}
      >
        <SettingsButton disabled={memories === null} onClick={() => setOpen(true)}>
          Manage
        </SettingsButton>
      </SettingsRow>
      <MemoriesDialog
        open={open}
        onOpenChange={setOpen}
        memories={memories ?? []}
        onForgotten={(id) =>
          setMemories((list) => list?.filter((memory) => memory.id !== id) ?? null)
        }
      />
    </>
  );
}
