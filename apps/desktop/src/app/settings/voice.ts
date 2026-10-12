import type { DictationLanguage } from "@/ipc/generated";

/** A choice of a Voice page menu: "" is the system's default (or auto-detect). */
export type VoiceChoice = { value: string; label: string; hint?: string };

const capitalised = (name: string) => name.replace(/(^|[\s-])\p{L}/gu, (start) => start.toUpperCase());

/** Auto-detect, then the speech engine's languages by name. */
export function languageChoices(languages: readonly DictationLanguage[]): VoiceChoice[] {
  const named = languages
    .map((language) => ({ value: language.code, label: capitalised(language.name) }))
    .toSorted((a, b) => a.label.localeCompare(b.label));
  return [{ value: "", label: "Auto-detect" }, ...named];
}

/**
 * System default, then each microphone. Before Brigadier may use the microphone the system
 * doesn't name them, so they are numbered. The chosen one stays listed while unplugged.
 */
export function microphoneChoices(devices: readonly Pick<MediaDeviceInfo, "kind" | "deviceId" | "label">[], chosen: string): VoiceChoice[] {
  const inputs = devices.filter(
    (device) => device.kind === "audioinput" && device.deviceId && device.deviceId !== "default" && device.deviceId !== "communications",
  );
  const choices: VoiceChoice[] = [
    { value: "", label: "System default" },
    ...inputs.map((device, index) => ({ value: device.deviceId, label: device.label || `Microphone ${index + 1}` })),
  ];
  if (chosen && !inputs.some((device) => device.deviceId === chosen)) {
    choices.push({ value: chosen, label: "Not connected", hint: "The system default is used meanwhile." });
  }
  return choices;
}

/** The word list as saved: trimmed, no empty or repeated entries. */
export function cleanWords(words: readonly string[]): string[] {
  const seen = new Set<string>();
  const kept: string[] = [];
  for (const word of words) {
    const trimmed = word.trim();
    if (!trimmed || seen.has(trimmed.toLowerCase())) continue;
    seen.add(trimmed.toLowerCase());
    kept.push(trimmed);
  }
  return kept;
}
