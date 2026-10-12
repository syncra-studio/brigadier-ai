import type { LinkTarget, Settings } from "@/ipc/generated";

/**
 * Where a web link opens: in a Browser tab of the session in front, or in the user's default
 * browser (Settings › Browser). Off, the built-in browser doesn't exist and every link goes to
 * the default browser.
 */

/** Hosts a local development server answers on: loopback addresses and `*.localhost`. */
export function localUrl(url: string): boolean {
  try {
    const host = new URL(url).hostname.toLowerCase();
    return (
      host === "localhost" ||
      host.endsWith(".localhost") ||
      host === "0.0.0.0" ||
      host === "[::1]" ||
      /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host)
    );
  } catch {
    return false;
  }
}

export type LinkChoices = Pick<Settings, "browserEnabled" | "webLinksIn" | "localLinksIn">;

/**
 * Whether `url` opens in a Browser tab ("tab") or the default browser ("system"); `null` for
 * anything that isn't a web page. A tab needs the browser on, the user's choice for that kind
 * of link, a session in front to hold it, and a system that shows pages inside the app.
 */
export function linkDestination(
  url: string,
  choices: LinkChoices,
  { session, embedded }: { session: boolean; embedded: boolean },
): "tab" | "system" | null {
  if (!/^https?:\/\//i.test(url)) return null;
  const target: LinkTarget = localUrl(url) ? choices.localLinksIn : choices.webLinksIn;
  return choices.browserEnabled && target === "brigadier" && session && embedded ? "tab" : "system";
}
