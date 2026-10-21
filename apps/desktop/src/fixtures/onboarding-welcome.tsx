/**
 * The first launch's intro, full window over a faked app. It writes the title it shows (its name
 * and its lines) to #onboarding-welcome-result (OnboardingWelcomeRender.test.ts reads it).
 * ?step=name shows the name instead; with ?drive=1 it types spaces, then a name, presses Return
 * and writes what Continue and the field did to #onboarding-name-result. The intro with ?drive=1
 * settles it (Escape), presses Enter with the focus off its button and writes what Continue did
 * to #onboarding-intro-result. With ?corners=1 it settles the intro and writes how opaque the
 * canvas is in the window's corner pixels, in the middle of each edge and at its centre to
 * #onboarding-corners-result; ?platform= names the platform (macOS unless given).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { Welcome } from "@/app/setup/onboarding/Welcome";
import { Dialog, DialogContent } from "@/components/overlays";
import { fixtureInput } from "@/fixtures/input";

mockIPC(() => null);

const params = new URLSearchParams(location.search);
document.documentElement.dataset.platform = params.get("platform") ?? "macos";
if (params.has("corners")) {
  // The canvas keeps what it drew, so the drive can read it back after the frame.
  const getContext = HTMLCanvasElement.prototype.getContext;
  HTMLCanvasElement.prototype.getContext = function (this: HTMLCanvasElement, kind: string, options?: object) {
    return getContext.call(this, kind, kind === "webgl2" ? { ...options, preserveDrawingBuffer: true } : options);
  } as typeof getContext;
}
const step = params.get("step") === "name" ? "name" : "intro";
const named: string[] = [];
let continued = 0;
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

const continueButton = () => document.querySelector<HTMLElement>(".welcome-continue");

/** Settles the intro, then presses Enter with the focus elsewhere: Continue's, wherever it is. */
async function driveIntro(): Promise<string> {
  for (let tries = 0; !document.querySelector(".welcome-continue") && tries < 50; tries++) await wait(50);
  await fixtureInput({ type: "key", key: "Escape" });
  for (let tries = 0; continueButton()?.inert !== false && tries < 50; tries++) await wait(50);
  const label = continueButton()?.textContent;
  (document.activeElement as HTMLElement | null)?.blur();
  await fixtureInput({ type: "key", key: "Enter" });
  await wait(600);
  return JSON.stringify({ label, continued });
}

/** Settles the intro, then reads the canvas's opacity (0–255) in its outermost pixels: the
 *  corners' and the middle of each edge (under the title bar, the sides, the bottom). */
async function driveCorners(): Promise<string> {
  for (let tries = 0; !document.querySelector(".welcome-continue") && tries < 50; tries++) await wait(50);
  await fixtureInput({ type: "key", key: "Escape" });
  await wait(1000);
  const canvas = document.querySelector<HTMLCanvasElement>(".welcome-canvas");
  const copy = document.createElement("canvas");
  copy.width = canvas?.width ?? 0;
  copy.height = canvas?.height ?? 0;
  const context = copy.getContext("2d");
  if (!canvas || !context || !copy.width) return JSON.stringify({ drawn: false });
  context.drawImage(canvas, 0, 0);
  const alpha = (x: number, y: number) => context.getImageData(x, y, 1, 1).data[3];
  const [right, bottom] = [copy.width - 1, copy.height - 1];
  const [middleX, middleY] = [Math.floor(copy.width / 2), Math.floor(copy.height / 2)];
  return JSON.stringify({
    corners: [alpha(0, 0), alpha(right, 0), alpha(0, bottom), alpha(right, bottom)],
    edges: [alpha(middleX, 0), alpha(0, middleY), alpha(right, middleY), alpha(middleX, bottom)],
    centre: alpha(middleX, middleY),
  });
}

function Fixture() {
  const [result, setResult] = useState("");
  const [nameResult, setNameResult] = useState("");
  const [introResult, setIntroResult] = useState("");
  const [cornersResult, setCornersResult] = useState("");
  useEffect(() => {
    if (params.has("corners")) {
      void driveCorners().then(setCornersResult);
      return;
    }
    if (step === "name" && params.has("drive")) void driveName().then(setNameResult);
    if (step === "intro" && params.has("drive")) void driveIntro().then(setIntroResult);
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
              onContinue={() => continued++}
              onName={async (name) => {
                named.push(name);
              }}
            />
          </div>
        </DialogContent>
      </Dialog>
      {result && <pre id="onboarding-welcome-result">{result}</pre>}
      {nameResult && <pre id="onboarding-name-result">{nameResult}</pre>}
      {introResult && <pre id="onboarding-intro-result">{introResult}</pre>}
      {cornersResult && <pre id="onboarding-corners-result">{cornersResult}</pre>}
    </>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
