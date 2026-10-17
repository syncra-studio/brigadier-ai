/**
 * The first launch's intro, full window over a faked app. It writes the title it shows (its name
 * and its lines) to #onboarding-welcome-result (OnboardingWelcomeRender.test.ts reads it).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { Welcome } from "@/app/onboarding/Welcome";
import { Dialog, DialogContent } from "@/components/ui/dialog";

mockIPC(() => null);

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    // The dialog's portal mounts after this does: look until the title is there.
    const look = setInterval(() => {
      const title = document.querySelector("h1.welcome-title");
      if (!title) return;
      clearInterval(look);
      setResult(
        JSON.stringify({
          name: title.getAttribute("aria-label"),
          lines: [...title.querySelectorAll(".welcome-title-line")].map((line) => line.textContent),
        }),
      );
    }, 50);
    return () => clearInterval(look);
  }, []);
  return (
    <>
      <Dialog open>
        <DialogContent
          showCloseButton={false}
          className="bg-background start-0 top-0 flex h-full w-full max-w-none translate-x-0 translate-y-0 flex-col gap-0 overflow-hidden rounded-none p-0 ring-0"
        >
          <div className="relative flex h-full min-h-0 flex-col">
            <Welcome step="intro" name="" onSound={() => {}} onContinue={() => {}} onName={async () => {}} />
          </div>
        </DialogContent>
      </Dialog>
      {result && <pre id="onboarding-welcome-result">{result}</pre>}
    </>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
