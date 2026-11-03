import { useEffect } from "react";

import { useAction } from "@/app/conversation/useAction";
import { SettingsPage, SettingsSection, SettingsSwitch } from "@/app/settings/parts";
import { TargetPicker, useListedModels } from "@/app/settings/routing/pickers";
import { refusingRule, rowChoice, ruleNames, ruleTitle } from "@/app/settings/routing/ruleWords";
import { rowModelName, targetModel, withFirst, withOrder, workRows } from "@/app/settings/routing/words";
import { Badge, Button, SettingsSelectTrigger } from "@/components/controls";
import { ProviderGlyph } from "@/components/icons";
import { effortLabel, SettingsCard, SettingsRow } from "@/components/shared";
import type { ComputerPermission, ComputerUseStatus, OverrideTarget, PermissionState } from "@/ipc/generated";
import { useModelGroups } from "@/lib/setup";
import {
  checkUnreadOnce,
  grantComputerPermission,
  openComputerSetup,
  openPrivacySettings,
  permissionName,
  permissionState,
  PERMISSIONS,
  setupLine,
  setupUnfinished,
  thisComputer,
  useCheckingUnread,
  useComputerUse,
} from "@/state/computerUse";
import { saveModelRules, useModelRules, useModelRulesRead } from "@/state/modelRules";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

/** Whether this computer is a Mac, from the app's own info. */
function onMac(): boolean {
  return useApp.getState().info?.platform === "macos";
}

/** The Computer use page's rows, for the page and for Settings search. */
export const COMPUTER_USE_ROWS = {
  enable: {
    get label() {
      return onMac() ? "Let sessions use apps on this Mac" : "Let sessions use apps on this computer";
    },
    get description() {
      return onMac()
        ? "Sessions, Chats and their workers can see and control apps in the background. The first time one needs this, setup starts by itself."
        : "Sessions, Chats and their workers can see and control apps in the background.";
    },
  },
  model: { label: "Model for computer use", description: "Which model a worker uses when it works in apps. Also in Settings › Routing." },
  menuBar: { label: "Show in menu bar", description: "See which sessions are using apps, and stop them" },
  accessibility: { label: "Accessibility", description: "Read what's in app windows and use their controls" },
  screenRecording: { label: "Screen Recording", description: "See app windows, to check what happened" },
  setup: { label: "Setup", description: "Where setting up computer use stands" },
  desktop: { label: "This computer", description: "What agents can and can't do in this desktop session" },
} as const;

/** The rows this computer's page shows: macOS asks for permissions and has a menu-bar item. */
export function computerUseRows() {
  const rows = COMPUTER_USE_ROWS;
  return onMac()
    ? [rows.enable, rows.model, rows.menuBar, rows.accessibility, rows.screenRecording, rows.setup]
    : [rows.enable, rows.model, rows.desktop, rows.setup];
}

/** Whether this system lists the page: macOS, Windows and Linux, from the app's own info. */
export function computerUseAvailable(): boolean {
  const platform = useApp.getState().info?.platform;
  return platform === "macos" || platform === "windows" || platform === "linux";
}

/**
 * Settings › Computer use: whether sessions, Chats and workers may use apps on this Mac, the
 * model a worker uses for it, the menu-bar item, the two macOS permissions the helper needs, and where setup stands. Showing it
 * starts nothing, but for one check of permissions nothing has read yet while computer use is on.
 */
