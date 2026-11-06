/**
 * The browser tool's page script (src-tauri/src/browser_agent.js) on a test page, as the app
 * runs it in a Browser tab: reading, clicking, typing, and what asks the user first. Writes what
 * it saw to #agent-browser-result (AgentBrowserRender.test.ts reads it).
 */
import source from "../../src-tauri/src/browser_agent.js?raw";

type Answer = Record<string, unknown>;
// The same expression the app evaluates: the script, called with one op.
const agent = (op: Record<string, unknown>): Answer =>
  // oxlint-disable-next-line no-eval
  (0, eval)(`(${source.trim().replace(/;$/, "")})(${JSON.stringify(op)})`) as Answer;

const byId = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const refAt = (selector: string) => document.querySelector(selector)?.getAttribute("data-brigadier-ref") ?? "";
const refOf = (id: string) => refAt(`#${id}`);

let count = 0;
byId("add").addEventListener("click", () => { byId("count").textContent = `Count: ${++count}`; });
const inputs: string[] = [];
byId("name").addEventListener("input", (event) => inputs.push((event.target as HTMLInputElement).value));
byId("chat").addEventListener("keydown", (event) => {
  if (event.key !== "Enter") return;
  event.preventDefault();
  const chat = byId<HTMLInputElement>("chat");
  const item = document.createElement("li");
  item.textContent = chat.value;
  byId("sent").append(item);
  chat.value = "";
});
const submitted: string[] = [];
for (const id of ["login", "pay"]) byId(id).addEventListener("submit", (event) => { event.preventDefault(); submitted.push(id); });

const seen: Record<string, unknown> = {};
const first = agent({ kind: "read", max: 20000 });
seen.read = first;
seen.refs = Object.fromEntries(["add", "name", "size", "note", "chat", "email", "password", "card", "code", "terms"].map((id) => [id, refOf(id)]));
seen.cut = agent({ kind: "read", max: 40 });
seen.again = (agent({ kind: "read", max: 20000 }) as { text: string }).text === (first as { text: string }).text;

seen.click = agent({ kind: "click", target: refOf("add") });
seen.clicked = byId("count").textContent;
seen.typeName = agent({ kind: "type", target: refOf("name"), text: "Ada", submit: false });
seen.name = { value: byId<HTMLInputElement>("name").value, inputs };
seen.typeSize = agent({ kind: "type", target: refOf("size"), text: "Large", submit: false });
seen.size = byId<HTMLSelectElement>("size").value;
seen.typeNote = agent({ kind: "type", target: refOf("note"), text: "Gift wrap", submit: false });
seen.note = byId("note").textContent;
seen.check = agent({ kind: "click", target: refOf("terms") });
seen.checked = byId<HTMLInputElement>("terms").checked;

// Enter sends a chat message: plain, it goes; declared as sent in the user's name, it asks.
seen.chat = agent({ kind: "type", target: refOf("chat"), text: "hello", submit: true });
seen.declaredSend = agent({ kind: "type", target: refOf("chat"), text: "hi there", submit: true, declared: "send" });
seen.sent = [...byId("sent").children].map((item) => item.textContent);
seen.chatValue = byId<HTMLInputElement>("chat").value;

// A password field gets no ref, and with one given anyway nothing goes into it, yes or no.
seen.email = agent({ kind: "type", target: refOf("email"), text: "ada@example.com", submit: false });
seen.passwordRef = refOf("password");
byId("password").setAttribute("data-brigadier-ref", "e90");
seen.password = agent({ kind: "type", target: "e90", text: "hunter2", submit: false });
seen.passwordDeclared = agent({ kind: "type", target: "e90", text: "hunter2", submit: false, declared: "signIn" });
seen.passwordYes = agent({ kind: "type", target: "e90", text: "hunter2", submit: false, approved: location.href });
seen.passwordClick = agent({ kind: "click", target: "e90" });
seen.passwordValue = byId<HTMLInputElement>("password").value;
// Declared as a payment, a card field still asks as what it is.
seen.cardDeclared = agent({ kind: "type", target: refOf("card"), text: "4242424242424242", submit: false, declared: "pay" });
// Filled by the user: a button beside it asks, with a yes given at this address.
byId<HTMLInputElement>("password").value = "hunter2";
seen.signIn = agent({ kind: "click", target: refAt("#login button") });
seen.signInYes = agent({ kind: "click", target: refAt("#login button"), approved: location.href });
seen.card = agent({ kind: "type", target: refOf("card"), text: "4242424242424242", submit: false });
seen.code = agent({ kind: "type", target: refOf("code"), text: "123456", submit: false });
seen.declaredPay = agent({ kind: "click", target: refAt("#pay button"), declared: "pay" });
seen.submitted = [...submitted];
// A sign-in without a form: the button beside a filled password asks too.
byId<HTMLInputElement>("pin").value = "1234";
seen.unlock = agent({ kind: "click", target: refOf("unlock") });

seen.stale = agent({ kind: "click", target: "e999" });
seen.notField = agent({ kind: "type", target: refOf("add"), text: "x", submit: false });
seen.link = agent({ kind: "click", target: refAt('a[href="#docs"]') });
seen.hash = location.hash;

// Filled by the user or autofill: a password shown as text and a disguised field stay hidden; a
// card number and a code are read as filled, never by their value.
const shownPassword = Object.assign(document.createElement("input"), { value: "s3cret" });
shownPassword.setAttribute("autocomplete", "current-password");
shownPassword.setAttribute("aria-label", "Shown password");
document.body.append(shownPassword);
const disguised = Object.assign(document.createElement("input"), { type: "text", value: "s3cret-word" });
disguised.setAttribute("aria-label", "Security word");
disguised.style.setProperty("-webkit-text-security", "disc");
document.body.append(disguised);
disguised.setAttribute("data-brigadier-ref", "e91");
seen.disguised = agent({ kind: "type", target: "e91", text: "hacked", submit: false, approved: location.href });
seen.disguisedValue = disguised.value;
byId<HTMLInputElement>("card").value = "4242424242424242";
byId<HTMLInputElement>("code").value = "123456";
const filledRead = (agent({ kind: "read", max: 20000 }) as { text: string }).text;
seen.filled = filledRead.split("\n").filter((line) => /assword|Card number|Code/.test(line));
seen.secretsRead = /hunter2|1234|s3cret/.test(filledRead);

// A display: contents wrapper has no box of its own, but what it holds shows (#362).
const wrapper = document.createElement("div");
wrapper.style.display = "contents";
wrapper.innerHTML = '<p>Inside a wrapper</p><button>Wrapped</button><span style="visibility: hidden">Hidden inside</span>';
document.body.append(wrapper);
seen.contents = (agent({ kind: "read", max: 20000 }) as { text: string }).text.split("\n").slice(-2);
// A hidden one's own text stays out; a child that shows itself again is read.
const hiddenWrapper = document.createElement("div");
hiddenWrapper.style.cssText = "display: contents; visibility: hidden";
hiddenWrapper.innerHTML = 'Hidden wrapper text<p style="visibility: visible">Shown again</p>';
document.body.append(hiddenWrapper);
seen.hiddenContents = (agent({ kind: "read", max: 20000 }) as { text: string }).text.split("\n").slice(-2);

byId("agent-browser-result").textContent = JSON.stringify(seen);
