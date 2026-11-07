/**
 * The guests' real preload (electron/guestPasswords.ts) on real pages, with real layout and trusted input, each case in
 * a frame of its own as each page has one. The test gives its code in ?code= with the types stripped and `electron`
 * standing in, since Vite can't serve that import to a page. What it sent goes to #guest-result (GuestPasswordsRender.test.ts).
 */
import { fixtureClick, fixtureInput } from "@/fixtures/input";

type Sent = [channel: string, payload: unknown];
type Ipc = { send: (channel: string, payload: unknown) => void; on: (channel: string, listener: (event: unknown, payload: unknown) => void) => void };
type Fill = { site?: string; username: string; password: string | null };

const code = atob(new URLSearchParams(location.search).get("code")!);

const stands = new Map<string, Ipc>();
(window as unknown as { installGuest: (frame: Window & { guestIpc?: Ipc }, id: string) => void }).installGuest = (frame, id) => {
  frame.guestIpc = stands.get(id)!;
  (frame as unknown as { eval: (source: string) => void }).eval(code);
};

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const shortName = (channel: string) => channel.replace("brigadier:guest-login-", "");
/** Past the preload's own waits: its report after the page's load, and after a change. */
const SETTLED_MS = 700;

let next = 0;

/** A page in a frame of its own, its preload in before anything else, as Electron puts it. */
async function open(body: string) {
  const id = String(next++);
  const sent: Sent[] = [];
  const listeners: ((event: unknown, payload: unknown) => void)[] = [];
  stands.set(id, {
    send: (channel, payload) => sent.push([shortName(channel), payload]),
    on: (_channel, listener) => listeners.push(listener),
  });
  const frame = document.createElement("iframe");
  frame.style.cssText = "position:fixed;inset:0;width:800px;height:500px;border:0;background:white";
  const page = `<!doctype html><html><head><script>parent.installGuest(window, ${JSON.stringify(id)})</script></head><body>${body}</body></html>`;
  frame.src = URL.createObjectURL(new Blob([page], { type: "text/html" }));
  const loaded = new Promise((resolve) => frame.addEventListener("load", resolve, { once: true }));
  document.body.append(frame);
  await loaded;
  const inner = frame.contentDocument!;
  const reports = () => sent.filter(([channel]) => channel === "fields").map(([, payload]) => payload);
  const field = (selector: string) => inner.querySelector<HTMLInputElement>(selector)!;
  return {
    frame,
    inner,
    sent,
    reports,
    field,
    /** What this page has sent but its reports of fields. */
    caught: () => sent.filter(([channel]) => channel !== "fields"),
    fill: (fill: Fill) => {
      for (const listener of listeners) listener(null, { site: inner.location.origin, ...fill });
    },
    close: () => frame.remove(),
  };
}

const SIGN_IN = `<form id=login>
  <input id=name name=user><input id=pw type=password autocomplete=current-password>
  <button id=go type=button>Sign in</button>
</form>`;

async function signIn() {
  const page = await open(SIGN_IN);
  await wait(SETTLED_MS);
  const seen: Record<string, unknown> = { reports: page.reports() };

  page.field("#name").value = "fake-sam";
  page.field("#pw").value = "fake-Tulip-41";
  page.field("#go").click();
  page.field("#pw").dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  seen.scriptedPress = page.caught();

  await fixtureClick(page.field("#go"));
  seen.pressed = page.caught();
  page.sent.length = 0;

  page.field("#pw").focus();
  await fixtureInput({ type: "key", key: "Enter" });
  seen.enter = page.caught();
  page.close();
  return seen;
}

async function filling() {
  const page = await open(SIGN_IN);
  await wait(SETTLED_MS);
  const seen: Record<string, unknown> = {};
  const events: string[] = [];
  page.field("#pw").addEventListener("input", () => events.push("input"));
  page.field("#pw").addEventListener("change", () => events.push("change"));
  const values = () => [page.field("#name").value, page.field("#pw").value];

  page.fill({ site: "https://another-site.test", username: "fake-sam", password: "fake-Tulip-41" });
  seen.otherSite = values();

  page.field("#name").value = "typed-by-user";
  page.fill({ username: "fake-sam", password: "fake-Tulip-41" });
  seen.typedStays = values();
  seen.events = events;
  page.close();

  const again = await open(SIGN_IN);
  await wait(SETTLED_MS);
  again.fill({ username: "fake-sam", password: null });
  seen.noPassword = [again.field("#name").value, again.field("#pw").value];
  again.close();
  return seen;
}

