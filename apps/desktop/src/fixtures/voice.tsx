/** Settings › Voice over a faked daemon, microphones and system voices. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { VoicePage } from "@/app/settings/VoicePage";
import { TooltipProvider } from "@/components/overlays/tooltip";
import type { DictationStatus, Request, Settings } from "@/ipc/generated";
import { onDictationDisconnected, onDictationUpdate, openMicrophone } from "@/state/dictation";
import { readAloud } from "@/state/readAloud";
import { useApp } from "@/state/store";

/*
 * ?drive=1 picks a microphone and a language, edits the word list, downloads (and cancels) and
 * removes the speech model, reads an answer aloud (in the system's default voice, whatever an
 * older version saved) and opens an unplugged microphone, then writes what it saw to #voice-result (VoicePageRender.test.ts reads it).
 */
const params = new URLSearchParams(location.search);
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent = "*, *::before, *::after { animation: none !important; transition: none !important; }";
  document.head.append(still);
}
const MB = 1024 ** 2;

useApp.setState((state) => ({ info: { ...state.info, platform: "macos" } as typeof state.info }));

let installed = false;
const status = (): DictationStatus => ({
  available: true,
  model: "ggml-base.bin",
  modelBytes: 141 * MB,
  installed,
  downloading: false,
  languages: [
    { code: "en", name: "english" },
    { code: "de", name: "german" },
    { code: "es", name: "spanish" },
  ],
});
const calls: string[] = [];
mockIPC((command, payload) => {
  if (command === "uninstall_app") return { identifier: "app.brigadier.fixture" };
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getDictation":
      return { method: req.method, dictation: status() };
    case "updateSettings":
      return { method: req.method, settings: req.settings as Settings };
    case "downloadDictationModel":
    case "cancelDictationDownload":
      calls.push(req.method);
      return { method: req.method };
    case "scanStorage":
      calls.push("scanStorage");
      return {
        method: req.method,
        report: {
          scanId: "scan-9",
          dataDir: "/data",
          totalBytes: 0,
          cleanableBytes: 0,
          projects: [],
          shared: [],
          kept: [],
          items: [
            {
              id: "model-1",
              category: "models",
              label: "Dictation speech model",
              path: "/data/models/whisper",
              bytes: 141 * MB,
              reason: "",
              checked: false,
              selectable: true,
              toTrash: false,
              badges: [],
            },
          ],
        },
      };
    case "cleanStorage":
      calls.push(`cleanStorage ${req.scanId} ${req.items.join(",")}`);
      installed = false;
      return { method: req.method, report: { removed: 1, reclaimedBytes: 141 * MB, trashedBytes: 0, failures: [], codexThreadsDeleted: 0 } };
    default:
      return { method: req.method };
  }
});

// The computer's microphones and voices.
const opened: unknown[] = [];
Object.defineProperty(navigator, "mediaDevices", {
  configurable: true,
  value: Object.assign(new EventTarget(), {
    enumerateDevices: async () => [
      { kind: "audioinput", deviceId: "default", label: "Default", groupId: "" },
      { kind: "audioinput", deviceId: "built-in", label: "MacBook Pro Microphone", groupId: "" },
      { kind: "audioinput", deviceId: "usb", label: "USB Microphone", groupId: "" },
    ],
    getUserMedia: async (constraints: MediaStreamConstraints) => {
      opened.push(constraints.audio);
      const audio = constraints.audio as MediaTrackConstraints;
      if ((audio.deviceId as { exact?: string } | undefined)?.exact === "gone") {
        throw Object.assign(new Error("no such device"), { name: "OverconstrainedError" });
      }
      return new MediaStream();
    },
  }),
});
const voices = [
  { voiceURI: "com.apple.voice.samantha", name: "Samantha", lang: "en-US" },
  { voiceURI: "com.apple.voice.daniel", name: "Daniel", lang: "en-GB" },
  { voiceURI: "com.apple.voice.thomas", name: "Thomas", lang: "fr-FR" },
] as unknown as SpeechSynthesisVoice[];
const spoken: { text: string; voice: string | null }[] = [];
// Utterances that take these voices, as the system's would.
class Utterance extends EventTarget {
  voice: SpeechSynthesisVoice | null = null;
  lang = "";
  constructor(public text: string) {
    super();
  }
}
window.SpeechSynthesisUtterance = Utterance as unknown as typeof SpeechSynthesisUtterance;
speechSynthesis.getVoices = () => voices;
speechSynthesis.speak = (utterance) => spoken.push({ text: utterance.text, voice: utterance.voice?.name ?? null });
speechSynthesis.cancel = () => {};

