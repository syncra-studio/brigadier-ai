// Renders the first launch's welcome sound, src/app/onboarding/welcome-sound.m4a.
//
//   node scripts/render-welcome-sound.mjs [out.m4a]
//
// A short orchestral piece in D, played on sampled instruments: the FluidR3 Mono GM SoundFont
// (MIT licence; see NOTICE.md next to the sound), rendered offline by spessasynth_core, with a
// faint synthesized shimmer at its end. The SoundFont isn't kept in the repo: it's downloaded
// once into a cache and checked against its SHA-256 (or set BRIGADIER_SOUNDFONT to a local copy).
// It builds, drops, then rings out:
//   - Before the drop there is no beat. Low strings and a low brass pedal on D rise from
//     silence before anything is seen; trembling strings and violins join as the orb comes up
//     and idles, open fifths with no third (so it never settles), leaning on a suspended G.
//     A timpani roll and a cymbal swell grow towards the moment the orb opens into the window
//     (OPEN) and crest with it; then the music breathes in.
//   - The drop comes as Brigadier's mark comes up in the window (DROP): the whole orchestra on
//     a full D major chord, timpani and a soft cymbal under it. While the mark is held, the
//     cellos drive in eighths under the chord and the timpani marks the half bar.
//   - Once the mark has gone (HIDDEN) the motion stops. The strings' chord, held from the
//     drop, rings on as one slow resonance, with no beat and no tune: it breathes in and out
//     under the title (HEADING), joined by RESONANCE's voices and a faint shimmer high above;
//     a single soft note touches the button (CONTINUE), then it settles and fades to silence.
// Those times match the intro's (IntroCanvas.tsx, and the --intro-* tokens in tokens.css).
//
// Writes a WAV, then encodes it to AAC with macOS's afconvert (on other systems the WAV is
// left next to the output instead).

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { audioToWav, SoundBankLoader, SpessaSynthProcessor } from "spessasynth_core";

const OUT = resolve(process.argv[2] ?? new URL("../src/app/onboarding/welcome-sound.m4a", import.meta.url).pathname);

/**
 * What joins the strings' chord once the mark has gone:
 *   "A", light: a soft halo pad under the strings, and the shimmer above;
 *   "B", air: the chord opens to bare fifths with high strings floating over them;
 *   "C", voices: a choir takes the chord from the strings, with soft voices above.
 * The intro ships B. To hear another, set BRIGADIER_WELCOME_RESONANCE or change this, and render.
 */
const RESONANCE = process.env.BRIGADIER_WELCOME_RESONANCE ?? "B";

/** The SoundFont: FluidR3 Mono GM 2.312, as MuseScore 2.1 ships it, and its SHA-256. */
const SOUNDFONT_URL = "https://github.com/musescore/MuseScore/raw/2.1/share/sound/FluidR3Mono_GM.sf3";
const SOUNDFONT_SHA256 = "cfcd66d89e8386823400eca64934b14fbea7bf48ba1f00d21189af1262794ec2";

const SR = 44_100;
/** When the orb comes up, when it opens into the window, and the drop, in seconds. */
const ORB = 0.58;
const OPEN = 4.07;
const DROP = 4.8;
/** The beat and the bar after the drop (100 beats a minute, four to a bar). */
const BEAT = 60 / 100;
const BAR = 4 * BEAT;
/** The mark has faded out: the motion stops and the resonance takes over. */
const HIDDEN = 6.75;
/** The title comes in on the bar after the drop; its button on that bar's last beat. */
const HEADING = DROP + BAR;
const CONTINUE = HEADING + 3 * BEAT;
/** The resonance settles from here and has faded to silence by SILENT. */
const SETTLE = CONTINUE + 0.8;
const SILENT = CONTINUE + 4.2;
const DURATION = SILENT + 0.4;
/** The loudest sample, as a fraction of full scale (about -7.5 dBFS). */
const PEAK = 0.42;

// ── The score, as timed MIDI messages ───────────────────────────────────────────────────────
const events = [];
const NOTE_ON = 0x90;
const NOTE_OFF = 0x80;
const CONTROL = 0xb0;
const PROGRAM = 0xc0;
const EXPRESSION = 11;

/** The sampled instruments' attacks take about this long to speak: notes start this early. */
const SPEAK = 0.008;

