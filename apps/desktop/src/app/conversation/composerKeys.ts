import type { ComposerKeys } from "@/app/shortcutCatalog";
import type { FollowUp } from "@/ipc/generated";
import type { SendLane } from "@/state/actions";

type EnterPress = Pick<KeyboardEvent, "ctrlKey" | "metaKey" | "shiftKey" | "altKey">;

/**
 * What Enter does in the composer with the send shortcut chosen (Settings › General ›
 * Composer): send the message, send it the follow-up's other way (steer instead of queue, or
 * the reverse), start it as a new background session, or nothing of ours (the editor starts a
 * new line). ⌘ is Ctrl off macOS. ⌥⏎ is the other way, and in a project's session (`tabs`) ⌘⏎
 * starts a background session; with Send on ⌘⏎ they are ⌥⌘⏎ and ⇧⌘⏎. Outside a project's
 * session ⌘⏎ (⇧⌘⏎) is the other way too.
 */
export function enterAction(
  event: EnterPress,
  mac: boolean,
  sendShortcut: ComposerKeys["sendShortcut"],
  tabs = false,
): "send" | "other" | "background" | null {
  const mod = mac ? event.metaKey : event.ctrlKey;
  // The other platform modifier (Ctrl on macOS, the Windows key elsewhere) isn't ours.
  if (mac ? event.ctrlKey : event.metaKey) return null;
  if (event.altKey && event.shiftKey) return null;
  if (sendShortcut === "enter") {
    if (event.shiftKey) return null;
    if (event.altKey) return mod ? null : "other";
    return mod ? (tabs ? "background" : "other") : "send";
  }
  if (!mod) return null;
  if (event.altKey) return "other";
  return event.shiftKey ? (tabs ? "background" : "other") : "send";
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