const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [
  ...root.querySelectorAll<T>(selector),
];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const row = (label: string) => all('[data-slot="settings-row"]').find((element) => element.dataset.setting === label);
const button = (name: string, root: ParentNode = document) =>
  all<HTMLButtonElement>("button", root).find((element) => text(element) === name || element.getAttribute("aria-label") === name);
const openMenu = (label: string): "menu" => {
  const trigger = button(label);
  trigger?.focus();
  trigger?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  return "menu";
};
const menuItems = () => all('[role="menu"][data-state="open"] [role^="menuitem"]').map(text);
const pickMenu = (name: string) =>
  (all('[role="menu"][data-state="open"] [role^="menuitem"]').find((item) => text(item).startsWith(name)) as HTMLElement | undefined)?.click();
const type = (input: HTMLInputElement | undefined, value: string) => {
  if (!input) return;
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
};
const words = () => all<HTMLInputElement>('[data-slot="word-list"] input');
const settings = () => useApp.getState().settings;
const modelLine = () => text(row("Speech model"));

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    // `?downloading`: the speech model a third downloaded, to look at.
    if (params.has("downloading")) {
      const timer = setTimeout(() => onDictationUpdate({ type: "download", received: 47 * MB, total: 141 * MB }), 300);
      return () => clearTimeout(timer);
    }
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    const steps: (() => void | "menu" | Promise<void>)[] = [
      () => {
        seen.sections = all("main section h2").map(text);
        seen.rows = all('[data-slot="settings-row"]').map((element) => element.dataset.setting);
        seen.modelFirst = modelLine();
        return openMenu("Microphone");
      },
      () => {
        seen.microphones = menuItems();
        pickMenu("USB Microphone");
      },
      () => openMenu("Language"),
      () => {
        seen.languages = menuItems();
        pickMenu("German");
      },
      () => button("Add word")?.click(),
      () => {
        seen.focused = document.activeElement === words()[0];
        type(words()[0], "  Brigadier ");
      },
      () => {
        // Add word right after leaving the field: the save's echo keeps the new field.
        words()[0]?.blur();
        button("Add word")?.click();
      },
      () => {
        seen.secondFocused = document.activeElement === words()[1];
        type(words()[1], "Jane Doe");
      },
      () => words()[1]?.blur(),
      () => {
        seen.wordsSaved = settings().dictationWords;
        button("Remove Brigadier")?.click();
      },
      () => {
        seen.wordsAfterRemove = { saved: settings().dictationWords, fields: words().map((input) => input.value) };
        button("Download")?.click();
      },
      () => {
        onDictationUpdate({ type: "download", received: 47 * MB, total: 141 * MB });
      },
      () => {
        seen.downloading = modelLine();
        seen.downloadBar = row("Speech model")?.querySelector('[role="progressbar"]')?.getAttribute("aria-valuenow") ?? null;
        button("Cancel")?.click();
        onDictationUpdate({ type: "downloadStopped", message: null });
      },
      () => {
        seen.cancelled = modelLine();
        button("Download")?.click();
      },
      () => onDictationUpdate({ type: "download", received: 10 * MB, total: 141 * MB }),
      // The connection drops, and with it the download's end: the line doesn't stay on Downloading.
      () => onDictationDisconnected(),
      () => {
        seen.afterDrop = modelLine();
        button("Download")?.click();
      },
      () => {
        installed = true;
        onDictationUpdate({ type: "downloaded" });
      },
      () => {
        seen.downloaded = modelLine();
        button("Remove")?.click();
      },
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 200));
        seen.removed = modelLine();
        // Settings saved by an earlier version still name a voice: Read aloud leaves it be.
        useApp.setState((state) => ({
          settings: { ...state.settings, readAloudVoice: "com.apple.voice.daniel" } as typeof state.settings,
        }));
        readAloud("answer-1", "**Done.** See [the docs](https://example.com).");
        seen.readAloud = spoken.at(-1);
        await openMicrophone("gone");
        seen.opened = opened;
      },
      () => {
        seen.settings = {
          microphone: settings().dictationMicrophone,
          language: settings().dictationLanguage,
        };
        seen.calls = calls;
        setResult(JSON.stringify(seen));
      },
    ];
    let stopped = false;
    const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
    const menuOpen = () => document.querySelector('[role="menu"][data-state="open"]') !== null;
    void (async () => {
      await wait(400);
      for (const step of steps) {
        if (stopped) return;
        const done = step();
        if (done instanceof Promise) await done;
        await wait(120);
        if (done === "menu") for (let tries = 0; tries < 40 && !menuOpen(); tries++) await wait(50);
      }
    })();
    return () => {
      stopped = true;
    };
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen">
        <main className="flex h-full min-w-0 flex-1">
          <div className="min-w-0 flex-1">
            <VoicePage />
          </div>
        </main>
      </div>
      {result && <pre id="voice-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