/** A note on `channel` from `at` for `length` seconds. Offs sort before ons at one time. */
function note(channel, at, length, pitch, velocity) {
  events.push({ at: Math.max(0, at - SPEAK), order: 1, message: [NOTE_ON | channel, pitch, velocity] });
  events.push({ at: at + length, order: 0, message: [NOTE_OFF | channel, pitch, 0] });
}
function control(channel, at, number, value) {
  events.push({ at, order: 0, message: [CONTROL | channel, number, Math.round(Math.min(127, Math.max(0, value)))] });
}
/** A smooth change of a controller (the expression, by default) from `from` to `to`, cut short at `until`. */
function ramp(channel, start, end, from, to, number = EXPRESSION, until = Infinity) {
  for (let at = start; at <= end + 1e-9 && at < until; at += 0.02) {
    const x = Math.min(1, (at - start) / (end - start));
    control(channel, at, number, from + (to - from) * x * x * (3 - 2 * x));
  }
}
const smooth = (x) => {
  const t = Math.min(1, Math.max(0, x));
  return t * t * (3 - 2 * t);
};

// The orchestra: a channel each, with its General MIDI program.
const VIOLINS = 0; // String Ensemble 1
const LOW_STRINGS = 1; // String Ensemble 1, low
const TREMOLO = 2; // Tremolo Strings
const HORNS = 3; // French Horn
const TROMBONES = 4; // Trombone
const TUBA = 5; // Tuba
const TIMPANI = 6; // Timpani
const BRASS = 7; // Brass Section
const CELLOS = 8; // Cello
const DRUMS = 9; // the percussion kit (cymbals)
const PAD = 10; // Halo Pad
const HIGH_STRINGS = 11; // Slow Strings
const CHOIR = 12; // Choir Aahs
const VOICES = 13; // Voice Oohs
const CELESTA = 14; // Celesta
const HARP = 15; // Harp
const PROGRAMS = {
  [VIOLINS]: 48,
  [LOW_STRINGS]: 48,
  [TREMOLO]: 44,
  [HORNS]: 60,
  [TROMBONES]: 57,
  [TUBA]: 58,
  [TIMPANI]: 47,
  [BRASS]: 61,
  [CELLOS]: 42,
  [PAD]: 94,
  [HIGH_STRINGS]: 49,
  [CHOIR]: 52,
  [VOICES]: 53,
  [CELESTA]: 8,
  [HARP]: 46,
};
const PANS = {
  [VIOLINS]: 44,
  [LOW_STRINGS]: 80,
  [TREMOLO]: 52,
  [HORNS]: 50,
  [TROMBONES]: 76,
  [TUBA]: 70,
  [TIMPANI]: 64,
  [BRASS]: 60,
  [CELLOS]: 84,
  [DRUMS]: 66,
  [PAD]: 64,
  [HIGH_STRINGS]: 36,
  [CHOIR]: 56,
  [VOICES]: 72,
  [CELESTA]: 58,
  [HARP]: 40,
};
for (let channel = 0; channel < 16; channel++) {
  if (channel in PROGRAMS) events.push({ at: 0, order: -1, message: [PROGRAM | channel, PROGRAMS[channel]] });
  control(channel, 0, 7, 100); // volume
  control(channel, 0, 10, PANS[channel]);
  control(channel, 0, 91, channel < PAD ? 72 : 110); // reverb: the resonance's own voices sit further back
  control(channel, 0, 93, 0); // chorus
  control(channel, 0, EXPRESSION, channel < PAD ? 100 : 0);
}
const CRASH = 49;
const RIDE = 51;

// ── Before the drop ─────────────────────────────────────────────────────────────────────────
// Low strings and a low brass pedal on D, rising from silence.
ramp(LOW_STRINGS, 0, OPEN, 18, 92);
ramp(LOW_STRINGS, OPEN + 0.1, DROP - 0.1, 92, 70);
note(LOW_STRINGS, 0, DROP - 0.02, 38, 70); // D2
note(LOW_STRINGS, 0.3, DROP - 0.32, 45, 64); // A2
ramp(TUBA, 0, OPEN, 25, 85);
ramp(TUBA, OPEN + 0.1, DROP - 0.1, 85, 60);
note(TUBA, 0.15, DROP - 0.2, 38, 52); // D2
ramp(TROMBONES, 0.4, OPEN, 20, 80);
ramp(TROMBONES, OPEN + 0.1, DROP - 0.1, 80, 55);
note(TROMBONES, 0.4, DROP - 0.45, 50, 46); // D3
// Trembling strings as the orb comes up, an open fifth, then its second and a suspended G.
ramp(TREMOLO, ORB, OPEN, 12, 100);
ramp(TREMOLO, OPEN + 0.05, DROP - 0.05, 100, 62);
note(TREMOLO, ORB, DROP - ORB, 62, 64); // D4
note(TREMOLO, ORB + 0.2, DROP - ORB - 0.2, 69, 60); // A4
note(TREMOLO, 1.7, DROP - 1.7, 64, 52); // E4
note(TREMOLO, 2.7, DROP - 2.7, 67, 56); // G4
// Violins high above, rising.
ramp(VIOLINS, 1.1, OPEN, 15, 95);
ramp(VIOLINS, OPEN + 0.05, DROP - 0.05, 95, 64);
note(VIOLINS, 1.1, DROP - 1.1, 74, 58); // D5
note(VIOLINS, 2.3, DROP - 2.3, 81, 50); // A5

