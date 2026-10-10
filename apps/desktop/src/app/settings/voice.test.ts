import assert from "node:assert/strict";
import { test } from "node:test";

import { cleanWords, languageChoices, microphoneChoices, voiceChoices } from "@/app/settings/voice";

test("languages: auto-detect first, then the engine's by name", () => {
  const choices = languageChoices([
    { code: "en", name: "english" },
    { code: "de", name: "german" },
    { code: "haw", name: "hawaiian" },
  ]);
  assert.deepEqual(choices, [
    { value: "", label: "Auto-detect" },
    { value: "en", label: "English" },
    { value: "de", label: "German" },
    { value: "haw", label: "Hawaiian" },
  ]);
});

test("microphones: system default, each input numbered until named, an unplugged choice kept", () => {
  const devices = [
    { kind: "audioinput", deviceId: "default", label: "" },
    { kind: "audioinput", deviceId: "a", label: "" },
    { kind: "videoinput", deviceId: "cam", label: "" },
    { kind: "audioinput", deviceId: "b", label: "" },
  ] as const;
  assert.deepEqual(
    microphoneChoices(devices, "").map((choice) => [choice.value, choice.label]),
    [
      ["", "System default"],
      ["a", "Microphone 1"],
      ["b", "Microphone 2"],
    ],
  );
  const named = [{ kind: "audioinput", deviceId: "a", label: "MacBook Pro Microphone" }] as const;
  assert.deepEqual(
    microphoneChoices(named, "gone").map((choice) => [choice.value, choice.label]),
    [
      ["", "System default"],
      ["a", "MacBook Pro Microphone"],
      ["gone", "Not connected"],
    ],
  );
});

test("voices: system default, the app's language first, a missing choice kept", () => {
  const voices = [
    { voiceURI: "fr.thomas", name: "Thomas", lang: "fr-FR" },
    { voiceURI: "en.samantha", name: "Samantha", lang: "en-US" },
    { voiceURI: "en.daniel", name: "Daniel", lang: "en-GB" },
  ];
  const choices = voiceChoices(voices, "gone", "en-US");
  assert.deepEqual(
    choices.map((choice) => [choice.value, choice.label, choice.hint]),
    [
      ["", "System default", undefined],
      ["en.daniel", "Daniel", "British English"],
      ["en.samantha", "Samantha", "American English"],
      ["fr.thomas", "Thomas", "French (France)"],
      ["gone", "Not installed", "The system default is used meanwhile."],
    ],
  );
});

test("the word list is saved trimmed, without empty or repeated words", () => {
  assert.deepEqual(cleanWords([" Brigadier ", "", "brigadier", "Jane Doe", "  "]), ["Brigadier", "Jane Doe"]);
});
