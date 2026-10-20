import { useEffect, useRef, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import {
  SettingsButton,
  SettingsCard,
  SettingsPage,
  SettingsRow,
  SettingsSection,
  SettingsSelect,
} from "@/app/settings/parts";
import { cleanWords, languageChoices, microphoneChoices } from "@/app/settings/voice";
import { ProgressLine } from "@/components/assistant-ui/elements/progress-line";
import { Plus, Trash } from "@/components/icons";
import { formatBytes } from "@/lib/format";
import {
  cancelSpeechModelDownload,
  downloadSpeechModel,
  refreshSpeechModel,
  useSpeechModel,
} from "@/state/dictation";
import { setSetting } from "@/state/settings";
import { removeSpeechModel } from "@/state/storage";
import { useApp } from "@/state/store";

/** The Voice page's rows, for the page and for Settings search. */
export const VOICE_ROWS = {
  microphone: { label: "Microphone", description: "Used for dictation" },
  language: { label: "Language", description: "The language you dictate in" },
  words: { label: "Word list", description: "Names and terms dictation should recognise" },
  model: { label: "Speech model", description: "Dictation runs on this computer with this model" },
} as const;

/**
 * Settings › Voice: the microphone and language dictation uses, words dictation should
 * recognise, and the speech model (downloaded, or not yet). Read aloud speaks in the system's
 * default voice, so it has nothing to set here.
 */
export function VoicePage() {
  return (
    <SettingsPage title="Voice">
      <SettingsSection title="General">
        <SettingsCard>
          <MicrophoneRow />
          <LanguageRow />
        </SettingsCard>
      </SettingsSection>
      <SettingsSection title="Dictation">
        <div className="flex flex-col gap-4">
          <WordList />
          <SettingsCard>
            <SpeechModelRow />
          </SettingsCard>
        </div>
      </SettingsSection>
    </SettingsPage>
  );
}

/** The webview's audio inputs, kept up to date as they come and go. */
function useMicrophones(): MediaDeviceInfo[] {
  const [devices, setDevices] = useState<MediaDeviceInfo[]>([]);
  useEffect(() => {
    const media = navigator.mediaDevices;
    if (typeof media?.enumerateDevices !== "function") return;
    let current = true;
    const read = () =>
      void media.enumerateDevices().then(
        (found) => current && setDevices(found),
        () => {},
      );
    read();
    media.addEventListener("devicechange", read);
    return () => {
      current = false;
      media.removeEventListener("devicechange", read);
    };
  }, []);
  return devices;
}

function MicrophoneRow() {
  const chosen = useApp((s) => s.settings.dictationMicrophone);
  const devices = useMicrophones();
  const save = useAction();
  const choices = microphoneChoices(devices, chosen);
  const unnamed = devices.some((device) => device.kind === "audioinput" && !device.label);
  return (
    <SettingsRow
      label={VOICE_ROWS.microphone.label}
      description={unnamed ? `${VOICE_ROWS.microphone.description}; names show once you have dictated` : VOICE_ROWS.microphone.description}
      error={save.error}
    >
      <SettingsSelect
        label={VOICE_ROWS.microphone.label}
        value={chosen}
        options={choices}
        onChange={(value) => save.run(() => setSetting("dictationMicrophone", value))}
      />
    </SettingsRow>
  );
}

function LanguageRow() {
  const chosen = useApp((s) => s.settings.dictationLanguage);
  const { status } = useSpeechModel();
  const save = useAction();
  const choices = languageChoices(status?.languages ?? []);
  // A language the engine doesn't know (an older choice) is auto-detected.
  const value = choices.some((choice) => choice.value === chosen) ? chosen : "";
  return (
    <SettingsRow label={VOICE_ROWS.language.label} description={VOICE_ROWS.language.description} error={save.error}>
      <SettingsSelect
        label={VOICE_ROWS.language.label}
        value={value}
        options={choices}
        disabled={!status}
        onChange={(next) => save.run(() => setSetting("dictationLanguage", next))}
      />
    </SettingsRow>
  );
}

/** The words dictation should recognise: one field each, saved as they are left. */
function WordList() {
  const saved = useApp((s) => s.settings.dictationWords);
  const [words, setWords] = useState<string[]>(saved);
  const [shown, setShown] = useState(saved);
  // A change saved elsewhere (another window) shows here; the echo of this page's own save
  // leaves the fields alone (one may just have been added).
  if (shown !== saved) {
    setShown(saved);
    if (saved.join("\n") !== cleanWords(words).join("\n")) setWords(saved);
  }
  const save = useAction();
  // Set by Add word: the new field takes the focus once it shows.
  const focusNew = useRef(false);
  const commit = (next: string[]) => {
    const clean = cleanWords(next);
    setWords(clean);
    if (clean.join("\n") !== saved.join("\n")) save.run(() => setSetting("dictationWords", clean));
  };
  return (
    <SettingsCard>
      <SettingsRow label={VOICE_ROWS.words.label} description={VOICE_ROWS.words.description} error={save.error}>
        <SettingsButton
          onClick={() => {
            focusNew.current = true;
            setWords((list) => [...list, ""]);
          }}
        >
          <Plus aria-hidden className="size-icon-sm" />
          Add word
        </SettingsButton>
      </SettingsRow>
      {words.length > 0 && (
        <ul data-slot="word-list" className="flex flex-col gap-2 px-4 pt-1 pb-3">
          {words.map((word, index) => (
            <li key={index} className="flex items-center gap-2">
              <input
                ref={(field) => {
                  if (field && focusNew.current && index === words.length - 1) {
                    focusNew.current = false;
                    field.focus();
                  }
                }}
                value={word}
                aria-label={`Word ${index + 1}`}
                placeholder="Jane Doe"
                spellCheck={false}
                autoComplete="off"
                className="border-foreground/16 bg-foreground/5 placeholder:text-foreground/40 rounded-nav h-8 min-w-0 flex-1 border px-3 text-sm outline-none focus:border-ring"
                onChange={(event) => setWords(words.map((old, at) => (at === index ? event.target.value : old)))}
                onBlur={() => commit(words)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") event.currentTarget.blur();
                  if (event.key === "Escape") {
                    // Esc leaves the field, not Settings.
                    event.stopPropagation();
                    event.currentTarget.blur();
                  }
                }}
              />
              <button
                type="button"
                aria-label={`Remove ${word.trim() || "word"}`}
                className="text-foreground/50 hover:text-foreground flex size-8 shrink-0 items-center justify-center"
                // The field keeps its focus: leaving it would save first and move the rows.
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => commit(words.filter((_, at) => at !== index))}
              >
                <Trash className="size-icon-sm" />
              </button>
            </li>
          ))}
        </ul>
      )}
    </SettingsCard>
  );
}

