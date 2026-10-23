import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ReportsPage } from "@/app/settings/ReportsPage";
import { TooltipProvider } from "@/components/overlays";
import artifact from "@/fixtures/continuity-artifact.json";

let calls = 0;
mockIPC((command) => {
  if (command !== "get_continuity_report") return null;
  calls++;
  if (calls === 3) throw new Error("GitHub CLI is offline. Try Refresh.");
  return calls === 1 ? artifact : { ...artifact, source: "fedcba9876543210" };
});

const wait = () => new Promise((resolve) => setTimeout(resolve, 150));
const page = () => document.querySelector('[data-slot="settings-page"]')?.textContent ?? "";
const refresh = () => Array.from(document.querySelectorAll("button")).find((b) => b.textContent === "Refresh")?.click();

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (!new URLSearchParams(location.search).has("drive")) return;
    let stopped = false;
    void (async () => {
      for (let tries = 0; tries < 60 && !page().includes("abcdef1234"); tries++) await wait();
      const initial = page();
      refresh();
      for (let tries = 0; tries < 60 && !page().includes("fedcba9876"); tries++) await wait();
      const refreshed = page();
      refresh();
      for (let tries = 0; tries < 60 && !page().includes("offline"); tries++) await wait();
      if (!stopped) setResult(JSON.stringify({ initial, refreshed, failed: page(), calls }));
    })();
    return () => { stopped = true; };
  }, []);
  return <TooltipProvider>
    <div className="bg-background text-foreground h-screen"><ReportsPage /></div>
    {result && <pre id="reports-result">{result}</pre>}
  </TooltipProvider>;
}

createRoot(document.getElementById("root")!).render(<Fixture />);
