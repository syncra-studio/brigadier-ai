import { type MouseEvent, useContext, useEffect, useState } from "react";

import {
  FILES_UPLOADING,
  uploading as anyUploading,
  useReadingFiles,
} from "@/app/conversation/attachments";
import { COMPOSER_EDITABLE, ComposerTargetContext } from "@/app/conversation/composerTarget";
import { DictateButton } from "@/app/conversation/DictateButton";
import { useImagesBlocked, useImagesUnsupportedToast } from "@/app/conversation/imageInput";
import { IMAGES_BLOCKED } from "@/app/conversation/imageModels";
import { askPausedSend } from "@/app/conversation/PausedSend";
import { ViewContext } from "@/app/conversation/viewContext";
import { shortcutKeys } from "@/app/shortcutCatalog";
import { SendButton, type SendState } from "@/components/controls";
import { ComposerPrimitive, useAui, useAuiState } from "@/components/conversation";
import type { RunState } from "@/ipc/generated";
import { clearBackgroundSend, markBackgroundSend, useBackgroundArmed } from "@/state/backgroundSend";
import { useBoard } from "@/state/board";
import { cancelDictation, useDictation } from "@/state/dictation";
import { NEW_CHAT_SCOPE } from "@/state/drafts";
import { useApp } from "@/state/store";
import { isTaskTab } from "@/state/tabStates";

/** A second Esc within this long stops the model (the first one arms the button). */
const ESC_WINDOW_MS = 2000;

/** Other fields that keep the keyboard on Esc. */
const TEXT_FIELD = "input, textarea, select, [contenteditable=true], [contenteditable='']";

/**
 * A two-press stop: the first Esc outside menus and cards arms the send button (it reads
 * "Esc"), a second within two seconds stops the model; Esc in the composer then also leaves the
 * field, keeping the text. With nothing running, Esc puts the cursor in the composer instead.
 * Esc while dictation (`dictationOwner`'s) downloads, starts or records cancels it, and only that.
 */
function useEscToStop(canCancel: boolean, dictationOwner: string): boolean {
  const aui = useAui();
  const { embedded } = useContext(ViewContext);
  const [armedAt, setArmedAt] = useState<number | null>(null);
  const dictation = useDictation(dictationOwner).phase.type;
  const dictating = dictation === "downloading" || dictation === "starting" || dictation === "recording";
  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented || event.isComposing) return;
      // A side chat's Esc is its own; the conversation beside it takes the rest.
      const inSideChat =
        event.target instanceof Element && event.target.closest("[data-embedded-view]") !== null;
      if (inSideChat !== embedded) return;
      // Menus, dialogs and the composer's own popovers take their Esc first.
      if (document.querySelector("[role=dialog], [role=menu], [role=listbox], [data-slot=composer-commands], [data-slot=composer-mentions]")) return;
      if (dictating) {
        cancelDictation();
        return;
      }
      const active = document.activeElement;
      const inComposer = active instanceof HTMLElement && active.matches(COMPOSER_EDITABLE);
      if (!canCancel) {
        if (inComposer || (active instanceof HTMLElement && active.matches(TEXT_FIELD))) return;
        const mine = [...document.querySelectorAll<HTMLElement>(COMPOSER_EDITABLE)].find(
          (field) => (field.closest("[data-embedded-view]") !== null) === embedded,
        );
        mine?.focus();
        return;
      }
      if (inComposer) active.blur();
      const now = event.timeStamp;
      if (armedAt !== null && now - armedAt < ESC_WINDOW_MS) {
        setArmedAt(null);
        aui.composer().cancel();
      } else {
        setArmedAt(now);
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [aui, canCancel, armedAt, embedded, dictating]);
  useEffect(() => {
    if (armedAt === null) return;
    const timer = setTimeout(() => setArmedAt(null), ESC_WINDOW_MS);
    return () => clearTimeout(timer);
  }, [armedAt]);
  return armedAt !== null && canCancel;
}

/**
 * How long Stop stays after the model stops being cancellable. A merge lands its workers one by
 * one and the conversation stops and starts working between them; without this the button
 * flipped Stop ↔ Send several times a second, each flip easing its glyph in again from nothing,
 * so it showed as a blank disc.
 */
const STOP_HOLD_MS = 700;

/** `on`, staying true for `ms` after it goes false (and false at once if it comes back on). */
function useHeldOn(on: boolean, ms: number): boolean {
  const [held, setHeld] = useState(on);
  // Set while rendering (React's way to adjust state to a prop), not in an effect.
  if (on && !held) setHeld(true);
  useEffect(() => {
    if (on) return;
    const timer = setTimeout(() => setHeld(false), ms);
    return () => clearTimeout(timer);
  }, [on, ms]);
  return on || held;
}

/**
 * Whether the model is starting for the first time in this stretch of work (`working`). A
 * session's model starts again for each worker report and landing; those starts keep Stop, so
 * the button doesn't dim to the spinner and back mid-work (#422).
 */
function useFirstStart(run: RunState | null, working: boolean): boolean {
  const [ran, setRan] = useState(false);
  // Set while rendering (React's way to adjust state to a prop), not in an effect.
  if (working && run === "running" && !ran) setRan(true);
  if (!working && ran) setRan(false);
  return run === "starting" && !ran;
}

/** What ⌘⏎ and ⌘-click on Send do in a project's session. */
const START_TIP = "Start as a new session";

