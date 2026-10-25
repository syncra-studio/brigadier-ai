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
import { sendButtonState } from "@/app/conversation/sendButtonState";
import { ViewContext } from "@/app/conversation/viewContext";
import { shortcutKeys } from "@/app/shortcutCatalog";
import { SendButton } from "@/components/controls";
import { useComposerSend, useAui, useAuiState } from "@/components/conversation";
import { clearBackgroundSend, markBackgroundSend, useBackgroundArmed } from "@/state/backgroundSend";
import { useWorkerName } from "@/app/conversation/WorkerChip";
import { cancelDictation, useDictation } from "@/state/dictation";
import { useDirectSending, useDirectTo } from "@/state/directTo";
import { NEW_CHAT_SCOPE } from "@/state/drafts";
import { turnStopping, useStopping } from "@/state/stopping";
import { useApp } from "@/state/store";
import { isTaskTab } from "@/state/tabStates";

/** A second Esc within this long stops the model (the first one arms the button). */
const ESC_WINDOW_MS = 2000;

/** Other fields that keep the keyboard on Esc. */
const TEXT_FIELD = "input, textarea, select, [contenteditable=true], [contenteditable='']";

/**
 * A two-press stop: the first Esc outside menus and cards arms Stop, with a reminder in its
 * tooltip. A second within two seconds stops the model; Esc also leaves the composer field,
 * keeping its text. With nothing running, Esc puts the cursor in the composer instead.
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
 * flipped Stop ↔ Send several times a second. Explicit Stop bypasses this hold.
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

/** What ⌘⏎ and ⌘-click on Send do in a project's session. */
const START_TIP = "Start as a new session";

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
 * One stable button: an arrow to send/queue/steer, Stop while starting or cancellable,
 * and Resume for a stopped request. Esc only changes Stop's tooltip. ⌘-click and the
 * New session chip keep the background-session send behaviour.
 */
export function SendControls({
  running,
  onResume,
}: {
  running: boolean;
  onResume: (() => void) | null;
}) {
  const aui = useAui();
  const { send, disabled: sendDisabled } = useComposerSend();
  const target = useContext(ComposerTargetContext);
  const conversationId = target?.conversation?.id ?? null;
  const empty = useAuiState((s) => s.composer.isEmpty);
  const cancellable = useAuiState((s) => s.composer.canCancel);
  // The user's own Stop shows at once (#498): no hold after it.
  const stopping = useStopping((s) => turnStopping(s.stops, conversationId));
  const held = useHeldOn(cancellable, STOP_HOLD_MS);
  const canCancel = !stopping && (cancellable || held);
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
  // The composer's To: its message goes to that worker alone, past the session's queue.
  const direct = useDirectTo(conversationId);
  const directName = useWorkerName(direct ?? "");
  const directSending = useDirectSending(conversationId);
  const state = sendButtonState({ armed, canCancel, empty, running, resumable: onResume !== null });
  const sending = state === "send";
  const toWorker = sending && direct !== null;
  const queue = sending && running && !empty && !toWorker;
  const background = sending && projectSession !== null && !empty && (modHeld || chip) && !toWorker;
  const tip = state === "stop" ? armed ? "Press Esc again to stop" : "Stop"
    : state === "resume" ? "Resume" : queue ? steers ? "Steer" : "Queue" : "Send message";
  const tooltip =
    sending && imagesBlocked
      ? IMAGES_BLOCKED
      : sending && uploading
        ? FILES_UPLOADING
        : toWorker
          ? `Send to ${directName ?? "the worker"}`
        : background
        ? START_TIP
        : sending && projectSession
          ? `${queue ? tip : "Send"} ${shortcutKeys("send", mac)} · ${START_TIP} ${shortcutKeys("start-in-tab", mac)}`
          : tip;
  const button = (
    <SendButton
      tooltip={tooltip}
      disabled={sending && (sendDisabled || imagesBlocked || uploading || (toWorker && directSending))}
      state={state}
      background={background}
      onClick={
        state === "resume"
          ? (onResume ?? undefined)
          : state === "stop"
            ? () => {
                if (cancellable) aui.composer().cancel();
              }
          : (event: MouseEvent<HTMLButtonElement>) => {
              if (toWorker) {
                // Its own way: no new session, no paused-queue question.
                if (projectSession) clearBackgroundSend(projectSession);
                if (!empty && !directSending) send();
                return;
              }
              const toNewSession = sending && !empty && (mac ? event.metaKey : event.ctrlKey);
              // A paused queue asks before this click sends.
              if (sending && !empty && !(projectSession && toNewSession) && askPausedSend(conversationId, () => send())) {
                event.preventDefault();
                return;
              }
              if (projectSession) {
                // A ⌘-click whose send never came is not this send's.
                clearBackgroundSend(projectSession);
                if (toNewSession) markBackgroundSend(projectSession);
              }
              send();
            }
      }
    />
  );
  return (
    <div className="flex shrink-0 items-center gap-1">
      <DictateButton owner={dictationOwner} />
      {button}
    </div>
  );
}
