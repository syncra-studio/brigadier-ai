/**
 * Onboarding's agents step over a faked daemon. ?claude= and ?codex= set how each agent is
 * found (missing, signedOut or connected; Claude Code missing and Codex connected unless said),
 * ?waiting=claude has Claude Code's sign-in under way (offering its code page with ?code=1)
 * and ?plan= names the plan Codex reports:
 * the step stays as it opens, for screenshots. ?drive=1 presses Claude Code's Install (Connect:
 * the agent's installer, then its sign-in), has it installed and its sign-in under way, cancels
 * that from Waiting… (which reads Cancel when hovered), presses Sign in, fails it, signs in
 * again and has the sign-in noticed, writing what each card and the step said at every point to
 * #onboarding-agents-result (OnboardingAgentsRender.test.ts reads it). The faked daemon moves on
 * only when the drive says, so every point is read as it stands. With ?code=1 the drive instead
 * has Claude Code's sign-in offer its code page, opens it, pastes a code and presses Enter.
 * With ?install=1 it only presses Install and follows the card through the install and the
 * sign-in to Connected. With ?nodownloader=1 the install fails as it does on a Linux with
 * neither curl nor wget, and the drive opens the install guide the card links.
 * ?platform= names the platform (macos, linux or windows; none unless given).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { AgentsStep } from "@/app/setup/onboarding/AgentsStep";
import { Dialog, DialogContent, TooltipProvider } from "@/components/overlays";
import { OVERVIEWS, status } from "@/fixtures/agents";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import type { AgentConnect, ProviderKind, ProviderOverview, ProviderStatus, Request } from "@/ipc/generated";
import { applyConnectEvents } from "@/state/connect";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}

/** An agent as the daemon finds it. */
function found(provider: ProviderKind, how: string, plan: string): ProviderStatus {
  if (how === "missing") return status(provider, { path: null, version: null, loggedIn: false, authMethod: null, plan: null });
  if (how === "signedOut") return status(provider, { loggedIn: false, authMethod: null, plan: null });
  return status(provider, { plan });
}

const [claude, codex] = OVERVIEWS as [ProviderOverview, ProviderOverview];
let claudeStatus = found("claude", params.get("claude") ?? "missing", "max");
const codexStatus = found("codex", params.get("codex") ?? "connected", params.get("plan") ?? "pro");

function show() {
  useApp.setState((state) => ({
    providers: {
      ...state.providers,
      view: {
        providers: [
          { ...claude, status: claudeStatus },
          { ...codex, status: codexStatus },
        ],
        sessions: [],
        fixtures: [],
      },
    },
  }));
}

/** What the faked daemon publishes as Connect's view. */
function publish(agents: AgentConnect[]) {
  applyConnectEvents([
    { seq: 0, stream: "connect", kind: "agent.connectChanged", atMs: Date.now(), event: { type: "agentConnectChanged", connect: { agents } } },
  ] as never);
}
const claudeAt = (progress: AgentConnect["progress"], codePage?: string) =>
  publish([{ provider: "claude", progress, ...(codePage === undefined ? {} : { codePage }) }]);
const CODE_PAGE = "https://claude.com/cai/oauth/authorize?code=true";
const NO_DOWNLOADER =
  "Installing Claude Code needs curl or wget, and neither is installed. Install one (for example `sudo apt install curl`) and try again, or install Claude Code yourself.";

const asked: string[] = [];
mockIPC((command, payload) => {
  if (command === "open_url") {
    asked.push(`openUrl ${(payload as { url: string }).url}`);
    return null;
  }
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "getProviders") return { method: req.method, view: useApp.getState().providers.view };
  // A check publishes what it finds (as events do in the app).
  if (req.method === "refreshProviders") {
    show();
  }
  if (req.method === "sendConnectCode") asked.push(`sendConnectCode ${req.provider} ${req.code}`);
  if (req.method === "connectAgent" || req.method === "cancelConnect") {
    asked.push(`${req.method} ${req.provider}`);
    // A missing agent installs first.
    if (req.method === "connectAgent") claudeAt({ type: claudeStatus.path ? "signingIn" : "installing" });
    else publish([]);
  }
  return { method: req.method };
});

useApp.setState({ connection: { status: "connected", daemon: null, reason: null } });
// ?mac=1 reads as on a Mac ("this Mac", ⌘ shortcuts), for screenshots, as ?platform=macos does.
const platform = params.get("platform") ?? (params.has("mac") ? "macos" : null);
if (platform) useApp.setState((state) => ({ info: { ...state.info, platform } as typeof state.info }));
show();
if (params.get("waiting") === "claude") claudeAt({ type: "signingIn" }, params.has("code") ? CODE_PAGE : undefined);