/** What each shape of page reports once it settled, and whether a fill lands. */
async function shape(body: string) {
  const page = await open(body);
  await wait(SETTLED_MS);
  page.fill({ username: "fake-sam", password: "fake-Tulip-41" });
  const filled = [...page.inner.querySelectorAll("input")].filter((box) => box.value).map((box) => box.id);
  const seen = { report: page.reports().at(-1) ?? null, filled };
  page.close();
  return seen;
}

async function nameFirst() {
  const page = await open(`<form id=step><input id=mail name=email><button id=next type=button>Next</button></form>`);
  await wait(SETTLED_MS);
  const seen: Record<string, unknown> = { first: page.reports().at(-1) };
  page.fill({ username: "fake-sam", password: "fake-Tulip-41" });
  seen.nameFilled = page.field("#mail").value;
  await fixtureClick(page.field("#next"));
  seen.caught = page.caught();
  // The next step: the password box comes in, and the fill lands there.
  const password = page.inner.createElement("input");
  password.type = "password";
  password.id = "pw";
  page.inner.querySelector("form")!.append(password);
  await wait(SETTLED_MS);
  seen.second = page.reports().at(-1);
  page.fill({ username: "fake-sam", password: "fake-Tulip-41" });
  seen.passwordFilled = page.field("#pw").value;
  page.close();
  return seen;
}

async function hiddenBoxes() {
  const page = await open(`<form><input id=name name=user><input id=pw type=password style="display:none">
    <button id=go type=button>Go</button></form>`);
  await wait(SETTLED_MS);
  page.field("#pw").value = "fake-Tulip-41";
  await fixtureClick(page.field("#go"));
  const seen = { caught: page.caught().filter(([channel]) => channel === "attempt") };
  page.close();
  return seen;
}

async function otherForm() {
  const page = await open(`<form id=login><input id=name name=user><input id=pw type=password></form>
    <form id=search><input id=q><button id=find type=button>Search</button></form>
    <button id=outside type=button>Sign in</button>
    <a id=link href="#x">Forgot password</a>
    <button id=own type=button form=login>Not inside</button>`);
  await wait(SETTLED_MS);
  page.field("#name").value = "fake-sam";
  page.field("#pw").value = "fake-Tulip-41";
  const seen: Record<string, unknown> = {};
  await fixtureClick(page.field("#find"));
  seen.searchButton = page.caught();
  await fixtureClick(page.field("#link"));
  seen.link = page.caught();
  await fixtureClick(page.field("#outside"));
  seen.formlessButton = page.caught();
  page.close();
  return seen;
}

/** A page draws its form late: "nothing here" waits for its load, or it would read as a sign-in that worked. */
async function lateForm() {
  const page = await open("<p>Nothing yet</p>");
  const seen: Record<string, unknown> = { atLoad: page.reports() };
  await wait(SETTLED_MS);
  seen.settled = page.reports();
  const box = page.inner.createElement("input");
  box.type = "password";
  page.inner.body.append(box);
  await wait(SETTLED_MS);
  seen.drawn = page.reports().at(-1);
  box.remove();
  await wait(SETTLED_MS);
  seen.gone = page.reports().at(-1);
  page.close();
  return seen;
}

async function drive(): Promise<Record<string, unknown>> {
  return {
    signIn: await signIn(),
    filling: await filling(),
    twoPasswords: await shape(`<form><input id=name name=user><input id=a type=password><input id=b type=password></form>`),
    newPassword: await shape(`<form><input id=name name=user><input id=a type=password autocomplete=new-password></form>`),
    hiddenPassword: await shape(`<form><input id=name name=user><input id=a type=password style="display:none"></form>`),
    invisiblePassword: await shape(`<form><input id=name name=user><input id=a type=password style="opacity:0"></form>`),
    tinyPassword: await shape(`<form><input id=name name=user><input id=a type=password style="width:1px;padding:0;border:0"></form>`),
    searchBox: await shape(`<form><input id=q name=q></form>`),
    hiddenBoxes: await hiddenBoxes(),
    markedName: await shape(`<form><input id=first><input id=marked autocomplete=username><input id=pw type=password></form>`),
    nameFirst: await nameFirst(),
    otherForm: await otherForm(),
    lateForm: await lateForm(),
  };
}

void drive().then(
  (seen) => seen,
  (cause: unknown) => ({ failed: String(cause) }),
).then((seen) => {
  const result = document.createElement("pre");
  result.id = "guest-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
});
