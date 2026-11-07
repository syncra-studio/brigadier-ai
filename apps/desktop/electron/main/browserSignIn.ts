// A page that needs the user, for the agent's hold of a tab and for the tab's banners: a sign-in, a captcha or Google's
// refusal, in the tab or in a window it opened. A tab's page is only looked into when an agent drives it (the renderer
// asks); nothing here types, clicks or answers anything.
import type { WebContents, WebFrameMain } from "electron";

import { isCaptchaFrame, isGoogleRejected, signInStop, stopForAddress, type SignInStop } from "./browserSignInRules";
import { note } from "./events";

/** The world the agent's script has (browserAgentSecrets.ts): the page can't reach it. */
const AGENT_WORLD = 1010;
const FRAME_WAIT_MS = 1000;
const LOOK_AFTER_MS = 250;

/** A visible password field, or a field for a one-time code, in a document and its open shadow roots. */
const FIELDS = `(() => {
  const shows = (e) => { const r = e.getBoundingClientRect(), s = getComputedStyle(e); return r.width >= 8 && r.height >= 8 && r.right > 0 && r.bottom > 0 && s.visibility !== "hidden" && s.display !== "none" && Number(s.opacity) > 0; };
  const asks = (e) => e.type === "password" || /one-time-code|webauthn/.test(e.autocomplete) || /^(otp|totp|mfa|2fa|two.?factor|verification.?code|app_otp)/i.test(e.name || e.id);
  const look = (root) => { for (const e of root.querySelectorAll("input")) if (asks(e) && shows(e)) return true;
    for (const e of root.querySelectorAll("*")) if (e.shadowRoot && look(e.shadowRoot)) return true; return false; };
  return look(document);
})()`;
/** Whether a captcha's frame takes up room: an invisible widget has a frame too, of no size. */
const HAS_ROOM = "innerWidth >= 50 && innerHeight >= 30";

function inFrame<T>(frame: WebFrameMain, code: string): Promise<T | null> {
  const asked = frame.executeJavaScript(code).then((answer) => answer as T, () => null);
  return Promise.race([asked, new Promise<null>((resolve) => setTimeout(() => resolve(null), FRAME_WAIT_MS))]);
}

/** Whether this page should stop an agent now, and why. */
export async function stopOf(guest: WebContents): Promise<SignInStop | null> {
  if (guest.isDestroyed()) return null;
  const url = guest.getURL();
  const byAddress = stopForAddress(url);
  if (byAddress) return byAddress;
  const frames = guest.mainFrame.framesInSubtree.filter((frame) => frame !== guest.mainFrame && !frame.isDestroyed());
  for (const frame of frames) {
    if (!isCaptchaFrame(frame.url) || !(await inFrame<boolean>(frame, HAS_ROOM))) continue;
    const address = new URL(url);
    return { kind: "captcha", site: address.host, key: `captcha ${address.origin}${address.pathname}` };
  }
  const here = await guest.executeJavaScriptInIsolatedWorld(AGENT_WORLD, [{ code: FIELDS }]).catch(() => false) as boolean;
  if (here) return signInStop(url);
  const found = await Promise.all(frames.map((frame) => inFrame<boolean>(frame, FIELDS)));
  return found.includes(true) ? signInStop(url) : null;
}

/**
 * Tells the page hosting a tab what its page, or a window it opened, shows. `tab` is the tab's guest; `contents` is it,
 * or a popup of it, which has no banner of its own and which no agent can see.
 */
export function watchSignIn(contents: WebContents, tab: WebContents): void {
  const tell = (seen: object) => { if (!tab.isDestroyed()) tab.hostWebContents?.send("brigadier:guest-sign-in", { from: tab.id, ...seen }); };
  let rejected = false;
  const address = () => {
    const now = isGoogleRejected(contents.getURL());
    if (now === rejected) return;
    rejected = now;
    note("guest-sign-in-rejected", { webContents: contents.id, shown: now });
    tell({ rejected: now });
  };
  // Not at did-navigate: a tab's first load gets there before the app's page can tell which tab this is.
  contents.on("did-finish-load", address);
  contents.on("did-navigate-in-page", address);

  let due: ReturnType<typeof setTimeout> | null = null;
  const lookSoon = () => {
    if (due) clearTimeout(due);
    due = setTimeout(() => {
      due = null;
      if (contents.isDestroyed()) return;
      if (contents === tab) {
        tell({ look: true });
        return;
      }
      void stopOf(contents).then((stop) => { if (stop) tell({ stop: { ...stop, window: true } }); });
    }, LOOK_AFTER_MS);
  };
  contents.on("did-stop-loading", lookSoon);
  contents.on("did-frame-finish-load", lookSoon);
  contents.on("did-navigate-in-page", lookSoon);
  contents.once("destroyed", () => { if (due) clearTimeout(due); });
}

/** The preload's refusal of a passkey request (guestPasskey.ts), from any frame: the tab says why, a popup's too. */
export function watchPasskeyRefusals(contents: WebContents, tab: WebContents): void {
  contents.ipc.on("brigadier:guest-passkey-refused", (event) => {
    note("guest-passkey-refused", { webContents: contents.id, frame: event.senderFrame?.url ?? "" });
    if (!tab.isDestroyed()) tab.hostWebContents?.send("brigadier:guest-passkey", { from: tab.id });
  });
}
