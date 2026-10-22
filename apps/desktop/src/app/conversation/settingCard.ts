import type { Conversation, PermissionLevel, Settings, SettingChange } from "@/ipc/generated";
import { type SettingsPageLink } from "@/state/actions";
import { setSetting } from "@/state/settings";

/**
 * A settings change the thread suggested on a card (Q90's `suggest_setting`): what it reads,
 * whether it is made already, and the one press that makes it.
 */

/**
 * A change the card shows. One to merge on its own came from an older Brigadier: that setting
 * is gone, and its suggestion shows nothing.
 */
export type ShownSettingChange = Exclude<SettingChange, { setting: "mergeOnOwn" }>;

/** Whether the thread's suggestion has a card. */
export const shownChange = (change: SettingChange): change is ShownSettingChange => change.setting !== "mergeOnOwn";

/** The card's words, `labels` the permission levels' names: its title, its button and the line once it is made. */
export function settingWords(change: ShownSettingChange, labels: Record<PermissionLevel, string>): { title: string; button: string; done: string } {
  switch (change.setting) {
    case "permission": {
      const label = labels[change.level];
      return { title: `Switch this session to ${label}`, button: `Switch to ${label}`, done: `This session is on ${label}.` };
    }
    case "browser":
      return change.on
        ? { title: "Turn on the built-in browser", button: "Turn on", done: "The built-in browser is on." }
        : { title: "Turn off the built-in browser", button: "Turn off", done: "The built-in browser is off." };
  }
}

/** The Settings page the change lives on. */
export function settingPage(change: ShownSettingChange): SettingsPageLink {
  switch (change.setting) {
    case "permission": return "configuration";
    case "browser": return "browser";
  }
}

/** The session's permission level, none for a Chat. */
function permissionOf(conversation: Conversation | null): PermissionLevel | null {
  return conversation?.setup?.type === "session" ? conversation.setup.permission : null;
}

/** Whether the change is made already, by the user's press or elsewhere. */
export function isMade(change: ShownSettingChange, settings: Settings, conversation: Conversation | null): boolean {
  switch (change.setting) {
    case "permission": return permissionOf(conversation) === change.level;
    case "browser": return settings.browserEnabled === change.on;
  }
}

/** Makes the change: the user pressed the card's button. */
export async function makeChange(
  change: ShownSettingChange,
  conversation: Conversation | null,
  setPermission: (conversation: Conversation, level: PermissionLevel) => Promise<void>,
): Promise<void> {
  switch (change.setting) {
    case "permission":
      if (conversation) await setPermission(conversation, change.level);
      return;
    case "browser":
      await setSetting("browserEnabled", change.on);
      return;
  }
}
