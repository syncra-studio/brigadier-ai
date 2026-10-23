/**
 * Onboarding's projects step over a faked daemon. ?drive=1 notes what comes selected, adds a
 * folder by hand, unticks a card, presses the Select toggle twice, presses Enter on a card, and writes
 * what it saw and what the daemon was asked to #onboarding-projects-result
 * (OnboardingProjectsRender.test.ts reads it). ?many=1 finds a dozen projects, for screenshots;
 * ?mac=1 reads as on a Mac.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ProjectsStep } from "@/app/setup/onboarding/ProjectsStep";
import { Dialog, DialogContent, TooltipProvider } from "@/components/overlays";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import type { ProjectCandidate, Request } from "@/ipc/generated";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);

const candidates: ProjectCandidate[] = [
  { path: "/w/storefront-web", name: "storefront-web", providers: ["claude"], sessions: 9, lastActiveMs: Date.now() - 3_600_000, projectId: null },
  { path: "/w/billing", name: "billing", providers: ["codex"], sessions: 4, lastActiveMs: Date.now() - 86_400_000, projectId: null },
];
if (params.has("many")) {
  for (let index = 0; index < 10; index++) {
    candidates.push({
      path: `/Users/me/Development/acme-studio/a-rather-long-project-folder-name-${index}`,
      name: `acme-studio/a-rather-long-repository-name-${index}`,
      providers: index % 2 ? ["claude", "codex"] : ["claude"],
      sessions: 12 * (index + 1),
      lastActiveMs: Date.now() - index * 7_200_000,
      projectId: null,
    });
  }
}

const calls: string[] = [];
mockIPC((command, payload) => {
  if (command === "pick_folder") return "/w/hand-picked";
  if (command === "warm_folder_picker") {
    calls.push(command);
    return null;
  }
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  calls.push(req.method);
  if (req.method === "findProjects") return { method: req.method, candidates };
  if (req.method === "checkFolder") {
    return { method: req.method, check: { kind: "repo", root: "/w/hand-picked", name: "hand-picked", nested: false, projectId: null } };
  }
  return { method: req.method };
});

useApp.setState({ connection: { status: "connected", daemon: null, reason: null } });
// ?mac=1 reads as on a Mac ("this Mac", ⌘ shortcuts), for screenshots.
if (params.has("mac")) useApp.setState((state) => ({ info: { ...state.info, platform: "macos" } as typeof state.info }));

const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const button = (name: string) => [...document.querySelectorAll("button")].find((b) => text(b).startsWith(name));
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function Fixture() {
  const [result, setResult] = useState<string | null>(null);
  useEffect(() => {
    if (!params.has("drive")) return;
    void (async () => {
      for (let tries = 0; !button("Select") && tries < 50; tries++) await wait(100);
      const toggle = () => button("Select")!;
      /** The footer's count, the Select toggle's label and the step's action. */
      const state = () => ({
        count: text(document.querySelector('footer [role="status"]')),
        toggle: text(toggle().firstElementChild),
        action: text(document.querySelector("footer button:last-child")),
      });
      const first = state();
      const folder = button("Add folder");
      if (folder) await fixtureClick(folder);
      for (let tries = 0; !document.body.textContent?.includes("hand-picked") && tries < 50; tries++) await wait(100);
      const header = text(button("Add folder")?.parentElement?.previousElementSibling).replace(/\s+/g, " ");
      const afterAdd = state();
      const card = document.querySelector('[role="checkbox"][aria-label="billing"]');
      if (card) await fixtureClick(card);
      await wait(100);
      const ticked = state();
      // The toggle's place, which must not move as it switches.
      const at = toggle().getBoundingClientRect().right;
      await fixtureClick(toggle());
      await wait(100);
      const all = state();
      await fixtureClick(toggle());
      await wait(100);
      const none = { ...state(), moved: toggle().getBoundingClientRect().right !== at };
      // Enter on a card goes on and leaves the card as it was (Space ticks it).
      (document.querySelector('[role="checkbox"][aria-label="billing"]') as HTMLElement).focus();
      await fixtureInput({ type: "key", key: "Enter" });
      await wait(300);
      const enter = { billing: document.querySelector('[role="checkbox"][aria-label="billing"]')?.getAttribute("aria-checked") };
      setResult(
        JSON.stringify({ first, header, afterAdd, ticked, all, none, enter, asked: calls.filter((method) => method !== "findProjects") }),
      );
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
          <ProjectsStep onBack={() => {}} />
        </DialogContent>
      </Dialog>
      {result && <pre id="onboarding-projects-result" className="fixed bottom-0 z-50">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
