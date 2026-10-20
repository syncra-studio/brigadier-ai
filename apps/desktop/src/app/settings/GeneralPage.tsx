import { useCallback, useEffect, useId, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { LicensesDialog } from "@/app/settings/LicensesDialog";
import { SettingsPage, SettingsRow, SettingsSection, SettingsSelect, SettingsSwitch, SwitchSetting } from "@/app/settings/parts";
import { SidebarClosedChoice } from "@/app/settings/SidebarClosedChoice";
import { Button, Input, Segmented } from "@/components/controls";
import { SettingsCard } from "@/components/shared";
import { useSidebar } from "@/components/layout";
import {
  type LoginItem,
  loginItem,
  openLoginItems,
  openNotificationSettings,
  setLoginItem,
} from "@/ipc/client";
import type { FollowUp, NotifyFinished } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import {
  keepAwakeOptions,
  lidClosedHint,
  setKeepAwake,
  setKeepAwakeLidClosed,
  useKeepAwake,
} from "@/state/keepAwake";
import { askNotifications, useNotificationPermission } from "@/state/notifications";
import { reopenOnboarding } from "@/state/onboarding";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

/** The General page's rows, for the page and for Settings search. */
export const GENERAL_ROWS = {
  name: {
    label: "Your name",
    description: "What Brigadier calls you",
  },
  sidebarClosed: {
    label: "When the sidebar is closed",
    description: "Keep a strip of icons at the edge, or hide it completely",
  },
  followUp: {
    label: "Follow-up behaviour",
    description: "What a message sent while an agent works does",
  },
  plainText: {
    label: "Plain text composer",
    description: "Keep code, Markdown and links as literal text",
  },
  contextUsage: {
    label: "Show context window usage",
    description: "A ring by the model picker shows how full the model's context is",
  },
  notifyFinished: {
    label: "Notify when a session finishes",
    description: "A notification when a chat or session is done",
  },
  needsYou: {
    label: "When something needs you",
    description: "Approvals and questions waiting for your answer",
  },
  keepAwake: {
    label: "Keep the computer awake",
    description: "Whether this computer may sleep while Brigadier is open",
  },
  lidClosed: {
    label: "Keep going with the lid closed",
    description: "Applied whenever the computer is kept awake",
  },
  hibernate: {
    label: "Hibernate after",
    description: "An idle chat pauses to free memory and picks up where it left off",
  },
  openAtLogin: {
    label: "Open at login",
    description: "Start Brigadier when you log in",
  },
  version: {
    label: "Version",
    description: "The Brigadier you're running",
  },
  licenses: {
    label: "Open source licenses",
    description: "Open source code Brigadier adapts or uses",
  },
  setup: {
    label: "Run setup again",
    description: "Sign in to agents, pick projects and choose defaults again",
  },
} as const;

const FOLLOW_UPS = [
  { value: "queue", label: "Queue" },
  { value: "steer", label: "Steer" },
] as const satisfies readonly { value: FollowUp; label: string }[];

const NOTIFY_FINISHED = [
  {
    value: "background",
    label: "In the background",
    hint: "Only while Brigadier isn't the app in front",
  },
  { value: "always", label: "Always", hint: "Also while you're in Brigadier" },
  { value: "never", label: "Never", hint: "No notification when work is done" },
] as const satisfies readonly { value: NotifyFinished; label: string; hint: string }[];

export function GeneralPage() {
  return (
    <SettingsPage title="General">
      <AppearanceSection />
      <ComposerSection />
      <NotificationsSection />
      <PowerSection />
      <AboutSection />
    </SettingsPage>
  );
}

function AppearanceSection() {
  const { collapseMode, setCollapseMode } = useSidebar();
  return (
    <SettingsSection title="Appearance">
      <SettingsCard>
        <NameRow />
        <SettingsRow
          label={GENERAL_ROWS.sidebarClosed.label}
          description={GENERAL_ROWS.sidebarClosed.description}
        >
          <SidebarClosedChoice
            label={GENERAL_ROWS.sidebarClosed.label}
            value={collapseMode}
            onChange={setCollapseMode}
          />
        </SettingsRow>
      </SettingsCard>
    </SettingsSection>
  );
}

/** The name the app calls the user by: saved as it's left or on Enter; Escape undoes the typing. */
function NameRow() {
  const id = useId();
  const name = useApp((s) => s.settings.userName);
  // What is typed, while it differs from the setting.
  const [typed, setTyped] = useState<string | null>(null);
  const save = useAction();

  const commit = () => {
    if (typed === null) return;
    const value = typed.trim();
    setTyped(null);
    if (value !== name) save.run(() => setSetting("userName", value));
  };

  return (
    <SettingsRow
      label={GENERAL_ROWS.name.label}
      description={GENERAL_ROWS.name.description}
      htmlFor={id}
      error={save.error}
    >
      <Input
        id={id}
        autoComplete="given-name"
        maxLength={80}
        placeholder="Your name"
        value={typed ?? name}
        className="border-input bg-popover h-control-md rounded-control text-label w-48 px-2.5"
        onChange={(event) => setTyped(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") commit();
          if (event.key === "Escape") {
            // Esc here undoes the typing rather than leaving Settings.
            event.stopPropagation();
            setTyped(null);
          }
        }}
      />
    </SettingsRow>
  );
}

function ComposerSection() {
  const mac = useApp((s) => s.info?.platform === "macos");
  const followUp = useApp((s) => s.settings.followUp);
  const followUpAction = useAction();
  // The key that sends the other way for one message.
  const other = mac ? "⌥Enter" : "Alt+Enter";
  return (
    <SettingsSection title="Composer">
      <SettingsCard>
        <SettingsRow
          label={GENERAL_ROWS.followUp.label}
          description={`${GENERAL_ROWS.followUp.description}; ${other} does the other for one message`}
          error={followUpAction.error}
        >
          <Segmented
            label={GENERAL_ROWS.followUp.label}
            value={followUp}
            options={FOLLOW_UPS}
            onChange={(value) => followUpAction.run(() => setSetting("followUp", value))}
          />
        </SettingsRow>
        <SwitchSetting setting="plainTextComposer" row={GENERAL_ROWS.plainText} />
        <SwitchSetting setting="showContextUsage" row={GENERAL_ROWS.contextUsage} />
      </SettingsCard>
    </SettingsSection>
  );
}

function NotificationsSection() {
  const permission = useNotificationPermission(true);
  const notifyFinished = useApp((s) => s.settings.notifyFinished);
  const needsYou = useApp((s) => s.settings.notifyNeedsYou);
  const statusAction = useAction();
  const finishedAction = useAction();
  const needsYouAction = useAction();
  // The rows stay usable while notifications are off, dimmed: they apply once they're on.
  const off = permission === "off" || permission === "notAsked";
  return (
    <SettingsSection title="Notifications">
      <SettingsCard>
        {permission === "off" && (
          <SettingsRow
            label="Notifications are off"
            description="Turn them on in System Settings to use the options below"
            error={statusAction.error}
          >
            <Button variant="soft" size="sm" onClick={() => statusAction.run(() => openNotificationSettings())}>
              Open System Settings
            </Button>
          </SettingsRow>
        )}
        {permission === "notAsked" && (
          <SettingsRow
            label="Notifications aren't on yet"
            description="Allow them to hear when work is done or needs you"
            error={statusAction.error}
          >
            <Button variant="soft" size="sm" onClick={() => statusAction.run(() => askNotifications())}>
              Allow
            </Button>
          </SettingsRow>
        )}
        <SettingsRow
          label={GENERAL_ROWS.notifyFinished.label}
          description={GENERAL_ROWS.notifyFinished.description}
          error={finishedAction.error}
          className={cn(off && "opacity-50")}
        >
          <SettingsSelect
            label={GENERAL_ROWS.notifyFinished.label}
            value={notifyFinished}
            options={NOTIFY_FINISHED}
            onChange={(value) => finishedAction.run(() => setSetting("notifyFinished", value))}
          />
        </SettingsRow>
        <SettingsRow
          label={GENERAL_ROWS.needsYou.label}
          description={GENERAL_ROWS.needsYou.description}
          error={needsYouAction.error}
          className={cn(off && "opacity-50")}
        >
          <SettingsSwitch
            label={GENERAL_ROWS.needsYou.label}
            checked={needsYou}
            onCheckedChange={(on) => needsYouAction.run(() => setSetting("notifyNeedsYou", on))}
          />
        </SettingsRow>
      </SettingsCard>
    </SettingsSection>
  );
}

function PowerSection() {
  const keepAwake = useApp((s) => s.settings.keepAwake);
  const lidClosed = useApp((s) => s.settings.keepAwakeLidClosed);
  const status = useKeepAwake((s) => s.status);
  const settingUp = useKeepAwake((s) => s.settingUp);
  const keepAwakeAction = useAction();
  const lidAction = useAction();
  const options = keepAwakeOptions(status?.screenOn ?? false);
  return (
    <SettingsSection title="Power">
      <SettingsCard>
        <SettingsRow
          label={GENERAL_ROWS.keepAwake.label}
          description={
            status?.forRun
              ? "An overnight run keeps the computer awake, screen on, until it ends; then this applies again"
              : GENERAL_ROWS.keepAwake.description
          }
          error={keepAwakeAction.error}
        >
          <SettingsSelect
            label={GENERAL_ROWS.keepAwake.label}
            value={keepAwake}
            options={options}
            onChange={(value) => keepAwakeAction.run(() => setKeepAwake(value))}
          />
        </SettingsRow>
        {status?.lidClosed !== "unsupported" && (
          <SettingsRow
            label={GENERAL_ROWS.lidClosed.label}
            description={lidClosedHint(status, settingUp)}
            error={lidAction.error}
          >
            <SettingsSwitch
              label={GENERAL_ROWS.lidClosed.label}
              checked={lidClosed}
              disabled={settingUp}
              onCheckedChange={(on) => lidAction.run(() => setKeepAwakeLidClosed(on))}
            />
          </SettingsRow>
        )}
        <HibernateRow />
        <OpenAtLoginRow />
      </SettingsCard>
    </SettingsSection>
  );
}

/** Open at login as last read, kept while the app runs. */
let lastLoginItem: LoginItem | null = null;

/**
 * Open at login, as the system has it: read when the page opens and whenever the window comes
 * back to the front (the user may have allowed or removed it in the system's settings). Not
 * shown where there is no way (a development build on macOS), once that is known. Waiting for the user's approval
 * reads as on, so turning it off takes the request back.
 */
function OpenAtLoginRow() {
  // As last read, at once; read again as the page opens.
  const [state, setState] = useState<LoginItem | null>(() => lastLoginItem);
  const save = useAction();
  const load = useCallback(() => {
    loginItem()
      .then((read) => {
        lastLoginItem = read;
        setState(read);
      })
      .catch((error: unknown) => console.error("reading open at login failed", error));
  }, []);
  useEffect(() => {
    load();
    window.addEventListener("focus", load);
    return () => window.removeEventListener("focus", load);
  }, [load]);
  if (state === "unsupported") return null;
  return (
    <SettingsRow
      label={GENERAL_ROWS.openAtLogin.label}
      description={
        state === "needsApproval"
          ? "Allow Brigadier in System Settings › Login Items to finish turning this on"
          : GENERAL_ROWS.openAtLogin.description
      }
      error={save.error}
    >
      {state === "needsApproval" && (
        <Button variant="soft" size="sm" onClick={() => save.run(() => openLoginItems())}>
          Open System Settings
        </Button>
      )}
      <SettingsSwitch
        label={GENERAL_ROWS.openAtLogin.label}
        checked={state === "on" || state === "needsApproval"}
        // Until the first read answers (a moment), the switch waits where it stands.
        disabled={state === null}
        onCheckedChange={(on) =>
          save.run(async () => {
            const next = await setLoginItem(on);
            lastLoginItem = next;
            setState(next);
          })
        }
      />
    </SettingsRow>
  );
}

function AboutSection() {
  const version = useApp((s) => s.info?.version);
  const [licenses, setLicenses] = useState(false);
  return (
    <SettingsSection title="About">
      <SettingsCard>
        <SettingsRow label={GENERAL_ROWS.version.label} description={GENERAL_ROWS.version.description}>
          <span className="text-muted-foreground text-label tabular-nums">
            {version ? `Brigadier ${version}` : "Brigadier"}
          </span>
        </SettingsRow>
        <SettingsRow label={GENERAL_ROWS.licenses.label} description={GENERAL_ROWS.licenses.description}>
          <Button variant="soft" size="sm" onClick={() => setLicenses(true)}>View</Button>
        </SettingsRow>
        <SettingsRow label={GENERAL_ROWS.setup.label} description={GENERAL_ROWS.setup.description}>
          <Button variant="soft" size="sm" onClick={() => reopenOnboarding()}>Run setup</Button>
        </SettingsRow>
      </SettingsCard>
      <LicensesDialog open={licenses} onOpenChange={setLicenses} />
    </SettingsSection>
  );
}

/** The range Hibernate after accepts: a minute to a week. */
const HIBERNATE_MINUTES = { min: 1, max: 10_080 } as const;

/** Minutes before an idle conversation hibernates: saved when the field is left or on Enter. */
function HibernateRow() {
  const id = useId();
  const minutes = useApp((s) => s.settings.hibernateAfterMinutes);
  // What is typed, while it differs from the setting; else the setting (which a change from
  // elsewhere, or a failed save going back, updates).
  const [typed, setTyped] = useState<string | null>(null);
  const [invalid, setInvalid] = useState(false);
  const save = useAction();
  const text = typed ?? String(minutes);

  const commit = () => {
    if (typed === null) return;
    const value = Number(typed);
    if (
      !Number.isInteger(value) ||
      value < HIBERNATE_MINUTES.min ||
      value > HIBERNATE_MINUTES.max
    ) {
      setInvalid(true);
      return;
    }
    setInvalid(false);
    setTyped(null);
    if (value !== minutes) save.run(() => setSetting("hibernateAfterMinutes", value));
  };

  return (
    <SettingsRow
      label={GENERAL_ROWS.hibernate.label}
      description={GENERAL_ROWS.hibernate.description}
      htmlFor={id}
      error={invalid ? "A whole number of minutes, from 1 to 10,080 (a week)" : save.error}
    >
      <Input
        id={id}
        type="number"
        inputMode="numeric"
        min={HIBERNATE_MINUTES.min}
        max={HIBERNATE_MINUTES.max}
        step={1}
        value={text}
        aria-invalid={invalid || undefined}
        className="border-input bg-popover h-control-md rounded-control text-label w-18 [appearance:textfield] px-2.5 tabular-nums [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none"
        onChange={(event) => setTyped(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") commit();
          if (event.key === "Escape") {
            // Esc here undoes the typing rather than leaving Settings.
            event.stopPropagation();
            setTyped(null);
            setInvalid(false);
          }
        }}
      />
      <span className="text-muted-foreground text-label">min</span>
    </SettingsRow>
  );
}
