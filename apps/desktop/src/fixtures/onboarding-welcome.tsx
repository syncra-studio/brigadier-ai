/**
 * The first launch's intro, full window over a faked app. It writes the title it shows (its name
 * and its lines) to #onboarding-welcome-result (OnboardingWelcomeRender.test.ts reads it).
 * ?step=name shows the name instead; with ?drive=1 it types spaces, then a name, presses Return
 * and writes what Continue and the field did to #onboarding-name-result.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { Welcome } from "@/app/onboarding/Welcome";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { fixtureInput } from "@/fixtures/input";

mockIPC(() => null);

const params = new URLSearchParams(location.search);
const step = params.get("step") === "name" ? "name" : "intro";
const named: string[] = [];
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const continueOn = () => !(document.querySelector('button[type="submit"]') as HTMLButtonElement | null)?.disabled;

/** Types into the name field as a keyboard would, then reads what Continue and the field say. */
async function driveName(): Promise<string> {
  for (let tries = 0; !document.querySelector("input") && tries < 50; tries++) await wait(50);
  await wait(100);
  const field = document.querySelector("input");
  const seen: Record<string, unknown> = {
    focused: document.activeElement === field,
    placeholder: field?.placeholder,
    empty: continueOn(),
  };
  await fixtureInput({ type: "type", text: "   " });
  seen.spaces = continueOn();
  await fixtureInput({ type: "type", text: "Alex" });
  seen.typed = continueOn();
  seen.hint = document.querySelector('button[type="submit"] kbd')?.textContent;
  await fixtureInput({ type: "key", key: "Enter" });
  await wait(100);
  seen.named = named;
  return JSON.stringify(seen);
}

function Fixture() {
  const [result, setResult] = useState("");
  const [nameResult, setNameResult] = useState("");
  useEffect(() => {
    if (step === "name" && params.has("drive")) void driveName().then(setNameResult);
  }, []);
  useEffect(() => {
    if (step !== "intro") return;
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
            <Welcome
              step={step}
              name=""
              onSound={() => {}}
              onContinue={() => {}}
              onName={async (name) => {
                named.push(name);
              }}
            />
          </div>
        </DialogContent>
      </Dialog>
      {result && <pre id="onboarding-welcome-result">{result}</pre>}
      {nameResult && <pre id="onboarding-name-result">{nameResult}</pre>}
    </>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