let continued = 0;
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const cards = () => [...document.querySelectorAll('[data-slot="agent-card"]')];

/** What a card says: its name, the line under it and its action (a button, or what it shows). */
function card(index: number) {
  const element = cards()[index];
  const action = element?.querySelector('button, [data-slot="badge"], [role="status"]');
  return {
    name: text(element?.querySelector("p")),
    line: text(element?.querySelector('[data-slot="agent-line"]')),
    // Waiting… holds Cancel too; the label showing is the one it says.
    action: action?.matches('[data-slot="agent-wait"]') ? shown(action) : text(action),
  };
}
/** The label of Waiting… / Cancel that shows. */
function shown(waitButton: Element) {
  return text([...waitButton.children].find((label) => getComputedStyle(label).visibility === "visible"));
}
const form = () => document.querySelector('[data-slot="connect-code"]');
const claudeCard = () => card(cards().findIndex((element) => text(element.querySelector("p")) === "Claude Code"));

async function until(check: () => boolean, what: string, ms = 10_000) {
  for (const end = performance.now() + ms; performance.now() < end; ) {
    if (check()) return;
    await wait(50);
  }
  throw new Error(`never: ${what} (Claude Code's card: ${JSON.stringify(claudeCard())})`);
}

const button = (name: string) =>
  [...document.querySelectorAll("button")].find((element) => text(element) === name);

/**
 * Presses Sign in, then takes the pointer away: Waiting… takes its place, and under a pointer
 * left there it reads Cancel (hovered) as soon as the browser notices, which may be before the
 * card is read (#455).
 */
async function signIn() {
  await fixtureClick(button("Sign in")!);
  await fixtureInput({ type: "pointer", x: 0, y: 0 });
}

/** The line under the title, the footer's count and whether Continue can be pressed. */
function step() {
  return {
    subtitle: text(document.querySelector("header p")),
    count: text(document.querySelector('footer [role="status"]')),
    continue: !document.querySelector<HTMLButtonElement>("footer button:last-child")?.disabled,
  };
}

