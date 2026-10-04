import { create } from "zustand";

import { type NotificationPermission, notificationPermission } from "@/ipc/client";

/**
 * Whether Brigadier may show notifications, for the overnight run cards: a run tells the user
 * when it finishes with one, so the card says when they're off.
 */
export const useNotifications = create<{ permission: NotificationPermission | null }>(() => ({
  permission: null,
}));

export async function loadNotificationPermission(): Promise<void> {
  useNotifications.setState({ permission: await notificationPermission() });
}
