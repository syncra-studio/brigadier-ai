/**
 * The whole first-run setup over a faked daemon, opened at its agents with Claude Code
 * connected and nothing found in the history. ?drive=1 continues to the projects, presses
 * Continue without projects with the settings write failing, reads where the setup stands and
 * what it says, presses it again with the write going through, and writes all of it to
 * #onboarding-finish-result (OnboardingFinishRender.test.ts reads it).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { OnboardingDialog } from "@/app/onboarding/OnboardingDialog";
import { TooltipProvider } from "@/components/overlays/tooltip";
import { OVERVIEWS } from "@/fixtures/agents";
import { fixtureClick } from "@/fixtures/input";
import type { Request } from "@/ipc/generated";
import { revealApp } from "@/lib/splash";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
const view = { providers: OVERVIEWS, sessions: [], fixtures: [] };

/** Whether the settings write fails (the daemon slow or gone). */
let failing = true;
const writes: string[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "getProviders") return { method: req.method, view };
  if (req.method === "findProjects") return { method: req.method, candidates: [] };
  if (req.method === "updateSettings") {
    writes.push(failing ? "failed" : "saved");
    if (failing) throw new Error("the daemon did not answer in time");
    return { method: req.method, settings: req.settings };
  }
  return { method: req.method };
});

useApp.setState((state) => ({
  connection: { status: "connected", daemon: null, reason: null },
  catalogLoaded: true,
  settings: { ...state.settings, onboarded: false, introSeen: true, userName: "Alex" },
  providers: { ...state.providers, view },
}));
void revealApp();

const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const action = () => document.querySelector<HTMLButtonElement>('[role="dialog"] footer button:last-child');

async function until(check: () => boolean, what: string, ms = 10_000) {
  for (const end = performance.now() + ms; performance.now() < end; ) {
    if (check()) return;
    await wait(50);
  }
  throw new Error(`never: ${what}`);
}

/** The step showing, what it says went wrong and whether the setup is still open. */
function standing() {
  return {
    title: text(document.querySelector('[role="dialog"] h1')),
    error: text(document.querySelector('[role="dialog"] [role="alert"]')) || null,
    open: document.querySelector('[role="dialog"]') !== null,
  };
}

function Fixture() {
  const [result, setResult] = useState<string | null>(null);
  useEffect(() => {
    if (!params.has("drive")) return;
    void (async () => {
      const seen: Record<string, unknown> = {};
      try {
        await until(() => text(document.querySelector('[role="dialog"] h1')) === "Connect your agents", "the agents");
        await until(() => action()?.disabled === false, "Continue");
        await fixtureClick(action()!);
        await until(() => text(action()).startsWith("Continue without projects"), "the projects");
        await fixtureClick(action()!);
        await until(() => writes.length === 1, "the write");
        await wait(500);
        seen.failed = standing();
        failing = false;
        await fixtureClick(action()!);
        await until(() => !standing().open, "the setup closing");
        seen.saved = { ...standing(), onboarded: useApp.getState().settings.onboarded };
        seen.writes = writes;
      } catch (error) {
        seen.error = String(error);
        seen.at = standing();
      }
      setResult(JSON.stringify(seen));
    })();
  }, []);
  return (
    <TooltipProvider>
      <OnboardingDialog />
      {result && <pre id="onboarding-finish-result" className="fixed bottom-0 z-50">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
