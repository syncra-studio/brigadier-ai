// A site permission's decisions, with no Electron in them: permissions.ts and browserPermissionsOs.ts carry them out.
import { type SiteAnswer, siteAnswer, type SiteAsk, type SiteChoice, type SiteDevice, type SitePermission } from "./profiles.ts";

export type CardAnswer = "once" | "visiting" | "never" | "dismiss";
export type RequestDecision = SiteAnswer | "not-the-tabs-site" | "no-location";
export type LocationStatus = "off" | "undetermined" | "denied" | "restricted" | "allowed" | "unknown";

/** Linux has no location: Chromium goes to Google's paid service there, which would need a key in the app (Electron's GOOGLE_API_KEY). */
export const locationAvailable = (platform: NodeJS.Platform): boolean => platform !== "linux";

/** A question names one site, the tab's: another site's frame asking gets no question, and a page with no tab no door. */
export function decideRequest(request: {
  origin: string | null;
  top: string | null;
  hasTab: boolean;
  deleting: boolean;
  asked: SiteAsk;
  site: SiteChoice | undefined;
  platform: NodeJS.Platform;
}): RequestDecision {
  const { origin, top, hasTab, deleting, asked, site, platform } = request;
  if (!origin || origin !== top || !hasTab || deleting) return "not-the-tabs-site";
  // Before any card: a yes here couldn't bring a position.
  if (asked.location && !locationAvailable(platform)) return "no-location";
  return siteAnswer(site, asked);
}

/** What the page's permission check reads: only a saved Allow, and never location where there is none. */
export function checkAllows(origin: string | null, asked: SiteAsk, site: SiteChoice | undefined, platform: NodeJS.Platform): boolean {
  if (asked.location && !locationAvailable(platform)) return false;
  return origin !== null && siteAnswer(site, asked) === "grant";
}

/** Allow while visiting the site and Never allow are saved for the site in its profile; the others aren't. */
export function savedFromCard(choice: CardAnswer, asked: SiteAsk): Partial<Record<SiteDevice, SitePermission>> {
  const saved = choice === "visiting" ? "allow" : choice === "never" ? "block" : null;
  if (!saved) return {};
  return Object.fromEntries((["camera", "microphone", "location"] as const).filter((device) => asked[device]).map((device) => [device, saved]));
}

export const cardAllows = (choice: CardAnswer): boolean => choice === "once" || choice === "visiting";

/** Asks macOS when it has no answer yet and waits for one. Unanswered at the end counts as no. */
export async function locationAllowed(
  read: () => LocationStatus,
  ask: () => void,
  { answerMs = 120_000, readMs = 250 }: { answerMs?: number; readMs?: number } = {},
): Promise<boolean> {
  let status = read();
  if (status === "undetermined") {
    ask();
    const until = Date.now() + answerMs;
    while (status === "undetermined" && Date.now() < until) {
      await new Promise((resolve) => setTimeout(resolve, readMs));
      status = read();
    }
  }
  return status === "allowed";
}

/** After the person's yes: a refusal goes to the page at once, and to the tab by name; the log has what the system said. */
export async function answerFromOs<Device extends string>({ refusedByOs, log, tell, answer }: {
  refusedByOs: () => Promise<Device[]>;
  log: (refused: Device[]) => void;
  tell: (refused: Device[]) => void;
  answer: (allowed: boolean) => void;
}): Promise<void> {
  let refused: Device[];
  try {
    refused = await refusedByOs();
  } catch {
    answer(false);
    return;
  }
  log(refused);
  if (refused.length > 0) tell(refused);
  answer(refused.length === 0);
}