export function ComputerUsePage() {
  const status = useComputerUse();
  const checking = useFirstCheck(status);
  const mac = useApp((s) => s.info?.platform === "macos");
  if (!mac) {
    return (
      <SettingsPage title="Computer use">
        <SettingsSection title="Enable">
          <SettingsCard>
            <EnableRow />
          </SettingsCard>
        </SettingsSection>
        <SettingsSection title="Model">
          <SettingsCard>
            <ModelRow />
          </SettingsCard>
        </SettingsSection>
        <SettingsSection title="This computer">
          <SettingsCard>
            <DesktopRow status={status} />
          </SettingsCard>
        </SettingsSection>
        <SettingsSection title="Setup">
          <SettingsCard>
            <SetupRow status={status} checking={false} />
          </SettingsCard>
        </SettingsSection>
      </SettingsPage>
    );
  }
  return (
    <SettingsPage title="Computer use">
      <SettingsSection title="Enable">
        <SettingsCard>
          <EnableRow />
        </SettingsCard>
      </SettingsSection>
      <SettingsSection title="Model">
        <SettingsCard>
          <ModelRow />
        </SettingsCard>
      </SettingsSection>
      <SettingsSection title="Menu bar">
        <SettingsCard>
          <MenuBarRow />
        </SettingsCard>
      </SettingsSection>
      <SettingsSection
        title="Permissions"
        description={status && !status.current && known(status) ? "As the helper last saw them." : undefined}
      >
        <SettingsCard>
          {PERMISSIONS.map((permission) => (
            <PermissionRow key={permission} permission={permission} status={status} checking={checking} />
          ))}
        </SettingsCard>
      </SettingsSection>
      <SettingsSection title="Setup">
        <SettingsCard>
          <SetupRow status={status} checking={checking} />
        </SettingsCard>
      </SettingsSection>
    </SettingsPage>
  );
}

function known(status: ComputerUseStatus): boolean {
  return status.accessibility !== "unknown" || status.screenRecording !== "unknown";
}

/**
 * With computer use on and the permissions never read (no helper has run for this data folder),
 * the page asks the helper once, so it shows what macOS granted it: true while it asks.
 */
function useFirstCheck(status: ComputerUseStatus | null): boolean {
  const enabled = useApp((s) => s.settings.computerUse);
  const unread = enabled && status !== null && !known(status) && status.setup.state === "notSetUp";
  const checking = useCheckingUnread();
  useEffect(() => {
    if (unread) checkUnreadOnce();
  }, [unread]);
  return checking && unread;
}

function EnableRow() {
  const on = useApp((s) => s.settings.computerUse);
  const save = useAction();
  const row = COMPUTER_USE_ROWS.enable;
  return (
    <SettingsRow label={row.label} description={row.description} error={save.error}>
      <SettingsSwitch label={row.label} checked={on} onCheckedChange={(next) => save.run(() => setSetting("computerUse", next))} />
    </SettingsRow>
  );
}

/** The kind of work in the rules file a worker with the computer tools is: Routing's row of the same name. */
const COMPUTER_USE_WORK = "Computer use";

/**
 * The model for computer use: the "Computer use" row of the rules file, the same Settings ›
 * Routing shows. A pick goes first in its order (the shipped one, until you have your own),
 * the rest after it; nothing is picked until the file is read, so a save never writes over it.
 */
function ModelRow() {
  useModelRulesRead();
  const rules = useModelRules((s) => s.view);
  const error = useModelRules((s) => s.error);
  const groups = useModelGroups();
  const models = useListedModels();
  const accounts = useApp((s) => s.accounts?.accounts);
  const save = useAction();
  const row = COMPUTER_USE_ROWS.model;
  const work = workRows(rules).find((found) => found.work === COMPUTER_USE_WORK);
  // The daemon skips a CLI whose lead account isn't signed in (including extra accounts).
  const now = rules && work?.rows.find((found) =>
    accounts?.some((account) => account.account.provider === found.agent && account.inUse && account.status?.loggedIn) &&
    !refusingRule(rules.rules, rowChoice(found, models), found.work));
  const pick = (target: OverrideTarget) => {
    if (!rules || !work) return;
    const picked = { work: work.work, agent: target.provider, model: targetModel(target), effort: null, reason: "Your choice." };
    save.run(async () => {
      const order = withFirst(work.rows, picked);
      const refusal = refusingRule(rules.rules, rowChoice(order[0]!, models), work.work);
      if (refusal) throw new Error(`Refused by your rule “${ruleTitle(refusal, ruleNames(groups))}”`);
      await saveModelRules(withOrder(rules.rows, work.work, order), rules.rules);
    });
  };
  return (
    <SettingsRow label={row.label} description={row.description} error={save.error ?? (rules ? null : error)}>
      <TargetPicker label={row.label} onPick={pick} align="end" {...(now && { opensOn: now.agent })}>
        <SettingsSelectTrigger
          label={row.label}
          data-slot="computer-use-model"
          disabled={!work || save.busy}
          placeholder={!now}
          icon={now && <ProviderGlyph provider={now.agent} className="size-3.5 shrink-0" />}
        >
          {now ? (
            <>
              {rowModelName(groups, now.agent, now.model)}
              {now.effort && <span className="text-muted-foreground"> · {effortLabel(now.effort)}</span>}
            </>
          ) : !rules ? (error ? "Couldn't read" : "Loading…") : "Pick a model"}
        </SettingsSelectTrigger>
      </TargetPicker>
    </SettingsRow>
  );
}

