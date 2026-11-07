// What counts as a page only the user should go through: signing in, a captcha, and Google's refusal. Addresses only,
// so a test can read them. Agents never go through these (src/state/browserSignIn.ts stops them first).

export type SignInKind = "signIn" | "captcha" | "rejected";
/** `key` says when two stops are the same one: after the user's Continue the tab isn't stopped for it again. */
export type SignInStop = { kind: SignInKind; site: string; key: string; window?: boolean };

/** Hosts that ask for an address on one page and a password on the next, so the first has no field to find. */
const SIGN_IN_HOSTS = new Set(["accounts.google.com", "login.microsoftonline.com", "login.live.com", "appleid.apple.com", "idmsa.apple.com"]);
const GITHUB_SIGN_IN = /^\/(login|session|sessions)(\/|$)/;

const parsed = (url: string): URL | null => {
  try {
    const address = new URL(url);
    return /^https?:$/.test(address.protocol) ? address : null;
  } catch {
    return null;
  }
};

export const signInStop = (url: URL | string): SignInStop | null => {
  const address = typeof url === "string" ? parsed(url) : url;
  return address && { kind: "signIn", site: address.host, key: `signIn ${address.origin}` };
};

const captchaStop = (address: URL): SignInStop => ({ kind: "captcha", site: address.host, key: `captcha ${address.origin}${address.pathname}` });

/** Google's "This browser or app may not be secure", which ends the sign-in. */
export function isGoogleRejected(url: string): boolean {
  const address = parsed(url);
  return address?.host === "accounts.google.com" && /^(\/v3)?\/signin\/rejected(\/|$)/.test(address.pathname);
}

/** What a page's own address says about it. */
export function stopForAddress(url: string): SignInStop | null {
  const address = parsed(url);
  if (!address) return null;
  if (isGoogleRejected(url)) return { kind: "rejected", site: address.host, key: `rejected ${address.origin}` };
  if (SIGN_IN_HOSTS.has(address.host)) return signInStop(address);
  if (address.host === "github.com" && GITHUB_SIGN_IN.test(address.pathname)) return signInStop(address);
  // Google's "unusual traffic" page, and Amazon's own.
  if (/^(www\.)?google\.[a-z.]+$/.test(address.host) && address.pathname.startsWith("/sorry/")) return captchaStop(address);
  if (/(^|\.)amazon\.[a-z.]+$/.test(address.host) && address.pathname.startsWith("/errors/validateCaptcha")) return captchaStop(address);
  return null;
}

/** A frame holding a captcha someone may have to answer. An invisible one says so, and isn't anyone's to answer. */
export function isCaptchaFrame(url: string): boolean {
  const address = parsed(url);
  if (!address) return false;
  const host = address.host;
  if (host === "challenges.cloudflare.com") return true;
  if (host === "geo.captcha-delivery.com" || host.endsWith(".arkoselabs.com")) return true;
  if (host.endsWith("hcaptcha.com")) return /(^|[#&])frame=checkbox/.test(address.hash) && !/size=invisible/.test(address.hash);
  if (host === "www.google.com" || host === "www.recaptcha.net") return /^\/recaptcha\/(api2|enterprise)\/anchor/.test(address.pathname) && address.searchParams.get("size") !== "invisible";
  return false;
}
