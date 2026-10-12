import { browserConfigure, openUrl } from "@/ipc/client";
import type { Settings } from "@/ipc/generated";
import { linkDestination } from "@/state/linkRouting";
import { openBrowserTab } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

/** The session in front, when one is: the one a Browser tab opens in. */
function sessionInFront(): string | null {
  const { selection, conversations } = useApp.getState();
  if (selection.type !== "conversation") return null;
  return conversations[selection.id]?.kind === "session" ? selection.id : null;
}

/**
 * Opens a web link where the user's Settings › Browser say. `agent`: it came from an agent (a
 * link in its reply, a preview's address), which the browsing history files under Agent.
 * `conversationId`: the session it belongs to (else the one in front).
 */
export async function openLink(
  url: string,
  { agent = false, conversationId }: { agent?: boolean; conversationId?: string } = {},
): Promise<void> {
  const { settings, info } = useApp.getState();
  const session = conversationId ?? sessionInFront();
  const where = linkDestination(url, settings, { session: session !== null, embedded: info?.platform !== "linux" });
  if (where === "tab" && session) {
    openBrowserTab(session, url, agent);
    return;
  }
  if (where) await openUrl(url);
}

/** Whether the built-in browser exists (Settings › Browser). */
export function useBrowserOn(): boolean {
  return useApp((s) => s.settings.browserEnabled);
}

/** Keeps the shell's copy of the browser choices it acts on itself (see `browser.rs`). */
let sent = "";
function configure({ settings }: { settings: Settings }): void {
  const options = {
    enabled: settings.browserEnabled,
    downloadsDir: settings.browserDownloadsDir,
    askDownloads: settings.browserAskDownloads,
  };
  const key = JSON.stringify(options);
  if (key === sent) return;
  sent = key;
  browserConfigure(options).catch((error: unknown) => console.error("configuring the browser failed", error));
}
/** Starts keeping the shell's copy (the app's start, inside the shell). */
export function startBrowserOptions(): void {
  configure(useApp.getState());
  useApp.subscribe(configure);
}