// A timpani roll on A and a cymbal swell, growing into the opening and cresting with it.
for (let at = OPEN - 1.3; at < OPEN; at += 1 / 16) {
  const x = (at - (OPEN - 1.3)) / 1.3;
  note(TIMPANI, at, 0.2, 45, Math.round(14 + 56 * x * x));
}
note(TIMPANI, OPEN, 1.2, 45, 76);
for (let at = OPEN - 1.6; at < OPEN; at += 1 / 22) {
  const x = (at - (OPEN - 1.6)) / 1.6;
  note(DRUMS, at, 0.3, RIDE, Math.round(5 + 50 * x ** 2.2));
}
note(DRUMS, OPEN, 2, CRASH, 48);

// ── The drop: the whole orchestra on D major ────────────────────────────────────────────────
for (const channel of [VIOLINS, LOW_STRINGS, TREMOLO, HORNS, TROMBONES, TUBA, BRASS]) control(channel, DROP - 0.01, EXPRESSION, 92);
note(TIMPANI, DROP, 1.5, 38, 96);
note(DRUMS, DROP, 2.5, CRASH, 48);
note(TUBA, DROP, 2 * BEAT, 38, 84); // D2
[50, 54, 57].forEach((pitch) => note(TROMBONES, DROP, 2 * BEAT, pitch, 76)); // D3 F#3 A3
[57, 62, 66].forEach((pitch) => note(HORNS, DROP, 2 * BEAT, pitch, 86)); // A3 D4 F#4
[62, 66, 69].forEach((pitch) => note(BRASS, DROP, 1.5 * BEAT, pitch, 54)); // D4 F#4 A4
// The strings' chord is held on, through the resonance after the mark (the violins' below).
[38, 50].forEach((pitch) => note(LOW_STRINGS, DROP, SILENT - DROP, pitch, 86)); // D2 D3
ramp(HORNS, DROP + 0.3, DROP + 2 * BEAT, 92, 80);
ramp(TROMBONES, DROP + 0.3, DROP + 2 * BEAT, 92, 78);
ramp(VIOLINS, DROP + 0.4, HEADING - 0.1, 92, 78, EXPRESSION, HIDDEN);

// ── The motion, while the mark is held ──────────────────────────────────────────────────────
// The cellos' eighths on D, driving, leaning on the first and the fourth beats; the timpani on
// the half bar, under the horns' D major. They stop as the mark goes.
const EIGHTHS = [96, 52, 66, 50, 82, 54, 64, 58];
EIGHTHS.forEach((weight, eighth) => {
  const at = DROP + (eighth * BEAT) / 2;
  if (at < HIDDEN) note(CELLOS, at, BEAT / 2 - 0.06, 50, Math.round(weight * (eighth < 4 ? 0.75 : 0.88)));
});
note(TIMPANI, DROP + 2 * BEAT, 1, 45, 70);
[57, 62, 66].forEach((pitch) => note(HORNS, DROP + 2 * BEAT, 2 * BEAT - 0.05, pitch, 58)); // A3 D4 F#4

// ── The resonance, once the mark has gone ───────────────────────────────────────────────────
// No beat and no tune. Each part's loudness breathes slowly in and out in its own time, from
// where it was to its own level, and from SETTLE everything eases down to silence.
const settle = (at) => 1 - smooth((at - SETTLE) / (SILENT - 0.2 - SETTLE));
/** `channel`'s expression from `from`: easing from `start` to `level` over `arrive` seconds, breathing up to `depth` above it every `period`. */
function breathe(channel, from, start, level, depth, period, arrive = 1.6) {
  for (let at = from; at <= SILENT; at += 0.04) {
    const breath = 0.5 - 0.5 * Math.cos((2 * Math.PI * (at - from)) / period);
    const arrived = smooth((at - from) / arrive);
    control(channel, at, EXPRESSION, (start + (level - start) * arrived + depth * arrived * breath) * settle(at));
  }
}
/** The violins' expression where their ramp was cut, as the resonance takes them over. */
const VIOLINS_AT_HIDDEN = 92 + (78 - 92) * smooth((HIDDEN - DROP - 0.4) / (HEADING - 0.1 - DROP - 0.4));

