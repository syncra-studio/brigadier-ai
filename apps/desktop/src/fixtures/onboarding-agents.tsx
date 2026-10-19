/**
 * Onboarding's agents step over a faked daemon: Claude Code isn't on this computer, Codex is
 * connected. ?drive=1 presses Claude Code's Connect and walks it through its install failing
 * (offline), Try again, the install, the browser sign-in, Cancel, Connect again and the sign-in
 * being noticed, writing what each column said at every step to #onboarding-agents-result
 * (OnboardingAgentsRender.test.ts reads it). The faked daemon moves on only when the drive says,
 * so every step is read as it stands. Without it the step stays as it opens, for screenshots;
 * ?plan= names the plan Codex reports.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { AgentsStep } from "@/app/onboarding/AgentsStep";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { TooltipProvider } from "@/components/ui/tooltip";
import { OVERVIEWS, status } from "@/fixtures/agents";
import { fixtureClick } from "@/fixtures/input";
import type { AgentConnect, ProviderOverview, Request } from "@/ipc/generated";
import { applyConnectEvents } from "@/state/connect";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}

const [claude, codex] = OVERVIEWS as [ProviderOverview, ProviderOverview];
/** Claude Code as the daemon finds it: nowhere at first. */
let claudeStatus = status("claude", { path: null, version: null, loggedIn: false, authMethod: null, plan: null });

function showClaude() {
  useApp.setState((state) => ({
    providers: {
      ...state.providers,
      view: {
        providers: [
          { ...claude, status: claudeStatus },
          { ...codex, status: status("codex", { plan: params.get("plan") ?? "pro" }) },
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
const claudeAt = (progress: AgentConnect["progress"]) => publish([{ provider: "claude", progress }]);

const asked: string[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "getProviders") return { method: req.method, view: useApp.getState().providers.view };
  if (req.method === "connectAgent" || req.method === "cancelConnect") {
    asked.push(`${req.method} ${req.provider}`);
    // As the daemon does: Connect begins with the install of an agent that is missing.
    if (req.method === "connectAgent") claudeAt(claudeStatus.path ? { type: "signingIn" } : { type: "installing" });
    else publish([]);
  }
  return { method: req.method };
});

useApp.setState({ connection: { status: "connected", daemon: null, reason: null } });
showClaude();

const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const columns = () => [...document.querySelectorAll('[data-slot="agent-column"]')];

/** What a column says: its name, its button and the line under it (and that line's buttons). */
function column(index: number) {
  const section = columns()[index];
  const line = section?.querySelector('[data-slot="connect-status"]');
  return {
    name: text(section?.querySelector("h2")),
    button: text(section?.querySelector("button")),
    enabled: !(section?.querySelector("button") as HTMLButtonElement | null)?.disabled,
    line: line ? text(line.querySelector('[role="status"]')) : text(section?.querySelector('[data-slot="agent-connection"]')),
    actions: [...(line?.querySelectorAll("button") ?? [])].map(text),
  };
}

async function until(check: () => boolean, what: string) {
  for (let tries = 0; tries < 100; tries++) {
    if (check()) return;
    await wait(50);
  }
  throw new Error(`never: ${what}`);
}

const button = (name: string) =>
  [...document.querySelectorAll("button")].find((element) => text(element) === name);

function Fixture() {
  const [result, setResult] = useState<string | null>(null);
  useEffect(() => {
    if (!params.has("drive")) return;
    void (async () => {
      const steps: Record<string, unknown> = {};
      try {
        await until(() => columns().length === 2, "two columns");
        steps.first = [column(0), column(1)];
        const continueEnabled = () => !(button("Continue") as HTMLButtonElement | undefined)?.disabled;
        steps.continueAtFirst = continueEnabled();

        await fixtureClick(columns()[0]!.querySelector("button")!);
        await until(() => column(0).line.startsWith("Installing"), "installing");
        steps.installing = column(0);

        // The installer can't reach the download server.
        claudeAt({ type: "failed", step: "install", error: "Brigadier couldn't download Claude Code. Check your internet connection, then try again." });
        await until(() => column(0).button === "Try again", "the failure");
        steps.failed = column(0);

        await fixtureClick(columns()[0]!.querySelector("button")!);
        await until(() => column(0).line.startsWith("Installing"), "installing again");
        // Installed: found, not signed in yet; its own sign-in opens in the browser.
        claudeStatus = { ...claudeStatus, path: "/Users/me/.local/bin/claude", version: "2.1.302" };
        showClaude();
        claudeAt({ type: "signingIn" });
        await until(() => column(0).line.startsWith("Finish"), "signing in");
        steps.signingIn = column(0);

        // Cancel, then Connect again: straight to the sign-in, which is noticed.
        await fixtureClick(button("Cancel")!);
        await until(() => column(0).button === "Connect", "cancelled");
        steps.cancelled = column(0);
        await fixtureClick(columns()[0]!.querySelector("button")!);
        await until(() => column(0).line.startsWith("Finish"), "signing in again");
        claudeAt({ type: "connected" });
        claudeStatus = { ...claudeStatus, loggedIn: true, authMethod: "claude.ai", plan: "max" };
        showClaude();
        await until(() => column(0).button === "Connected", "connected");
        steps.connected = column(0);
        steps.continueAtEnd = continueEnabled();
        steps.asked = asked;
        // Words a non-developer never sees on this step.
        const words = document.body.textContent ?? "";
        steps.jargon = ["CLI", "terminal", "Terminal", "OpenAI", "ChatGPT", "npm", "PATH"].filter((word) => words.includes(word));
      } catch (error) {
        steps.error = String(error);
      }
      setResult(JSON.stringify(steps));
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
          <AgentsStep name="Sam" onEscape={() => {}} onBack={() => {}} onContinue={() => {}} />
        </DialogContent>
      </Dialog>
      {result && <pre id="onboarding-agents-result" className="fixed bottom-0 z-50">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
