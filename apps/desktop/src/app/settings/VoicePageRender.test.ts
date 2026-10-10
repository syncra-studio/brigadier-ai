import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// The real page in Chromium over a faked daemon, microphones and voices.
test("voice settings: microphone, language, voice, words and the speech model", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "voice.html?drive=1", "voice-result", 15000)) as Record<string, unknown>;
  assert.deepEqual(seen.sections, ["General", "Read aloud", "Dictation"]);
  assert.deepEqual(seen.rows, ["Microphone", "Language", "Voice", "Word list", "Speech model"]);
  assert.deepEqual(seen.microphones, ["System default", "MacBook Pro Microphone", "USB Microphone"]);
  assert.deepEqual(seen.languages, ["Auto-detect", "English", "German", "Spanish"]);
  assert.deepEqual(seen.voices, ["System default", "DanielBritish English", "SamanthaAmerican English", "ThomasFrench (France)"]);
  // Picking a voice says a sample in it.
  assert.deepEqual(seen.sample, { text: "This is how answers sound when they are read aloud.", voice: "Daniel" });
  assert.equal(seen.focused, true);
  assert.equal(seen.secondFocused, true);
  assert.deepEqual(seen.wordsSaved, ["Brigadier", "Jane Doe"]);
  assert.deepEqual(seen.wordsAfterRemove, { saved: ["Jane Doe"], fields: ["Jane Doe"] });
  assert.equal(seen.modelFirst, "Speech modelNot downloaded yet, 141.0 MB. It downloads when you first dictate, or now.Download");
  assert.equal(seen.downloading, "Speech modelDownloading… 47.0 MB of 141.0 MBCancel");
  assert.equal(seen.cancelled, seen.modelFirst);
  assert.equal(seen.downloaded, "Speech modelDownloaded, 141.0 MB. Dictation runs on this computer with this model.Remove");
  assert.equal(seen.removed, seen.modelFirst);
  // Read aloud speaks an answer's words in the chosen voice.
  assert.deepEqual(seen.readAloud, { text: "Done. See the docs.", voice: "Daniel" });
  // An unplugged microphone falls back to the system default.
  const opened = seen.opened as { deviceId?: unknown }[];
  assert.deepEqual(opened.map((audio) => audio.deviceId ?? null), [{ exact: "gone" }, null]);
  assert.deepEqual(seen.settings, { microphone: "usb", language: "de", voice: "com.apple.voice.daniel" });
  // Remove goes through Free up space's scan and clean, for the model item alone.
  assert.deepEqual(seen.calls, [
    "downloadDictationModel",
    "cancelDictationDownload",
    "downloadDictationModel",
    "scanStorage",
    "cleanStorage scan-9 model-1",
  ]);
});