const RESONANCES = {
  // Light: the strings breathe on the chord, a halo pad swells in under them, and the shimmer
  // (D, F#, A and E, high) drifts over the top.
  A: {
    shimmer: [86, 90, 93, 100],
    accent: [CELESTA, 93], // A6
    play() {
      breathe(VIOLINS, HIDDEN, VIOLINS_AT_HIDDEN, 72, 14, 3.8);
      breathe(LOW_STRINGS, HIDDEN, 92, 70, 10, 5.1, 2.2);
      [50, 57, 66].forEach((pitch) => note(PAD, HIDDEN - 0.3, SILENT - HIDDEN + 0.3, pitch, 72)); // D3 A3 F#4
      breathe(PAD, HIDDEN - 0.3, 0, 78, 16, 4.6, 2.5);
    },
  },
  // Air: the violins' third falls away, leaving bare fifths; slow strings float a fifth and a
  // ninth above them, and the shimmer sits higher still.
  B: {
    shimmer: [93, 98, 100, 105],
    accent: [HARP, 86], // D6
    violinsLetGo: { 78: HIDDEN + 1.8 },
    play() {
      breathe(VIOLINS, HIDDEN, VIOLINS_AT_HIDDEN, 70, 18, 3.4);
      breathe(LOW_STRINGS, HIDDEN, 92, 66, 12, 5.6, 2.2);
      [81, 88].forEach((pitch) => note(HIGH_STRINGS, HIDDEN + 0.4, SILENT - HIDDEN - 0.4, pitch, 64)); // A5 E6
      breathe(HIGH_STRINGS, HIDDEN + 0.4, 0, 68, 18, 4.2, 2.8);
    },
  },
  // Voices: a choir swells into the chord as the strings thin under it, soft voices on the
  // octave and fifth above, the shimmer quieter.
  C: {
    shimmer: [86, 90, 93, 98],
    accent: [CELESTA, 90], // F#6
    play() {
      breathe(VIOLINS, HIDDEN, VIOLINS_AT_HIDDEN, 46, 10, 4.1, 3);
      breathe(LOW_STRINGS, HIDDEN, 92, 60, 8, 5.3, 2.5);
      [50, 57, 62, 66].forEach((pitch) => note(CHOIR, HIDDEN - 0.4, SILENT - HIDDEN + 0.4, pitch, 84)); // D3 A3 D4 F#4
      breathe(CHOIR, HIDDEN - 0.4, 0, 86, 16, 4.4, 2.2);
      [74, 81].forEach((pitch) => note(VOICES, HIDDEN + 0.5, SILENT - HIDDEN - 0.5, pitch, 70)); // D5 A5
      breathe(VOICES, HIDDEN + 0.5, 0, 70, 18, 3.7, 2.6);
    },
  },
};
if (!(RESONANCE in RESONANCES)) throw new Error(`no resonance ${RESONANCE}: A, B or C`);
const resonance = RESONANCES[RESONANCE];
// The violins' D major from the drop, held to the end unless the resonance lets one go sooner.
[74, 78, 81].forEach((pitch) => note(VIOLINS, DROP, (resonance.violinsLetGo?.[pitch] ?? SILENT) - DROP, pitch, 80)); // D5 F#5 A5
resonance.play();
// The button: one soft, high note in the chord.
const [accentChannel, accentPitch] = resonance.accent;
control(accentChannel, CONTINUE - 0.1, EXPRESSION, 100);
note(accentChannel, CONTINUE, 2.5, accentPitch, 40);

/**
 * The shimmer, synthesized: soft pairs of sine tones high above the chord, each pair a little
 * out of tune with itself so it slowly beats, each swelling and fading in its own time.
 */
