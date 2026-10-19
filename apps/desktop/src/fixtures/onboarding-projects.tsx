/**
 * Onboarding's projects step over a faked daemon. ?drive=1 notes what comes selected, adds a
 * folder by hand, clicks Select all then Select none, presses Enter, and writes what it saw and
 * what the daemon was asked to #onboarding-projects-result (OnboardingProjectsRender.test.ts
 * reads it).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ProjectsStep } from "@/app/onboarding/ProjectsStep";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { TooltipProvider } from "@/components/ui/tooltip";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import type { ProjectCandidate, Request } from "@/ipc/generated";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);

const candidates: ProjectCandidate[] = [
  { path: "/w/storefront-web", name: "storefront-web", providers: ["claude"], sessions: 9, lastActiveMs: Date.now() - 3_600_000, projectId: null },
  { path: "/w/billing", name: "billing", providers: ["codex"], sessions: 4, lastActiveMs: Date.now() - 86_400_000, projectId: null },
];

const calls: string[] = [];
mockIPC((command, payload) => {
  if (command === "pick_folder") return "/w/hand-picked";
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

const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const button = (name: string) => [...document.querySelectorAll("button")].find((b) => text(b).startsWith(name));
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function Fixture() {
  const [result, setResult] = useState<string | null>(null);
  useEffect(() => {
    if (!params.has("drive")) return;
    void (async () => {
      for (let tries = 0; !button("Select none") && tries < 50; tries++) await wait(100);
      const status = () => text(document.querySelector('[role="status"]'));
      const first = status();
      const folder = button("Add a folder");
      if (folder) await fixtureClick(folder);
      for (let tries = 0; !document.body.textContent?.includes("hand-picked") && tries < 50; tries++) await wait(100);
      const header = text(button("Add a folder")?.parentElement?.previousElementSibling).replace(/\s+/g, " ");
      const afterAdd = status();
      const all = button("Select all");
      if (all) await fixtureClick(all);
      await wait(200);
      const none = button("Select none");
      if (none) await fixtureClick(none);
      await wait(200);
      const selected = status();
      await fixtureInput({ type: "key", key: "Enter" });
      await wait(300);
      setResult(JSON.stringify({ first, header, afterAdd, selected, asked: calls.filter((method) => method !== "findProjects") }));
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
          <ProjectsStep onBack={() => {}} />
        </DialogContent>
      </Dialog>
      {result && <pre id="onboarding-projects-result" className="fixed bottom-0 z-50">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