function Fixture() {
  const [result, setResult] = useState<string | null>(null);
  useEffect(() => {
    if (!params.has("drive")) return;
    void (async () => {
      const seen: Record<string, unknown> = {};
      try {
        await until(() => cards().length === 2, "two cards");
        seen.first = { cards: [card(0), card(1)], ...step() };

        if (params.get("codex") === "signedOut") {
          // Enter is Continue's alone: on a card's Sign in it neither signs in nor, held back, goes on.
          (button("Sign in") as HTMLButtonElement).focus();
          await fixtureInput({ type: "key", key: "Enter" });
          await wait(200);
          seen.enter = { continued, asked };
          setResult(JSON.stringify(seen));
          return;
        }

        if (params.has("code")) {
          // The browser couldn't hand the sign-in back: its code page is offered.
          claudeAt({ type: "signingIn" }, CODE_PAGE);
          await until(() => form() !== null, "the code field");
          const field = form()!.querySelector("input")!;
          const send = () => [...form()!.querySelectorAll("button")].find((element) => text(element) === "Send")!;
          seen.offered = { ...claudeCard(), says: text(form()!.querySelector("p")), placeholder: field.placeholder, send: !send().disabled };
          await fixtureClick(button("Open the code page")!);
          await fixtureClick(field);
          await fixtureInput({ type: "type", text: "abc#xyz" });
          seen.typed = { send: !send().disabled };
          // Enter sends the code: the field's own, not the step's Continue.
          await fixtureInput({ type: "key", key: "Enter" });
          await until(() => asked.some((what) => what.startsWith("sendConnectCode")), "the code sent");
          await until(() => field.value === "", "the field cleared");
          seen.sent = { asked, continued };
          claudeAt({ type: "connected" });
          claudeStatus = { ...claudeStatus, loggedIn: true, authMethod: "claude.ai", plan: "max" };
          show();
          await until(() => claudeCard().action === "Connected", "connected");
          seen.connected = { ...claudeCard(), field: form() !== null };
          setResult(JSON.stringify(seen));
          return;
        }

        // Install is Connect: the agent's installer, then its sign-in, the card waiting.
        await fixtureClick(button("Install")!);
        // The pointer away, so Waiting… doesn't read Cancel under it.
        await fixtureInput({ type: "pointer", x: 0, y: 0 });
        await until(() => claudeCard().line === "Installing Claude Code…", "installing");
        seen.installing = claudeCard();

        if (params.has("nodownloader")) {
          // Neither curl nor wget to download the installer: the card says so and links the guide.
          claudeAt({
            type: "failed",
            step: "install",
            error: NO_DOWNLOADER,
            reason: "noDownloader",
          });
          await until(() => claudeCard().line === NO_DOWNLOADER, "the missing downloader");
          const element = cards().find((each) => text(each.querySelector("p")) === "Claude Code")!;
          const line = element.querySelector('[data-slot="agent-line"]')!;
          const guide = element.querySelector('[data-slot="connect-guide"] button');
          seen.noDownloader = {
            line: text(line),
            wraps: getComputedStyle(line).whiteSpace !== "nowrap",
            guide: text(guide),
            buttons: [...element.querySelectorAll("button")].map((each) => text(each)),
          };
          await fixtureClick(guide!);
          await until(() => asked.some((what) => what.startsWith("openUrl")), "the guide opened");
          seen.asked = asked;
          setResult(JSON.stringify(seen));
          return;
        }

        const path = platform === "windows" ? String.raw`C:\Users\me\.local\bin\claude.exe` : "/home/me/.local/bin/claude";
        // Installed: a check finds it, signed out, as its sign-in starts.
        claudeStatus = { ...claudeStatus, path, version: "2.1.306" };
        show();
        claudeAt({ type: "signingIn" });
        await until(() => claudeCard().line === "Complete sign-in in your browser", "signing in");
        seen.waiting = claudeCard();

        if (params.has("install")) {
          claudeAt({ type: "connected" });
          claudeStatus = { ...claudeStatus, loggedIn: true, authMethod: "claude.ai", plan: "max" };
          show();
          await until(() => claudeCard().action === "Connected", "connected");
          seen.connected = claudeCard();
          seen.asked = asked;
          setResult(JSON.stringify(seen));
          return;
        }

        // Hovered, Waiting… reads Cancel in the same room, and stops the sign-in.
        const waitButton = document.querySelector('[data-slot="agent-wait"]')!;
        const before = waitButton.getBoundingClientRect();
        await fixtureInput({ type: "pointer", x: before.x + before.width / 2, y: before.y + before.height / 2 });
        await until(() => claudeCard().action === "Cancel", "Cancel on hover");
        const after = waitButton.getBoundingClientRect();
        seen.hover = { ...claudeCard(), label: waitButton.getAttribute("aria-label"), moved: before.width !== after.width || before.x !== after.x };
        await fixtureClick(waitButton);
        await until(() => claudeCard().action === "Sign in", "cancelled");
        seen.cancelled = claudeCard();

        await signIn();
        await until(() => claudeCard().action === "Waiting…", "waiting after the cancel");

        claudeAt({ type: "failed", step: "signIn", error: "The sign-in didn't finish. Try again." });
        await until(() => claudeCard().action === "Sign in", "the failure");
        seen.failed = claudeCard();

        await signIn();
        await until(() => claudeCard().action === "Waiting…", "waiting again");
        claudeAt({ type: "connected" });
        claudeStatus = { ...claudeStatus, loggedIn: true, authMethod: "claude.ai", plan: "max" };
        show();
        await until(() => claudeCard().action === "Connected", "connected");
        seen.connected = { cards: [card(0), card(1)], ...step() };
        seen.asked = asked;
        // Words a non-developer never sees on this step.
        const words = document.body.textContent ?? "";
        seen.jargon = ["CLI", "terminal", "Terminal", "OpenAI", "ChatGPT", "npm", "PATH"].filter((word) => words.includes(word));
      } catch (error) {
        seen.error = String(error);
      }
      setResult(JSON.stringify(seen));
    })();
  }, []);
  return (
    <TooltipProvider>
      {/* As OnboardingDialog holds it. */}
      <Dialog open>
        <DialogContent
          showCloseButton={false}
          aria-describedby={undefined}
          className="bg-background start-0 top-0 flex h-full w-full max-w-none translate-x-0 translate-y-0 flex-col gap-0 overflow-hidden rounded-none p-0 ring-0 rtl:translate-x-0"
          onOpenAutoFocus={(event) => {
            event.preventDefault();
            (event.currentTarget as HTMLElement | null)?.focus();
          }}
        >
          <div className="h-titlebar shrink-0" />
          <AgentsStep onBack={() => {}} onContinue={() => continued++} />
        </DialogContent>
      </Dialog>
      {result && <pre id="onboarding-agents-result" className="fixed bottom-0 z-50">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
