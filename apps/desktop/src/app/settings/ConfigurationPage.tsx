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
  NEVER_PUSHES_NOTE,
  PERMISSION_DETAILS,
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

/** What each level lets a session's lead and workers do, and what it risks, in plain words. */
const LEVEL_COPY: Record<PermissionLevel, { allows: string; risk: string }> = {
  askForApproval: {
    allows:
      "The lead and its workers run in a sandbox. They can read files on this computer, except Brigadier’s own private folders, and write only in the session’s checkout or worktree, the repository’s Git folder, their scratch folders and toolchain caches. They have no internet, except workers that research the web. Anything outside that asks you first, on a card, and you give each plan its go-ahead.",
    risk: "The safest level. You answer more cards, and work waits for you while you’re away.",
  },
  approveForMe: {
    allows:
      "The same sandbox, with the internet. When a command needs to leave the sandbox, an automatic reviewer decides instead of you, and the lead gives plans their go-ahead for you. You’re asked only what only you can answer.",
    risk: "The reviewer can be wrong. With the internet on, what the work downloads runs in the sandbox and data in the project can be sent out.",
  },
  fullAccess: {
    allows:
      "No sandbox: the lead and its workers run commands like your own terminal, without asking. They can read, change or delete any file you can, install software and use the internet.",
    risk: "A wrong command, or instructions planted in a web page or file (prompt injection), can delete or leak data anywhere on this computer. Use it with projects and sources you trust.",
  },
};

const PERMISSION_LABEL = "Default permission level";

/** A level's row for search: what it allows and risks, found at the permission row. */
const levelRow = (level: PermissionLevel) => ({
  label: PERMISSION_LABELS[level],
  description: `${LEVEL_COPY[level].allows} ${LEVEL_COPY[level].risk}`,
  target: PERMISSION_LABEL,
});

/** The Configurations page's rows, for the page and for Settings search. */
export const CONFIGURATION_ROWS = {
  sessionModel: {
    label: "Model for sessions",
    description:
      "The model that leads new sessions (the orchestrator), where a project hasn’t picked its own.",
  },
  chatModel: { label: "Model for chats", description: "The model new Chats start with." },
  permission: {
    label: PERMISSION_LABEL,
    description:
      "Where new sessions start. Pick another level for one session in its composer; started sessions keep theirs.",
  },
  askForApproval: levelRow("askForApproval"),
  approveForMe: levelRow("approveForMe"),
  fullAccess: levelRow("fullAccess"),
  reviewWork: {
    label: "Review the work",
    description:
      "Another AI model reviews each change and reports what it finds. Turn off to skip these reviews and use less quota.",
  },
  shortReplies: {
    label: "Short replies",
    description:
      "Sessions answer in a few plain lines, overnight runs included. Turn off for fuller answers.",
  },
  memories: {
    label: "Memories",
    description:
      "What Brigadier remembers about you, such as your preferences, used by every Chat and session.",
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
  enrichBrain: {
    label: "Use spare quota to deepen the Brain",
    description:
      "Before a usage window resets with quota left, a cheap model studies your projects further.",
  },
} as const;

const ADVANCED_LABELS = [
  CONFIGURATION_ROWS.contextUsage.label,
  CONFIGURATION_ROWS.fullAccessNotice.label,
  CONFIGURATION_ROWS.enrichBrain.label,
];

/**
 * How Brigadier works for the user, in the order it's most often changed: the models new
 * sessions and chats start with, how much sessions may do without asking, how they work,
 * what Brigadier remembers, then display and Brain extras under Advanced.
 */
export function ConfigurationPage() {
  const groups = useAvailableModelGroups();
  const orchestrator = useApp((s) => s.settings.defaultOrchestrator);
  const chatModel = useApp((s) => s.settings.defaultChatModel);
  const permission = useApp((s) => s.settings.defaultPermission);
  const automatic = builtInDefault(groups);

  return (
    <SettingsPage
      title="Configurations"
      description="How Brigadier’s sessions and chats work for you. A project remembers its own model and level, which win over these."
    >
      <SettingsSection title="Models">
        <SettingsCard>
          <DefaultModelRow
            setting="defaultOrchestrator"
            row={CONFIGURATION_ROWS.sessionModel}
            description={
              orchestrator
                ? CONFIGURATION_ROWS.sessionModel.description
                : `Automatic: the first signed-in agent's default (${modelName(groups, automatic)}).`
            }
            groups={groups}
            value={orchestrator}
            fallback={automatic}
          />
          <DefaultModelRow
            setting="defaultChatModel"
            row={CONFIGURATION_ROWS.chatModel}
            description={
              chatModel
                ? CONFIGURATION_ROWS.chatModel.description
                : "Automatic: the same as the model for sessions."
            }
            groups={groups}
            value={chatModel}
            fallback={orchestrator ?? automatic}
          />
        </SettingsCard>
      </SettingsSection>

      <SettingsSection
        title="Permissions"
        description={`A session’s level covers its lead and every worker. ${NEVER_PUSHES_NOTE}`}
      >
        <SettingsCard>
          <PermissionRow value={permission} />
          <LevelRow level={permission} />
        </SettingsCard>
      </SettingsSection>

      <SettingsSection title="How sessions work">
        <SettingsCard>
          <SwitchSetting setting="reviewWork" row={CONFIGURATION_ROWS.reviewWork} />
          <SwitchSetting setting="shortReplies" row={CONFIGURATION_ROWS.shortReplies} />
        </SettingsCard>
      </SettingsSection>

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
            <SwitchSetting setting="showContextUsage" row={CONFIGURATION_ROWS.contextUsage} />
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

/** A default model: "Automatic" (null) or a picked agent, model and effort. */
function DefaultModelRow({
  setting,
  row,
  description,
  groups,
  value,
  fallback,
}: {
  setting: "defaultOrchestrator" | "defaultChatModel";
  row: { label: string };
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
      label={row.label}
      description={
        gone ? `${description} It isn't available now, so new conversations use another model.` : description
      }
      error={save.error}
    >
      {value && <SettingsButton onClick={() => change(null)}>Use automatic</SettingsButton>}
      <ModelSelector
        label={row.label}
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
          hint: `${PERMISSION_DETAILS[level]}.`,
        }))}
        onChange={(level) => save.run(() => setSetting("defaultPermission", level))}
      />
    </SettingsRow>
  );
}

/** The default level: what it allows, then its risk. */
function LevelRow({ level }: { level: PermissionLevel }) {
  return (
    <SettingsRow
      label={PERMISSION_LABELS[level]}
      description={
        <span className="flex flex-col gap-1">
          <span>{LEVEL_COPY[level].allows}</span>
          <span>
            <span className="text-foreground/80 font-medium">Risk: </span>
            {LEVEL_COPY[level].risk}
          </span>
          <span>
            Plan mode waits for you to approve its plan at every level, and in a folder you don’t
            trust, sessions always ask first.
          </span>
        </span>
      }
    >
      <Badge variant="secondary">Default</Badge>
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
    memories === null
      ? null
      : memories.length === 0
        ? "Nothing saved yet."
        : `${memories.length} saved.`;
  return (
    <>
      <SettingsRow
        label={CONFIGURATION_ROWS.memories.label}
        description={`${CONFIGURATION_ROWS.memories.description}${count ? ` ${count}` : ""}`}
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
