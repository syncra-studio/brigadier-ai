import { openLink } from "@/state/links";
import { localUrl } from "@/state/linkRouting";
import { sessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/**
 * Dev servers open here: a server's address printed in a terminal is offered as a Browser tab, and
 * `vite --open` and the like open one through the terminal's `BROWSER` (see `terminals.rs`).
 */

// eslint-disable-next-line no-control-regex
const COLORS = /\x1b\[[0-9;]*[A-Za-z]/g;
const ADDRESS = /https?:\/\/(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\]|[\w-]+\.localhost)(?![\w-]|\.[\w-])(?::\d+)?(?:\/[^\s'"<>)]*)?/g;
/** A server keeps printing its address; asked once for a while. */
const QUIET_MS = 10 * 60_000;
const offered = new Map<string, number>();

/** The server addresses in what a terminal printed. */
export function serverAddresses(output: string): string[] {
  return [...new Set(output.replace(COLORS, "").match(ADDRESS) ?? [])].filter(localUrl);
}

function showing(origin: string): boolean {
  const { selection } = useApp.getState();
  if (selection.type !== "conversation") return false;
  return sessionTabs(selection.id).tabs.some((tab) => {
    if (tab.kind !== "browser") return false;
    try {
      return new URL(tab.url).origin === origin;
    } catch {
      return false;
    }
  });
}

/** A terminal printed `output`: a dev server's address in it is offered once. */
export function noteTerminalOutput(output: string): void {
  if (!output.includes("://")) return;
  if (!useApp.getState().settings.browserEnabled || useApp.getState().settings.localLinksIn !== "brigadier") return;
  for (const url of serverAddresses(output)) {
    const { origin, host } = new URL(url);
    const now = Date.now();
    if (now - (offered.get(origin) ?? -QUIET_MS) < QUIET_MS || showing(origin)) continue;
    offered.set(origin, now);
    toast(`Dev server at ${host}`, { actions: [{ label: "Open here", run: () => void openLink(url) }] });
  }
}

/** A `BROWSER` asked on its terminal for `value`, its token and address. */
export function openFromTerminal(value: string, token: string | null): void {
  const at = value.indexOf(";");
  if (at < 0 || !token || value.slice(0, at) !== token) return;
  const url = value.slice(at + 1);
  if (localUrl(url)) {
    offered.set(new URL(url).origin, Date.now());
    void openLink(url).catch(() => {});
  }
}
