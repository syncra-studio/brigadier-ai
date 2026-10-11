import type { ComposerKeys } from "@/app/shortcutCatalog";
import type { FollowUp } from "@/ipc/generated";
import type { SendLane } from "@/state/actions";

type EnterPress = Pick<KeyboardEvent, "ctrlKey" | "metaKey" | "shiftKey" | "altKey">;

/**
 * What Enter does in the composer with the send shortcut chosen (Settings › General ›
 * Composer): send the message, send it the follow-up's other way (steer instead of queue, or
 * the reverse), or nothing of ours (the editor starts a new line). ⌘ is Ctrl off macOS.
 */
export function enterAction(
  event: EnterPress,
  mac: boolean,
  sendShortcut: ComposerKeys["sendShortcut"],
): "send" | "other" | null {
  const mod = mac ? event.metaKey : event.ctrlKey;
  // The other platform modifier (Ctrl on macOS, the Windows key elsewhere) and Alt aren't ours.
  const stray = event.altKey || (mac ? event.ctrlKey : event.metaKey);
  if (stray) return null;
  if (sendShortcut === "enter") {
    if (event.shiftKey) return null;
    return mod ? "other" : "send";
  }
  if (!mod) return null;
  return event.shiftKey ? "other" : "send";
}

/**
 * Where a message the composer sends goes. `asked` is what the composer asked for: `followUp`
 * when sent while an answer works without saying how (it goes the default way), `steer` or
 * `auto` when the other key said so. A queued message pulled out to edit goes back to its slot
 * unless steered in explicitly.
 */
export function sendLane(
  asked: SendLane | "followUp",
  { pulled, followUp }: { pulled: boolean; followUp: FollowUp },
): SendLane {
  if (pulled && (asked === "auto" || asked === "followUp")) return "queue";
  if (asked === "followUp") return followUp === "steer" ? "steer" : "auto";
  return asked;
}
