import { useEffect, useSyncExternalStore } from "react";

import workletUrl from "@/app/conversation/dictation-worklet?worker&url";
import type { DictationStatus, DictationUpdate } from "@/ipc/generated";
import { request } from "@/ipc/client";
import { useApp } from "@/state/store";

/**
 * Dictation (the composer's Dictate button), run on this computer: the
 * microphone's audio goes to brigadierd, whose speech engine turns it into text that lands at
 * the composer's caret. The speech model downloads on first use. One dictation runs at a time,
 * owned by one composer (its conversation, or the new chat).
 */

export type DictationPhase =
  | { type: "idle" }
  | { type: "downloading"; received: number; total: number }
  | { type: "starting" }
  | { type: "recording" }
  | { type: "transcribing" }
  | { type: "failed"; message: string };

type Snapshot = {
  /** Null until the daemon has said. */
  status: DictationStatus | null;
  phase: DictationPhase;
  /** The composer the dictation is for. */
  owner: string | null;
  /** The microphone's recent loudness, 0–1, oldest first (the waveform). */
  levels: number[];
  /** The speech model's download, however it was started, while it runs. */
  model: { received: number; total: number } | null;
  /** Why the last download from Settings › Voice stopped short. */
  modelError: string | null;
};

const LEVELS = 40;
const LEVEL_EVERY_MS = 50;

let snapshot: Snapshot = {
  status: null,
  phase: { type: "idle" },
  owner: null,
  levels: [],
  model: null,
  modelError: null,
};
const listeners = new Set<() => void>();

function set(patch: Partial<Snapshot>): void {
  snapshot = { ...snapshot, ...patch };
  for (const listener of listeners) listener();
}

