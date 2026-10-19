import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// The real page in Chromium over a faked daemon, microphones and voices.
test("voice settings: microphone, language, words and the speech model; Read aloud in the system voice", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "voice.html?drive=1", "voice-result", 15000)) as Record<string, unknown>;
  // Read aloud has nothing to choose: it speaks in the system's default voice.
  assert.deepEqual(seen.sections, ["General", "Dictation"]);
  assert.deepEqual(seen.rows, ["Microphone", "Language", "Word list", "Speech model"]);
  assert.deepEqual(seen.microphones, ["System default", "MacBook Pro Microphone", "USB Microphone"]);
  assert.deepEqual(seen.languages, ["Auto-detect", "English", "German", "Spanish"]);
  assert.equal(seen.focused, true);
  assert.equal(seen.secondFocused, true);
  assert.deepEqual(seen.wordsSaved, ["Brigadier", "Jane Doe"]);
  assert.deepEqual(seen.wordsAfterRemove, { saved: ["Jane Doe"], fields: ["Jane Doe"] });
  assert.equal(seen.modelFirst, "Speech modelNot downloaded yet, 141.0 MB; it downloads when you first dictate, or nowDownload");
  // The download reads as its progress: the bytes so far, of the whole, over a bar.
  assert.equal(seen.downloading, "Speech modelDownloading…47.0 MB of 141.0 MBCancel");
  assert.equal(seen.downloadBar, "33");
  assert.equal(seen.cancelled, seen.modelFirst);
  assert.equal(seen.afterDrop, seen.modelFirst);
  assert.equal(seen.downloaded, "Speech modelDownloaded, 141.0 MB; dictation runs on this computer with itRemove");
  assert.equal(seen.removed, seen.modelFirst);
  // Read aloud speaks an answer's words in the system's default voice, even where settings an
  // earlier version saved still name another.
  assert.deepEqual(seen.readAloud, { text: "Done. See the docs.", voice: null });
  // An unplugged microphone falls back to the system default.
  const opened = seen.opened as { deviceId?: unknown }[];
  assert.deepEqual(opened.map((audio) => audio.deviceId ?? null), [{ exact: "gone" }, null]);
  assert.deepEqual(seen.settings, { microphone: "usb", language: "de" });
  // Remove goes through Free up space's scan and clean, for the model item alone.
  assert.deepEqual(seen.calls, [
    "downloadDictationModel",
    "cancelDictationDownload",
    "downloadDictationModel",
    "downloadDictationModel",
    "scanStorage",
    "cleanStorage scan-9 model-1",
  ]);
});