function MenuBarRow() {
  const enabled = useApp((s) => s.settings.computerUse);
  const on = useApp((s) => s.settings.computerUseMenuBar);
  const save = useAction();
  const row = COMPUTER_USE_ROWS.menuBar;
  return (
    <SettingsRow label={row.label} description={row.description} error={save.error}>
      <SettingsSwitch
        label={row.label}
        checked={on}
        disabled={!enabled}
        onCheckedChange={(next) => save.run(() => setSetting("computerUseMenuBar", next))}
      />
    </SettingsRow>
  );
}

const STATE_LABEL: Record<PermissionState, string> = {
  granted: "Granted",
  notGranted: "Not granted",
  unknown: "Unknown",
};

function PermissionRow({ permission, status, checking }: { permission: ComputerPermission; status: ComputerUseStatus | null; checking: boolean }) {
  const enabled = useApp((s) => s.settings.computerUse);
  const grant = useAction();
  const open = useAction();
  const row = COMPUTER_USE_ROWS[permission];
  const state = status ? permissionState(status, permission) : "unknown";
  return (
    <SettingsRow label={row.label} description={row.description} error={grant.error ?? open.error}>
      <Badge data-slot="permission-state" variant={state === "granted" ? "success" : state === "notGranted" ? "warning" : "secondary"}>
        {checking ? "Checking…" : STATE_LABEL[state]}
      </Badge>
      {state !== "granted" && !checking && (
        <Button variant="soft" size="sm" disabled={!enabled || grant.busy} onClick={() => grant.run(() => grantComputerPermission(permission))}>
          Grant…
        </Button>
      )}
      <Button
        variant="soft"
        size="sm"
        aria-label={`Open System Settings for ${permissionName(permission)}`}
        onClick={() => open.run(() => openPrivacySettings(permission))}
      >
        Open System Settings
      </Button>
    </SettingsRow>
  );
}

/**
 * Windows and Linux: what the desktop session lets agents do, as the helper last said (on Linux,
 * whether it is X11 or Wayland). Nothing to grant.
 */
function DesktopRow({ status }: { status: ComputerUseStatus | null }) {
  const row = COMPUTER_USE_ROWS.desktop;
  const notes = status?.notes ?? [];
  const description = notes.length > 0 ? notes.join(" ")
    : status?.current && status.setup.state === "ready" ? "Agents can use apps on this computer."
    : "Desktop details are available while the helper is running.";
  return <SettingsRow label={row.label} description={<span data-slot="desktop-notes">{description}</span>} />;
}

function SetupRow({ status, checking }: { status: ComputerUseStatus | null; checking: boolean }) {
  const enabled = useApp((s) => s.settings.computerUse);
  const platform = useApp((s) => s.info?.platform);
  const open = useAction();
  const here = thisComputer(platform === "macos" || platform === "windows" || platform === "linux" ? platform : undefined);
  const line = status && !checking ? setupLine(status) : enabled ? "Checking…" : `Off. Agents can't use apps on ${here}.`;
  return (
    <SettingsRow label={COMPUTER_USE_ROWS.setup.label} setting="Setup" description={<span data-slot="setup-line">{line}</span>} error={open.error}>
      {status && enabled && !checking && setupUnfinished(status) && (
        <Button variant="soft" size="sm" onClick={() => open.run(() => openComputerSetup())}>Finish setup…</Button>
      )}
    </SettingsRow>
  );
}
