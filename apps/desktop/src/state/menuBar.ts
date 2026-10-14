import { invoke } from "@tauri-apps/api/core";

import { setRunningChats } from "@/ipc/client";
import type { Conversation, RunningChat } from "@/ipc/generated";
import { runningConversations, useActivity } from "@/state/activity";
import { useApp } from "@/state/store";

/**
 * Keeps the menu-bar item's "Running" list: the conversations with a turn or a worker in
 * progress, by title. Picking one opens it.
 */

let listed = "";

function sync(): void {
  const { conversations } = useApp.getState();
  const chats: RunningChat[] = runningConversations(useActivity.getState().byConversation)
    .map((id) => conversations[id])
    // Side chats aren't listed anywhere; archived ones aren't running.
    .filter(
      (conversation): conversation is Conversation =>
        conversation !== undefined && !conversation.sideOf && conversation.lifecycle === "active",
    )
    .map((conversation) => ({ id: conversation.id, title: conversation.title }));
  const key = JSON.stringify(chats);
  if (key === listed) return;
  listed = key;
  setRunningChats(chats).catch((error: unknown) => {
    console.error("updating the menu-bar item failed", error);
  });
}

/** Whether the computer-use item shows, as last told to the shell. */
let computerMenu: boolean | null = null;
/** Set once the app is up: making the item takes the shell's main thread a while, which at
 * startup holds up the first screen. */
let computerMenuAllowed = false;

/** Shows the computer-use menu-bar item while computer use and "Show in menu bar" are on. */
function syncComputerMenu(): void {
  const { settings, info } = useApp.getState();
  const shown =
    computerMenuAllowed && info?.platform === "macos" && settings.computerUse && settings.computerUseMenuBar;
  if (shown === computerMenu) return;
  computerMenu = shown;
  invoke("set_computer_menu", { shown }).catch((error: unknown) => {
    console.error("updating the computer-use menu-bar item failed", error);
  });
}

/** Lets the computer-use item show, once startup is done. */
export function allowComputerMenu(): void {
  computerMenuAllowed = true;
  syncComputerMenu();
}

export function startMenuBar(): void {
  useActivity.subscribe(sync);
  useApp.subscribe((state, previous) => {
    if (state.conversations !== previous.conversations) sync();
    if (state.settings !== previous.settings) syncComputerMenu();
  });
  sync();
  syncComputerMenu();
}
