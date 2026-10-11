/*
 * The first launch's welcome sound (scripts/render-welcome-sound.mjs). It plays through Web
 * Audio rather than an <audio> element, so it doesn't show in the system's Now Playing. The
 * window plays it without waiting for a click (the webview is made with autoplay allowed).
 * It comes inlined in a chunk of its own, loaded only for the first launch: the app's
 * content security policy doesn't let it fetch its own files.
 */

/** How loud it plays. */
const GAIN = 0.6;
/** How long it takes to fade out when the setup closes before it has finished. */
const FADE_S = 0.6;

let loading: Promise<{ context: AudioContext; buffer: AudioBuffer }> | null = null;

/** Starts loading and decoding the sound, so the intro can start with it. Safe to call again. */
export function preloadWelcomeSound(): Promise<{ context: AudioContext; buffer: AudioBuffer }> {
  loading ??= (async () => {
    const context = new AudioContext();
    try {
      const { default: dataUrl } = await import("@/app/onboarding/welcome-sound.m4a?inline");
      const bytes = Uint8Array.from(atob(dataUrl.slice(dataUrl.indexOf(",") + 1)), (c) => c.charCodeAt(0));
      const buffer = await context.decodeAudioData(bytes.buffer);
      return { context, buffer };
    } catch (error) {
      void context.close().catch(() => {});
      throw error;
    }
  })();
  loading.catch(() => {
    loading = null;
  });
  return loading;
}

/**
 * Plays the sound once from the start. Resolves, as it starts, to a function that fades it out
 * (for a setup closed before it ends); rejects if it can't play.
 */
export async function playWelcomeSound(): Promise<() => void> {
  const { context, buffer } = await preloadWelcomeSound();
  await context.resume();
  if (context.state !== "running") throw new Error(`the welcome sound can't play (${context.state})`);
  const gain = context.createGain();
  gain.gain.value = GAIN;
  gain.connect(context.destination);
  const source = context.createBufferSource();
  source.buffer = buffer;
  source.connect(gain);
  let stopped = false;
  source.addEventListener("ended", () => {
    stopped = true;
    source.disconnect();
    gain.disconnect();
    // Played once: the next launch that needs it (a setup run again never does) loads it anew.
    loading = null;
    void context.close().catch(() => {});
  });
  source.start();
  return () => {
    if (stopped) return;
    stopped = true;
    const now = context.currentTime;
    gain.gain.setValueAtTime(gain.gain.value, now);
    gain.gain.linearRampToValueAtTime(0, now + FADE_S);
    source.stop(now + FADE_S);
  };
}