function subscribeStore(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Puts `text` at a composer's caret; resolves once the composer has it. */
export type Insert = (text: string) => Promise<void>;
const inserters = new Map<string, Insert>();

/** A composer's editor says how to put text at its caret. */
export function registerInserter(owner: string, insert: Insert): () => void {
  inserters.set(owner, insert);
  return () => {
    if (inserters.get(owner) === insert) inserters.delete(owner);
  };
}

/** The recording under way. */
type Capture = {
  stream: MediaStream;
  context: AudioContext;
  worklet: AudioWorkletNode;
  analyser: AnalyserNode;
  meter: ReturnType<typeof setInterval>;
  /** Set once the daemon has started the dictation. */
  id: string | null;
  /** Audio posted before the dictation had an id. */
  early: Int16Array[];
  /** The audio sent so far, in order. */
  sending: Promise<void>;
  /** Cancelled: its audio goes nowhere. */
  dropped: boolean;
  /** Called when the worklet has posted its last samples. */
  flushed: (() => void) | null;
};

let capture: Capture | null = null;
/** The dictation being transcribed, and what to do with its text when it comes. */
let onText: { id: string; deliver: (text: string) => Promise<void> } | null = null;
/** The dictation to start once the model is downloaded. */
let startAfterDownload: string | null = null;
let statusRequested = false;

/** Whether this webview can capture audio the way dictation does. */
function captureSupported(): boolean {
  return (
    typeof navigator.mediaDevices?.getUserMedia === "function" &&
    typeof AudioWorkletNode !== "undefined"
  );
}

function refreshStatus(): void {
  // Progress from before this asked, with the daemon now saying nothing downloads, is left over
  // from a download whose end never arrived.
  const before = snapshot.model;
  request({ method: "getDictation" })
    .then(({ dictation }) => {
      set({ status: dictation });
      if (!dictation.downloading && before && snapshot.model === before) set({ model: null });
    })
    .catch((error: unknown) => console.error("dictation status failed", error));
}

/**
 * The dictation state for the composer `owner`, and whether its Dictate button shows: only
 * where the webview can capture audio and the daemon has the speech engine.
 */
export function useDictation(owner: string): {
  available: boolean;
  phase: DictationPhase;
  levels: number[];
} {
  const state = useSyncExternalStore(subscribeStore, () => snapshot);
  useEffect(() => {
    if (statusRequested) return;
    statusRequested = true;
    refreshStatus();
  }, []);
  const mine = state.owner === owner;
  return {
    available: Boolean(state.status?.available) && captureSupported(),
    phase: mine ? state.phase : { type: "idle" },
    levels: mine ? state.levels : [],
  };
}

function fail(message: string): void {
  stopCapture();
  startAfterDownload = null;
  onText = null;
  set({ phase: { type: "failed", message }, levels: [] });
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Starts dictating into the composer `owner` (downloading the speech model first if needed). */
export async function startDictation(owner: string): Promise<void> {
  if (capture || (snapshot.owner !== owner && busy())) return;
  set({ owner, levels: [] });
  if (!snapshot.status?.installed) {
    startAfterDownload = owner;
    set({ phase: { type: "downloading", received: 0, total: snapshot.status?.modelBytes ?? 0 } });
    try {
      await request({ method: "downloadDictationModel" });
    } catch (error) {
      fail(`The speech model couldn't download: ${messageOf(error)}`);
    }
    return;
  }
  set({ phase: { type: "starting" } });
  try {
    await openCapture();
  } catch (error) {
    const denied = error instanceof DOMException && error.name === "NotAllowedError";
    fail(
      denied
        ? "Brigadier can't use the microphone. Allow it in System Settings → Privacy & Security → Microphone."
        : `Dictation couldn't start: ${messageOf(error)}`,
    );
    return;
  }
  try {
    const { dictationId } = await request({ method: "startDictation" });
    // Read again: it may have been cancelled while the daemon started.
    const opened = capture as Capture | null;
    if (!opened) {
      void request({ method: "cancelDictation", dictationId }).catch(() => {});
      return;
    }
    opened.id = dictationId;
    for (const chunk of opened.early.splice(0)) send(opened, chunk);
    set({ phase: { type: "recording" } });
  } catch (error) {
    fail(`Dictation couldn't start: ${messageOf(error)}`);
  }
}

function busy(): boolean {
  return snapshot.phase.type !== "idle" && snapshot.phase.type !== "failed";
}

const AUDIO: MediaTrackConstraints = {
  channelCount: 1,
  echoCancellation: true,
  noiseSuppression: true,
  autoGainControl: true,
};

/**
 * The microphone chosen in Settings › Voice (`deviceId`), or the system's default when none is
 * chosen or it isn't connected now.
 */
export async function openMicrophone(deviceId: string): Promise<MediaStream> {
  if (!deviceId) return navigator.mediaDevices.getUserMedia({ audio: AUDIO });
  try {
    return await navigator.mediaDevices.getUserMedia({ audio: { ...AUDIO, deviceId: { exact: deviceId } } });
  } catch (error) {
    const name = (error as { name?: unknown } | null)?.name;
    if (name !== "OverconstrainedError" && name !== "NotFoundError") throw error;
    return navigator.mediaDevices.getUserMedia({ audio: AUDIO });
  }
}

async function openCapture(): Promise<void> {
  const stream = await openMicrophone(useApp.getState().settings.dictationMicrophone);
  const context = new AudioContext();
  try {
    // A context made after an await may start suspended (no user gesture left to start it).
    await context.resume();
    await context.audioWorklet.addModule(workletUrl);
  } catch (error) {
    for (const track of stream.getTracks()) track.stop();
    void context.close();
    throw error;
  }
  const source = context.createMediaStreamSource(stream);
  const worklet = new AudioWorkletNode(context, "pcm16");
  const analyser = context.createAnalyser();
  analyser.fftSize = 1024;
  source.connect(worklet);
  source.connect(analyser);
  // A node reached from the destination keeps being processed; the worklet outputs silence.
  worklet.connect(context.destination);
  const samples = new Float32Array(analyser.fftSize);
  const meter = setInterval(() => {
    analyser.getFloatTimeDomainData(samples);
    let sum = 0;
    for (const sample of samples) sum += sample * sample;
    const level = Math.min(1, Math.sqrt(Math.sqrt(sum / samples.length)) * 1.6);
    set({ levels: [...snapshot.levels, level].slice(-LEVELS) });
  }, LEVEL_EVERY_MS);
  const opened: Capture = {
    stream,
    context,
    worklet,
    analyser,
    meter,
    id: null,
    early: [],
    sending: Promise.resolve(),
    dropped: false,
    flushed: null,
  };
  worklet.port.addEventListener("message", (event: MessageEvent<Int16Array | string>) => {
    if (event.data === "flushed") opened.flushed?.();
    else if (typeof event.data === "string" || opened.dropped) return;
    else if (opened.id) send(opened, event.data);
    else opened.early.push(event.data);
  });
  worklet.port.start();
  capture = opened;
}

function base64(chunk: Int16Array): string {
  const bytes = new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength);
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(binary);
}

/** Sends a piece of the capture's audio after the ones before it. */
function send(current: Capture, chunk: Int16Array): void {
  if (!current.id) return;
  const dictationId = current.id;
  const audio = base64(chunk);
  current.sending = current.sending.then(() =>
    request({ method: "appendDictation", dictationId, audio }).then(
      () => undefined,
      (error: unknown) => {
        if (!current.dropped) fail(`Dictation stopped: ${messageOf(error)}`);
      },
    ),
  );
}

function stopCapture(): void {
  const current = capture;
  if (!current) return;
  capture = null;
  current.dropped = true;
  clearInterval(current.meter);
  for (const track of current.stream.getTracks()) track.stop();
  void current.context.close();
}

/** The worklet's last samples, then the microphone off. */
async function finishCapture(current: Capture): Promise<void> {
  clearInterval(current.meter);
  for (const track of current.stream.getTracks()) track.stop();
  await new Promise<void>((resolve) => {
    const timer = setTimeout(resolve, 500);
    current.flushed = () => {
      clearTimeout(timer);
      resolve();
    };
    current.worklet.port.postMessage("flush", []);
  });
  void current.context.close();
}

/**
 * Stops recording and transcribes: the text goes to the composer's caret, and with `then`
 * (a "Transcribe and send") that runs once the composer has it.
 */
export async function stopDictation(then?: () => void): Promise<void> {
  const current = capture;
  const owner = snapshot.owner;
  if (!current?.id || !owner) return;
  capture = null;
  set({ phase: { type: "transcribing" }, levels: [] });
  const dictationId = current.id;
  onText = {
    id: dictationId,
    deliver: async (text) => {
      const insert = inserters.get(owner);
      if (text && insert) await insert(text);
      then?.();
    },
  };
  try {
    await finishCapture(current);
    await current.sending;
    await request({ method: "finishDictation", dictationId });
  } catch (error) {
    fail(`Transcribing failed: ${messageOf(error)}`);
  }
}

/** Drops the dictation (or the download it waits for) and its audio. */
export function cancelDictation(): void {
  const current = capture;
  stopCapture();
  onText = null;
  if (current?.id) {
    void request({ method: "cancelDictation", dictationId: current.id }).catch(() => {});
  }
  if (snapshot.phase.type === "downloading") {
    startAfterDownload = null;
    void request({ method: "cancelDictationDownload" }).catch(() => {});
  }
  set({ phase: { type: "idle" }, owner: null, levels: [] });
}

/** From the bridge: how a dictation or the model's download goes. */
export function onDictationUpdate(update: DictationUpdate): void {
  switch (update.type) {
    case "download":
      set({ model: { received: update.received, total: update.total } });
      if (snapshot.phase.type === "downloading") {
        set({ phase: { type: "downloading", received: update.received, total: update.total } });
      }
      break;
    case "downloaded": {
      set({ model: null, modelError: null });
      if (snapshot.status) set({ status: { ...snapshot.status, installed: true, downloading: false } });
      const owner = startAfterDownload;
      startAfterDownload = null;
      if (owner && snapshot.phase.type === "downloading") void startDictation(owner);
      break;
    }
    case "downloadStopped":
      set({ model: null, modelError: update.message ?? null });
      refreshStatus();
      if (snapshot.phase.type !== "downloading") break;
      if (update.message) fail(`The speech model couldn't download: ${update.message}`);
      else set({ phase: { type: "idle" }, owner: null });
      startAfterDownload = null;
      break;
    case "transcribed": {
      if (onText?.id !== update.dictationId) break;
      const { deliver } = onText;
      onText = null;
      set({ phase: { type: "idle" }, owner: null });
      void deliver(update.text.trim());
      break;
    }
    case "failed":
      if (onText?.id === update.dictationId) fail(update.message);
      break;
  }
}

/** The daemon went away: what it was doing for dictation is gone with it. */
export function onDictationDisconnected(): void {
  if (busy()) fail("The connection to Brigadier's core dropped.");
  // A download's progress and end no longer reach this connection; the next status says.
  set({ model: null });
  statusRequested = false;
}

/** The speech model as Settings › Voice shows it: its status, its download, why one failed. */
export function useSpeechModel(): Pick<Snapshot, "status" | "model" | "modelError"> & { dictating: boolean } {
  const state = useSyncExternalStore(subscribeStore, () => snapshot);
  useEffect(() => {
    if (statusRequested) return;
    statusRequested = true;
    refreshStatus();
  }, []);
  return {
    status: state.status,
    model: state.model ?? (state.status?.downloading ? { received: 0, total: state.status.modelBytes } : null),
    modelError: state.modelError,
    dictating: busy(),
  };
}

/** Downloads the speech model without starting a dictation (Settings › Voice). */
export async function downloadSpeechModel(): Promise<void> {
  set({ model: { received: 0, total: snapshot.status?.modelBytes ?? 0 }, modelError: null });
  try {
    await request({ method: "downloadDictationModel" });
  } catch (error) {
    set({ model: null, modelError: messageOf(error) });
  }
}

export function cancelSpeechModelDownload(): void {
  void request({ method: "cancelDictationDownload" }).catch(() => {});
}

/** Reads the speech model's status again (after it was removed). */
export function refreshSpeechModel(): void {
  refreshStatus();
}
