import { useEffect } from "react";

import { useAction } from "@/app/conversation/useAction";
import { SettingsPage, SettingsSection, SettingsSwitch } from "@/app/settings/parts";
import { Badge, Button } from "@/components/controls";
import { SettingsCard, SettingsRow } from "@/components/shared";
import type { ComputerPermission, ComputerUseStatus, PermissionState } from "@/ipc/generated";
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
    ? [rows.enable, rows.menuBar, rows.accessibility, rows.screenRecording, rows.setup]
    : [rows.enable, rows.desktop, rows.setup];
}

/** Whether this system lists the page: macOS, Windows and Linux, from the app's own info. */
export function computerUseAvailable(): boolean {
  const platform = useApp.getState().info?.platform;
  return platform === "macos" || platform === "windows" || platform === "linux";
}

/**
 * Settings › Computer use: whether sessions, Chats and workers may use apps on this Mac, the
 * menu-bar item, the two macOS permissions the helper needs, and where setup stands. Showing it
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
  const description = notes.length > 0 ? notes.join(" ") : "Shown once an agent has used apps here.";
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