function SpeechModelRow() {
  const { status, model, modelError, dictating } = useSpeechModel();
  const remove = useAction();
  const size = status ? formatBytes(status.modelBytes) : "";
  const line = !status
    ? "Looking…"
    : model
      ? (
          <ProgressLine
            className="gap-1.5 pt-1 text-xs"
            title="Downloading…"
            done={model.received}
            total={model.total || status.modelBytes}
            meta={`${formatBytes(model.received)} of ${formatBytes(model.total || status.modelBytes)}`}
          />
        )
      : status.installed
        ? `Downloaded, ${size}; dictation runs on this computer with it`
        : `Not downloaded yet, ${size}; it downloads when you first dictate, or now`;
  return (
    <SettingsRow label={VOICE_ROWS.model.label} description={line} error={remove.error ?? (model ? null : modelError)}>
      {status && model && <SettingsButton onClick={cancelSpeechModelDownload}>Cancel</SettingsButton>}
      {status && !model && !status.installed && (
        <SettingsButton onClick={() => void downloadSpeechModel()}>Download</SettingsButton>
      )}
      {status && !model && status.installed && (
        <SettingsButton
          destructive
          disabled={remove.busy || dictating}
          onClick={() =>
            remove.run(async () => {
              try {
                await removeSpeechModel();
              } finally {
                refreshSpeechModel();
              }
            })
          }
        >
          {remove.busy ? "Removing…" : "Remove"}
        </SettingsButton>
      )}
    </SettingsRow>
  );
}
