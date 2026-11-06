// Every guest's top frame, in its own isolated world: the page can't reach this or forge what it sends. It finds sign-in
// fields, tells main when one comes or goes, catches what the user signs in with, and fills what main sends. It decides
// nothing: main and the app's page say whether this tab may fill or offer (main/browserLogins.ts). Subframes get no
// preload (nodeIntegrationInSubFrames stays off), so a sign-in inside a frame gets neither, by design.
import { ipcRenderer } from "electron";

const NAME_HINT = /user|login|email|identifier|account/i;
const FIELD_WAIT_MS = 200;
const AFTER_LOAD_MS = 400;

const shown = (field: HTMLElement) => field.checkVisibility({ visibilityProperty: true, opacityProperty: true }) && field.getBoundingClientRect().width > 1;
const visiblePasswords = () => [...document.querySelectorAll<HTMLInputElement>("input[type=password]")].filter(shown);

const textLike = (field: HTMLInputElement) => ["text", "email", "tel", ""].includes(field.getAttribute("type")?.toLowerCase() ?? "");

/** The username box of a password field: marked as one, else the last text box before it in its form. */
function usernameBeside(password: HTMLInputElement): HTMLInputElement | null {
  const scope: ParentNode = password.form ?? document;
  const boxes = [...scope.querySelectorAll<HTMLInputElement>("input")].filter((box) => textLike(box) && shown(box));
  const marked = boxes.find((box) => /\busername\b/.test(box.autocomplete));
  if (marked) return marked;
  const before = boxes.filter((box) => box.compareDocumentPosition(password) & Node.DOCUMENT_POSITION_FOLLOWING);
  return before.at(-1) ?? null;
}

/** A name-first step, such as Google's: one name box alone in its form, with no password box in sight. */
function nameStep(): HTMLInputElement | null {
  if (visiblePasswords().length > 0) return null;
  for (const form of document.forms) {
    const boxes = [...form.querySelectorAll<HTMLInputElement>("input")].filter((box) => textLike(box) && shown(box));
    const only = boxes.length === 1 ? boxes[0]! : null;
    if (only && (/\busername\b/.test(only.autocomplete) || NAME_HINT.test(`${only.name} ${only.id}`))) return only;
  }
  return null;
}

/** A form with one password box signs in; two or more, or one marked as new, is signing up or changing it. */
function signInPassword(): HTMLInputElement | null {
  const fields = visiblePasswords();
  const first = fields[0];
  if (!first || /\bnew-password\b/.test(first.autocomplete)) return null;
  return fields.filter((field) => field.form === first.form).length === 1 ? first : null;
}

let reported: string | null = null;
let loaded = false;

function report(): void {
  const hasPassword = visiblePasswords().length > 0;
  const name = nameStep() !== null;
  // A page draws its form late: "nothing here" waits for its load, or it would read as a sign-in that worked.
  if (!hasPassword && !name && !loaded) return;
  const state = JSON.stringify({ hasPassword, fillable: signInPassword() !== null || name });
  if (state === reported) return;
  reported = state;
  ipcRenderer.send("brigadier:guest-login-fields", JSON.parse(state));
}

let due: ReturnType<typeof setTimeout> | null = null;
function reportSoon(): void {
  if (due) return;
  due = setTimeout(() => {
    due = null;
    report();
  }, FIELD_WAIT_MS);
}

/** The user pressed to go on: what the boxes hold now goes to main, which keeps it until the sign-in worked or failed. */
function caught(target: EventTarget | null): void {
  if (!window.isSecureContext) return;
  const fields = visiblePasswords().filter((field) => field.value);
  // The new password when a form has several.
  const password = fields.at(-1);
  if (password) {
    // A button of another form, such as the site's search, isn't this sign-in.
    const from = target instanceof Element ? target.closest("form") : null;
    if (from && password.form && from !== password.form) return;
    ipcRenderer.send("brigadier:guest-login-attempt", { username: usernameBeside(password)?.value ?? "", password: password.value });
    return;
  }
  const name = nameStep();
  if (name?.value) ipcRenderer.send("brigadier:guest-login-name", name.value);
}

function set(field: HTMLInputElement, value: string): void {
  field.value = value;
  field.dispatchEvent(new Event("input", { bubbles: true }));
  field.dispatchEvent(new Event("change", { bubbles: true }));
}

/** Fills the boxes that are still empty: what the user typed stays. Main names the site, so a page that moved on gets nothing. */
ipcRenderer.on("brigadier:guest-login-fill", (_event, fill: { site: string; username: string; password: string | null }) => {
  if (location.origin !== fill.site || !window.isSecureContext) return;
  const password = signInPassword();
  if (password) {
    const name = usernameBeside(password);
    if (name && !name.value && fill.username) set(name, fill.username);
    if (!password.value && fill.password) set(password, fill.password);
    return;
  }
  const name = nameStep();
  if (name && !name.value && fill.username) set(name, fill.username);
});

window.addEventListener("submit", (event) => { if (event.isTrusted) caught(event.target); }, true);
window.addEventListener("click", (event) => {
  if (!event.isTrusted || !(event.target instanceof Element)) return;
  if (event.target.closest("button, input[type=submit], input[type=button], input[type=image], [role=button]")) caught(event.target);
}, true);
window.addEventListener("keydown", (event) => {
  if (event.isTrusted && event.key === "Enter" && event.target instanceof HTMLInputElement) caught(event.target);
}, true);

window.addEventListener("DOMContentLoaded", () => {
  report();
  new MutationObserver(reportSoon).observe(document.documentElement, { subtree: true, childList: true, attributes: true, attributeFilter: ["style", "class", "hidden", "type"] });
});
window.addEventListener("load", () => {
  setTimeout(() => {
    loaded = true;
    report();
  }, AFTER_LOAD_MS);
});