function shimmer(left, right, pitches) {
  const from = HIDDEN;
  pitches.forEach((pitch, k) => {
    const frequency = 440 * 2 ** ((pitch - 69) / 12);
    const pan = 0.5 + 0.32 * Math.sin(1 + k * 2.4);
    const period = 2.9 + 1.1 * k;
    const offset = 0.6 * k;
    const level = 0.0016 * (1 - 0.15 * k);
    for (let i = Math.floor(from * SR); i < Math.min(left.length, SILENT * SR); i++) {
      const at = i / SR;
      const breath = 0.5 - 0.5 * Math.cos((2 * Math.PI * (at - from - offset)) / period);
      const size = level * smooth((at - from) / 2.4) * settle(at) * (0.25 + 0.75 * breath);
      const tone = Math.sin(2 * Math.PI * frequency * at) + Math.sin(2 * Math.PI * frequency * 1.0011 * at + k);
      left[i] += size * tone * (1 - pan);
      right[i] += size * tone * pan;
    }
  });
}

// ── Rendering ───────────────────────────────────────────────────────────────────────────────
async function soundFont() {
  const local = process.env.BRIGADIER_SOUNDFONT;
  const path = local ?? join(tmpdir(), "brigadier-soundfonts", "FluidR3Mono_GM.sf3");
  if (!existsSync(path)) {
    if (local) throw new Error(`BRIGADIER_SOUNDFONT: no file at ${local}`);
    console.log(`downloading the SoundFont from ${SOUNDFONT_URL}`);
    const response = await fetch(SOUNDFONT_URL);
    if (!response.ok) throw new Error(`the SoundFont download failed: ${response.status}`);
    mkdirSync(join(tmpdir(), "brigadier-soundfonts"), { recursive: true });
    writeFileSync(path, Buffer.from(await response.arrayBuffer()));
  }
  const bytes = readFileSync(path);
  const digest = createHash("sha256").update(bytes).digest("hex");
  if (digest !== SOUNDFONT_SHA256) throw new Error(`the SoundFont at ${path} isn't the expected one (sha-256 ${digest})`);
  return bytes;
}

const bytes = await soundFont();
const synth = new SpessaSynthProcessor(SR, { eventsEnabled: false });
synth.soundBankManager.addSoundBank(SoundBankLoader.fromArrayBuffer(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.length)), "main");
await synth.processorInitialized;
synth.setSystemParameter("autoAllocateVoices", true);

events.sort((a, b) => a.at - b.at || a.order - b.order);
const N = Math.round(SR * DURATION);
const left = new Float32Array(N);
const right = new Float32Array(N);
const BLOCK = 128;
let next = 0;
for (let start = 0; start < N; start += BLOCK) {
  const until = (start + BLOCK / 2) / SR;
  while (next < events.length && events[next].at <= until) synth.processMessage(new Uint8Array(events[next++].message));
  synth.process(left, right, start, Math.min(BLOCK, N - start));
}
shimmer(left, right, resonance.shimmer);

// ── The whole ───────────────────────────────────────────────────────────────────────────────
// A quiet fade to silence at the end, no clicks, at the planned peak.
let peak = 0;
for (const out of [left, right]) {
  for (let i = 0; i < N; i++) {
    const time = i / SR;
    const end = Math.min(1, Math.max(0, (DURATION - time) / 1.2));
    out[i] *= end * end * (3 - 2 * end) * Math.min(1, time / 0.01);
    peak = Math.max(peak, Math.abs(out[i]));
  }
}
let squares = 0;
for (const out of [left, right]) {
  for (let i = 0; i < N; i++) {
    out[i] = (out[i] / peak) * PEAK;
    squares += out[i] * out[i];
  }
}
const rms = Math.sqrt(squares / (N * 2));
const wav = Buffer.from(audioToWav([left, right], SR));
console.log(`resonance ${RESONANCE}: peak ${(20 * Math.log10(PEAK)).toFixed(1)} dBFS, rms ${(20 * Math.log10(rms)).toFixed(1)} dBFS, ${DURATION.toFixed(2)} s`);
console.log(`orb ${ORB}, open ${OPEN}, drop ${DROP}, hidden ${HIDDEN}, heading ${HEADING.toFixed(3)}, continue ${CONTINUE.toFixed(3)}, silent ${SILENT.toFixed(3)}`);

if (process.platform !== "darwin") {
  const out = OUT.replace(/\.m4a$/, ".wav");
  writeFileSync(out, wav);
  console.log(`wrote ${out} (encode it to AAC to replace ${OUT})`);
} else {
  const temporary = join(tmpdir(), `welcome-sound-${process.pid}.wav`);
  writeFileSync(temporary, wav);
  try {
    execFileSync("afconvert", ["-f", "m4af", "-d", "aac", "-b", "128000", "-q", "127", temporary, OUT]);
  } finally {
    rmSync(temporary, { force: true });
  }
  console.log(`wrote ${OUT}`);
}
