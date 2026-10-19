import type { Conversation, PermissionLevel, Settings, SettingChange } from "@/ipc/generated";
import { type SettingsPageLink } from "@/state/actions";
import { setSetting } from "@/state/settings";

/**
 * A settings change the thread suggested on a card (Q90's `suggest_setting`): what it reads,
 * whether it is made already, and the one press that makes it.
 */

/** The card's words, `labels` the permission levels' names: its title, its button and the line once it is made. */
export function settingWords(change: SettingChange, labels: Record<PermissionLevel, string>): { title: string; button: string; done: string } {
  switch (change.setting) {
    case "permission": {
      const label = labels[change.level];
      return { title: `Switch this session to ${label}`, button: `Switch to ${label}`, done: `This session is on ${label}.` };
    }
    case "browser":
      return change.on
        ? { title: "Turn on the built-in browser", button: "Turn on", done: "The built-in browser is on." }
        : { title: "Turn off the built-in browser", button: "Turn off", done: "The built-in browser is off." };
    case "mergeOnOwn":
      return change.on
        ? { title: "Merge on its own when the work is ready", button: "Turn on", done: "Brigadier merges on its own." }
        : { title: "Stop merging on its own", button: "Turn off", done: "You merge when the work is ready." };
  }
}

/** The Settings page the change lives on. */
export function settingPage(change: SettingChange): SettingsPageLink {
  switch (change.setting) {
    case "permission": return "configuration";
    case "browser": return "browser";
    case "mergeOnOwn": return "git";
  }
}

/** The session's permission level, none for a Chat. */
function permissionOf(conversation: Conversation | null): PermissionLevel | null {
  return conversation?.setup?.type === "session" ? conversation.setup.permission : null;
}

/** Whether the change is made already, by the user's press or elsewhere. */
export function isMade(change: SettingChange, settings: Settings, conversation: Conversation | null): boolean {
  switch (change.setting) {
    case "permission": return permissionOf(conversation) === change.level;
    case "browser": return settings.browserEnabled === change.on;
    case "mergeOnOwn": return settings.autoMerge === change.on;
  }
}

/** Makes the change: the user pressed the card's button. */
export async function makeChange(
  change: SettingChange,
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
    case "mergeOnOwn":
      await setSetting("autoMerge", change.on);
      return;
  }
}
