import assert from "node:assert/strict";
import test from "node:test";

import { isCaptchaFrame, isGoogleRejected, signInStop, stopForAddress } from "./browserSignInRules.ts";

test("hosts that ask for an address first are sign-ins on any of their pages", () => {
  for (const host of ["accounts.google.com", "login.microsoftonline.com", "login.live.com", "appleid.apple.com", "idmsa.apple.com"]) {
    assert.deepEqual(stopForAddress(`https://${host}/some/page?x=1`), { kind: "signIn", site: host, key: `signIn https://${host}` });
  }
});

test("GitHub stops only at its sign-in pages", () => {
  for (const path of ["/login", "/login/oauth/authorize", "/session", "/sessions/two-factor"]) {
    assert.equal(stopForAddress(`https://github.com${path}`)?.kind, "signIn", path);
  }
  for (const path of ["/", "/loginfo", "/stephen/sessions", "/settings"]) {
    assert.equal(stopForAddress(`https://github.com${path}`), null, path);
  }
  assert.equal(stopForAddress("https://gist.github.com/login"), null);
});

test("Google's refusal is its own kind, on both of its addresses", () => {
  for (const path of ["/signin/rejected", "/v3/signin/rejected", "/v3/signin/rejected/more"]) {
    assert.equal(isGoogleRejected(`https://accounts.google.com${path}`), true, path);
    assert.deepEqual(stopForAddress(`https://accounts.google.com${path}`), { kind: "rejected", site: "accounts.google.com", key: "rejected https://accounts.google.com" });
  }
  assert.equal(isGoogleRejected("https://accounts.google.com/signin/rejectedly"), false);
  assert.equal(isGoogleRejected("https://accounts.google.com/v3/signin/identifier"), false);
  assert.equal(isGoogleRejected("https://example.com/v3/signin/rejected"), false);
});

test("Google's and Amazon's own captcha pages stop, each page its own stop", () => {
  assert.deepEqual(stopForAddress("https://www.google.com/sorry/index?continue=x"), { kind: "captcha", site: "www.google.com", key: "captcha https://www.google.com/sorry/index" });
  assert.equal(stopForAddress("https://www.google.co.uk/sorry/index")?.kind, "captcha");
  assert.equal(stopForAddress("https://www.amazon.de/errors/validateCaptcha")?.kind, "captcha");
  assert.equal(stopForAddress("https://www.google.com/search?q=sorry"), null);
  assert.equal(stopForAddress("https://www.amazon.com/errors/other"), null);
  assert.equal(stopForAddress("https://notgoogle.com/sorry/index"), null);
});

test("an address that isn't the web's stops nothing", () => {
  for (const url of ["", "about:blank", "file:///accounts.google.com/login", "not a url", "app://accounts.google.com/"]) {
    assert.equal(stopForAddress(url), null, url);
    assert.equal(isGoogleRejected(url), false, url);
    assert.equal(isCaptchaFrame(url), false, url);
    assert.equal(signInStop(url), null, url);
  }
});

test("one sign-in per site: two of its pages are the same stop, and the port counts", () => {
  assert.equal(signInStop("https://site.test/login")?.key, signInStop("https://site.test/two-factor")?.key);
  assert.notEqual(signInStop("https://site.test/")?.key, signInStop("https://other.test/")?.key);
  assert.deepEqual(signInStop(new URL("https://site.test:8443/login")), { kind: "signIn", site: "site.test:8443", key: "signIn https://site.test:8443" });
});

test("captcha frames someone may have to answer, and not the invisible ones", () => {
  for (const url of [
    "https://challenges.cloudflare.com/cdn-cgi/challenge-platform/h/b/turnstile",
    "https://geo.captcha-delivery.com/captcha/?initialCid=x",
    "https://client-api.arkoselabs.com/fc/gc/",
    "https://newassets.hcaptcha.com/captcha/v1/abc/static/hcaptcha.html#frame=checkbox&id=1",
    "https://www.google.com/recaptcha/api2/anchor?k=key&size=normal",
    "https://www.recaptcha.net/recaptcha/enterprise/anchor?k=key",
  ]) assert.equal(isCaptchaFrame(url), true, url);
  for (const url of [
    "https://www.google.com/recaptcha/api2/anchor?k=key&size=invisible",
    "https://www.google.com/recaptcha/api2/bframe?k=key",
    "https://www.google.com/search",
    "https://newassets.hcaptcha.com/captcha/v1/abc/static/hcaptcha.html#frame=checkbox&size=invisible",
    "https://newassets.hcaptcha.com/captcha/v1/abc/static/hcaptcha.html#frame=challenge",
    "https://example.com/recaptcha/api2/anchor",
  ]) assert.equal(isCaptchaFrame(url), false, url);
});
