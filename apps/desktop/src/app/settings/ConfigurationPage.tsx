import { useEffect, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { SettingsPage, SettingsSection, SwitchSetting } from "@/app/settings/parts";
import { findModel, ModelSelector, type ModelGroup, SettingsCard, SettingsRow } from "@/components/shared";
import { Badge, Button, SettingsSelect, selectTrigger } from "@/components/controls";
import { request } from "@/ipc/client";
import type { ModelChoice, PermissionLevel } from "@/ipc/generated";
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
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

/**
 * What each level lets a session's lead and workers do, then what it risks, in plain words: the
 * permission menu's line under each level.
 */
export const LEVEL_COPY: Record<PermissionLevel, string> = {
  ask: "Asks you before it edits a file or runs a command. The safest level: work waits for you while you’re away.",
  acceptEdits:
    "Edits files freely and asks you before other commands. The project’s files can change without a word from you.",
  full: "Never asks: runs commands like your own terminal. A wrong command or planted instructions can delete or leak any of your files.",
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
  ask: levelRow("ask"),
  acceptEdits: levelRow("acceptEdits"),
  full: levelRow("full"),
  startWorkersOnOwn: {
    label: "Start workers on its own",
    description: "Choose whether a session starts helpers when they help, or asks you first",
  },
  fullAccessNotice: {
    label: "Show the Full access notice",
    description: "A card above the composer while a session is on Full access",
  },
  brigadeSkill: {
    label: "How sessions run workers",
    description: "Brigadier’s brigade skill. Your own copy at ~/.brigadier/brigade/SKILL.md replaces it",
  },
  enrichBrain: {
    label: "Use spare quota to deepen the Brain",
    description:
      "Before a usage window resets with quota left, a cheap model studies your projects further",
  },
} as const;

/**
 * How Brigadier works for the user: one card of the defaults new sessions and chats start with
 * (models, permission level, starting workers), what the composer
 * shows, and the Brain's spare quota. Each row says in a line what it chooses; its menu explains the
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
        <SwitchSetting setting="startWorkersOnOwn" row={CONFIGURATION_ROWS.startWorkersOnOwn} />
      </SettingsCard>

      <SettingsSection title="Workers">
        <SettingsCard>
          <BrigadeSkillRow />
        </SettingsCard>
      </SettingsSection>
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

/** The brigade skill sessions get: the shipped one, or the user's copy, which Reset removes. */
function BrigadeSkillRow() {
  const reset = useAction();
  const [skill, setSkill] = useState<{ custom: boolean; path: string } | null>(null);
  useEffect(() => {
    void request({ method: "getBrigadeSkill" }).then(setSkill, () => setSkill(null));
  }, []);
  return (
    <SettingsRow
      label={CONFIGURATION_ROWS.brigadeSkill.label}
      description={skill?.custom ? `Your copy at ${skill.path} is in use` : CONFIGURATION_ROWS.brigadeSkill.description}
      error={reset.error}
    >
      {skill?.custom && (
        <Button
          variant="soft"
          size="sm"
          disabled={reset.busy}
          onClick={() => reset.run(() => request({ method: "resetBrigadeSkill" }).then(setSkill))}
        >
          Reset to shipped
        </Button>
      )}
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
            level === "full" ? (
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