const SEND_TIPS: Record<SendState, string> = {
  starting: "Starting your task…",
  armed: "Press Esc again to stop",
  stop: "Stop",
  queue: "Queue",
  resume: "Resume",
  send: "Send message",
};

/** Whether ⌘ (Ctrl off macOS) is held down, while `on`. */
function useModHeld(mac: boolean, on: boolean): boolean {
  const [held, setHeld] = useState(false);
  useEffect(() => {
    if (!on) return;
    const update = (event: KeyboardEvent) => setHeld(mac ? event.metaKey : event.ctrlKey);
    // Its key-up goes to another app once the window loses focus (⌘Tab).
    const release = () => setHeld(false);
    window.addEventListener("keydown", update);
    window.addEventListener("keyup", update);
    window.addEventListener("blur", release);
    return () => {
      window.removeEventListener("keydown", update);
      window.removeEventListener("keyup", update);
      window.removeEventListener("blur", release);
    };
  }, [mac, on]);
  return on && held;
}

/**
 * One send button, which changes with the state: ↑ to send (its tip "Queue" while the model
 * works: a Chat queues the message, a session's orchestrator sorts it; "Steer" when steering is
 * the default follow-up, Settings › General), ■ to stop, "Esc" once
 * armed, ▶ to resume a stopped request, and a grey spinner while the conversation's model
 * starts. In a project's session ⌘-click starts the message as a new session in the
 * background, as ⌘⏎ does; its arrow turns while ⌘ is held (or the New session chip is on).
 */
export function SendControls({
  running,
  onResume,
}: {
  running: boolean;
  onResume: (() => void) | null;
}) {
  const aui = useAui();
  const target = useContext(ComposerTargetContext);
  const conversationId = target?.conversation?.id ?? null;
  const run = useBoard((s) =>
    !!conversationId && s.board?.conversationId === conversationId ? s.board.run : null,
  );
  const empty = useAuiState((s) => s.composer.isEmpty);
  const cancellable = useAuiState((s) => s.composer.canCancel);
  const canCancel = useHeldOn(cancellable, STOP_HOLD_MS);
  const starting = useFirstStart(run, canCancel);
  const dictationOwner = conversationId ?? NEW_CHAT_SCOPE;
  const armed = useEscToStop(canCancel, dictationOwner);
  const steers = useApp((s) => s.settings.followUp === "steer");
  const mac = useApp((s) => s.info?.platform === "macos");
  // A project's session: ⌘⏎ and ⌘-click start a new session in the background.
  const projectSession = target?.conversation && isTaskTab(target.conversation) ? target.conversation.id : null;
  const modHeld = useModHeld(mac, projectSession !== null);
  const chip = useBackgroundArmed(projectSession);
  // Images for a model that takes none hold the message until they go or the model changes;
  // files still being read or stored hold it until they are.
  const imagesBlocked = useImagesBlocked();
  useImagesUnsupportedToast();
  const stored = useAuiState((s) => anyUploading(s.composer.attachments));
  const uploading = useReadingFiles(dictationOwner) || stored;
  const state: SendState =
    armed
      ? "armed"
      : canCancel && empty
        ? starting
          ? "starting"
          : "stop"
        : running && !empty
          ? "queue"
          : !running && empty && onResume
            ? "resume"
            : "send";
  const sending = state === "send" || state === "queue";
  // Stop is a plain button, not the Cancel primitive: held Stop (nothing to cancel) and live Stop
  // are then the same element, so a flip between them never remounts it.
  const cancelling = state === "starting" || state === "armed" || state === "stop";
  const background = sending && projectSession !== null && !empty && (modHeld || chip);
  const tip = state === "queue" && steers ? "Steer" : SEND_TIPS[state];
  const tooltip =
    sending && imagesBlocked
      ? IMAGES_BLOCKED
      : sending && uploading
        ? FILES_UPLOADING
        : background
        ? START_TIP
        : sending && projectSession
          ? `${state === "send" ? "Send" : tip} ${shortcutKeys("send", mac)} · ${START_TIP} ${shortcutKeys("start-in-tab", mac)}`
          : tip;
  const button = (
    <SendButton
      tooltip={tooltip}
      disabled={sending && (imagesBlocked || uploading)}
      state={state}
      background={background}
      onClick={
        state === "resume"
          ? (onResume ?? undefined)
          : cancelling
            ? () => {
                if (cancellable) aui.composer().cancel();
              }
          : (event: MouseEvent<HTMLButtonElement>) => {
              const toNewSession = sending && !empty && (mac ? event.metaKey : event.ctrlKey);
              // Into a paused queue it asks first; the Send primitive's own click doesn't send.
              if (sending && !empty && !(projectSession && toNewSession) && askPausedSend(conversationId, () => aui.composer().send())) {
                event.preventDefault();
                return;
              }
              if (!projectSession) return;
              // A ⌘-click whose send never came is not this send's.
              clearBackgroundSend(projectSession);
              // Before the Send primitive's own click sends: this send goes to a new session.
              if (toNewSession) markBackgroundSend(projectSession);
            }
      }
    />
  );
  return (
    <div className="flex shrink-0 items-center gap-1">
      <DictateButton owner={dictationOwner} />
      {state === "resume" || cancelling ? button : <ComposerPrimitive.Send asChild>{button}</ComposerPrimitive.Send>}
    </div>
  );
}
