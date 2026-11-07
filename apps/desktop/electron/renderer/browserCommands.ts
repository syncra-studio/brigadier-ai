// The browser's commands, by the Tauri build's names, answered in the page: a tab's <webview> lives here.
import type { BrowserBounds, BrowserEvent } from "@/ipc/generated";

import { agentCapture, agentDrive, agentEvents, agentLoad, agentRun, agentSignInStop } from "./browserAgentHands";
import { closePage, find, focusPage, go, navigatePage, openPage, placePage, printPage, setModal, zoom } from "./tabLayer";

type Args = Record<string, unknown>;
type Channelish = { onmessage: (message: BrowserEvent) => void };

export const browserCommands: Record<string, (args: Args) => unknown> = {
  browser_open: (args) => {
    const events = args.events as Channelish;
    return openPage(args.id as string, args.url as string, args.bounds as BrowserBounds, (event) => events.onmessage(event));
  },
  browser_navigate: (args) => navigatePage(args.id as string, args.url as string, args.bounds as BrowserBounds),
  browser_place: (args) => placePage(args.id as string, args.bounds as BrowserBounds | null),
  browser_layer: (args) => setModal(args.modal as boolean),
  browser_go: (args) => go(args.id as string, args.action as string),
  browser_zoom: (args) => zoom(args.id as string, args.zoom as number | null),
  browser_find: (args) => find(args.id as string, args.query as string, args.step as "type" | "next" | "previous"),
  browser_print: (args) => printPage(args.id as string),
  browser_close: (args) => closePage(args.id as string),
  browser_configure: () => null,
  browser_focus: (args) => focusPage(args.id as string | null),
  browser_permission: (args) => window.brigadierShell.permissionAnswer(args.request as number, args.answer as string),
  browser_permission_settings: (args) => window.brigadierShell.permissionOpenSettings(args.device as string),
  browser_profiles_read: () => window.brigadierShell.profiles("read"),
  browser_profiles_write: (args) => window.brigadierShell.profiles("write", args),
  browser_profile_clear: (args) => window.brigadierShell.profiles("clear", args),
  browser_profile_delete: (args) => window.brigadierShell.profiles("remove", args),
  browser_profile_counts: (args) => window.brigadierShell.profiles("counts", args),
  // Not built yet: browser features come section by section (L4). Saved data reads as none.
  browser_passwords: (args) => window.brigadierShell.vault("list", args),
  browser_password_add: (args) => window.brigadierShell.vault("add", args),
  browser_password_remove: (args) => window.brigadierShell.vault("remove", args),
  browser_password_reveal: (args) => window.brigadierShell.vault("reveal", args),
  browser_password_never_sites: (args) => window.brigadierShell.vault("never", args),
  browser_password_allow_site: (args) => window.brigadierShell.vault("allow", args),
  browser_password_offer_answer: (args) => window.brigadierShell.vault("answer", args),
  browser_bookmarks: (args) => window.brigadierShell.bookmarks("list", args),
  browser_bookmark_add: (args) => window.brigadierShell.bookmarks("add", args),
  browser_bookmark_update: (args) => window.brigadierShell.bookmarks("update", args),
  browser_bookmark_remove: (args) => window.brigadierShell.bookmarks("remove", args),
  browser_history: () => [],
  browser_history_remove: () => null,
  browser_downloads: () => [],
  browser_download_open: () => null,
  browser_downloads_remove: () => null,
  browser_clear: () => null,
  browser_site_count: () => null,
  browser_sites: (args) => window.brigadierShell.profiles("sites", args),
  browser_site_set: (args) => window.brigadierShell.profiles("setSite", args),
  browser_read: () => null,
  browser_snapshot: () => null,
  browser_agent: (args) => agentRun(args.id as string, args.op as Record<string, unknown>),
  browser_agent_load: (args) => agentLoad(args.id as string, args.url as string),
  browser_agent_capture: (args) => agentCapture(args.id as string),
  browser_agent_drive: (args) => agentDrive(args.id as string, args.on as boolean, args.resumed === true),
  browser_agent_sign_in_stop: (args) => agentSignInStop(args.id as string),
  browser_agent_events: (args) => agentEvents(args.id as string, args.since as number, args.kinds as string[]),
  browser_import_sources: () => [],
  browser_import: () => null,
  browser_import_open_access: () => null,
};
