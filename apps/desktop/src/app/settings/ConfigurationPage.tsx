import { useAction } from "@/app/conversation/useAction";
import {
  SettingsCard,
  SettingsPage,
  SettingsRow,
  SettingsSection,
  SettingsSelect,
} from "@/app/settings/parts";
import { Badge } from "@/components/ui/badge";
import type { PermissionLevel } from "@/ipc/generated";
import {
  NEVER_PUSHES_NOTE,
  PERMISSION_DETAILS,
  PERMISSION_LABELS,
  PERMISSION_LEVELS,
} from "@/lib/setup";
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

const levelRow = (level: PermissionLevel) => ({
  label: PERMISSION_LABELS[level],
  description: `${LEVEL_COPY[level].allows} ${LEVEL_COPY[level].risk}`,
});

/** The Configuration page's rows, for the page and for Settings search. */
export const CONFIGURATION_ROWS = {
  permission: {
    label: "Default permission level",
    description:
      "Where new sessions start. Pick another level for one session in its composer; changing this leaves started sessions as they are.",
  },
  askForApproval: levelRow("askForApproval"),
  approveForMe: levelRow("approveForMe"),
  fullAccess: levelRow("fullAccess"),
};

export function ConfigurationPage() {
  const permission = useApp((s) => s.settings.defaultPermission);
  return (
    <SettingsPage
      title="Configuration"
      description="Choose how much Brigadier’s sessions may do on this computer without asking you."
    >
      <SettingsSection title="Permissions">
        <SettingsCard>
          <PermissionRow value={permission} />
        </SettingsCard>
      </SettingsSection>

      <SettingsSection
        title="What each level allows"
        description={`A session’s level covers its lead and every worker. ${NEVER_PUSHES_NOTE} Plan mode waits for you to approve its plan at every level, and in a folder you don’t trust, sessions always ask first.`}
      >
        <SettingsCard>
          {PERMISSION_LEVELS.map((level) => (
            <LevelRow key={level} level={level} isDefault={level === permission} />
          ))}
        </SettingsCard>
      </SettingsSection>
    </SettingsPage>
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

/** One level: what it allows, then its risk, marked when it is the default. */
function LevelRow({ level, isDefault }: { level: PermissionLevel; isDefault: boolean }) {
  return (
    <SettingsRow
      label={CONFIGURATION_ROWS[level].label}
      description={
        <span className="flex flex-col gap-1">
          <span>{LEVEL_COPY[level].allows}</span>
          <span>
            <span className="text-foreground/80 font-medium">Risk: </span>
            {LEVEL_COPY[level].risk}
          </span>
        </span>
      }
    >
      {isDefault && <Badge variant="secondary">Default</Badge>}
    </SettingsRow>
  );
}
