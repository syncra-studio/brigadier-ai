import { useEffect } from "react";
import { create } from "zustand";

import {
  type NotificationPermission,
  askNotificationPermission,
  notificationPermission,
} from "@/ipc/client";
import { useApp } from "@/state/store";

/**
 * Whether Brigadier may show notifications, for Settings › General › Notifications.
 */
export const useNotifications = create<{ permission: NotificationPermission | null }>(() => ({
  permission: null,
}));

export async function loadNotificationPermission(): Promise<void> {
  useNotifications.setState({ permission: await notificationPermission() });
}

/** Asks the OS to let Brigadier notify (its prompt), and keeps the answer. */
export async function askNotifications(): Promise<void> {
  useNotifications.setState({ permission: await askNotificationPermission() });
}

/**
 * Keeps the permission current while `active`: asking the OS doesn't prompt, so it is read
 * again every few seconds while the window shows and when it comes back to the front, so
 * turning notifications on in System Settings shows up.
 */
export function useNotificationPermission(active: boolean): NotificationPermission | null {
  const permission = useNotifications((s) => s.permission);
  useEffect(() => {
    if (!active) return;
    const load = () => {
      if (!useApp.getState().windowVisible) return;
      loadNotificationPermission().catch((error: unknown) => console.error("checking notifications failed", error));
    };
    load();
    const timer = window.setInterval(load, 10_000);
    window.addEventListener("focus", load);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", load);
    };
  }, [active]);
  return permission;
}
