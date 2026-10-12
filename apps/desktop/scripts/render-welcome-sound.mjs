// Renders the first launch's welcome sound, src/app/onboarding/welcome-sound.m4a.
//
//   node scripts/render-welcome-sound.mjs [out.m4a]
//
// A build-up and a signature, in D, played on sampled instruments: the FluidR3 Mono GM SoundFont
// (MIT licence; see NOTICE.md next to the sound), rendered offline by spessasynth_core, with a
// faint synthesized shimmer high above. The SoundFont isn't kept in the repo: it's downloaded
// once into a cache and checked against its SHA-256 (or set BRIGADIER_SOUNDFONT to a local copy).
//   - Before the drop there is no beat. Low strings and a low brass pedal on D rise from
//     silence before anything is seen; trembling strings and violins join as the orb comes up
//     and idles, open fifths with no third (so it never settles), leaning on a suspended G.
//     A timpani roll and a cymbal swell grow towards the moment the orb opens into the window
//     (OPEN) and crest with it; then the music breathes in.
//   - As Brigadier's mark comes up in the window (DROP), Brigadier's signature sounds: a struck,
//     resonant bloom on D that rings long (SIGNATURE picks which of the three below).
//   - It doesn't go on as a score. What it leaves rings on as one slow, breathing resonance,
//     with no beat and no tune, under the title (HEADING); a single soft bell touches its button
//     (CONTINUE); then it settles and fades to silence a few seconds later.
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
 * Brigadier's signature, after the burst:
 *   "A", the bloom: one chord, struck low and bright, that the brass and strings swell out of;
 *   "B", the rise: three notes climbing in fifths (D, A, E), stacking into the same bloom;
 *   "C", the bells: bells and voices on one chord, blooming into a choir.
 * The intro ships A. To hear another, set BRIGADIER_WELCOME_SIGNATURE or change this, and render.
 */
const SIGNATURE = process.env.BRIGADIER_WELCOME_SIGNATURE ?? "A";

/** The SoundFont: FluidR3 Mono GM 2.312, as MuseScore 2.1 ships it, and its SHA-256. */
const SOUNDFONT_URL = "https://github.com/musescore/MuseScore/raw/2.1/share/sound/FluidR3Mono_GM.sf3";
const SOUNDFONT_SHA256 = "cfcd66d89e8386823400eca64934b14fbea7bf48ba1f00d21189af1262794ec2";

const SR = 44_100;
/** When the orb comes up, when it opens into the window, and the drop, in seconds. */
const ORB = 0.58;
const OPEN = 4.07;
const DROP = 4.8;
/** The title's first letter, and its button. */
const HEADING = 7.2;
const CONTINUE = 9;
/** The resonance settles from here and has faded to silence by the end. */
const SETTLE = CONTINUE + 0.8;
const SILENT = CONTINUE + 4.2;
const DURATION = SILENT + 0.4;
/**
 * The loudest sample before the drop, as a fraction of full scale (about -13 dBFS): the
 * build-up stays exactly as loud as it was first made, whichever signature follows it.
 */
const BUILD_PEAK = 0.22309069335;

// ── The score, as timed MIDI messages ───────────────────────────────────────────────────────
const events = [];
const NOTE_ON = 0x90;
const NOTE_OFF = 0x80;
const CONTROL = 0xb0;
const PROGRAM = 0xc0;
const EXPRESSION = 11;
const REVERB = 91;

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
/** A smooth change of a controller (the expression, by default) from `from` to `to`. */
function ramp(channel, start, end, from, to, number = EXPRESSION) {
  for (let at = start; at <= end + 1e-9; at += 0.02) {
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
const VIBES = 7; // Vibraphone
const VOICES = 8; // Voice Oohs
const DRUMS = 9; // the percussion kit (cymbals)
const PIANO = 10; // Grand Piano, its lowest notes
const HARP = 11; // Harp
const BELLS = 12; // Tubular Bells
const CELESTA = 13; // Celesta
const CHOIR = 14; // Choir Aahs
const PAD = 15; // Halo Pad
const PROGRAMS = {
  [VIOLINS]: 48,
  [LOW_STRINGS]: 48,
  [TREMOLO]: 44,
  [HORNS]: 60,
  [TROMBONES]: 57,
  [TUBA]: 58,
  [TIMPANI]: 47,
  [VIBES]: 11,
  [VOICES]: 53,
  [PIANO]: 0,
  [HARP]: 46,
  [BELLS]: 14,
  [CELESTA]: 8,
  [CHOIR]: 52,
  [PAD]: 94,
};
const PANS = {
  [VIOLINS]: 44,
  [LOW_STRINGS]: 80,
  [TREMOLO]: 52,
  [HORNS]: 50,
  [TROMBONES]: 76,
  [TUBA]: 70,
  [TIMPANI]: 64,
  [VIBES]: 54,
  [VOICES]: 72,
  [DRUMS]: 66,
  [PIANO]: 62,
  [HARP]: 40,
  [BELLS]: 70,
  [CELESTA]: 58,
  [CHOIR]: 56,
  [PAD]: 64,
};
for (let channel = 0; channel < 16; channel++) {
  if (channel in PROGRAMS) events.push({ at: 0, order: -1, message: [PROGRAM | channel, PROGRAMS[channel]] });
  control(channel, 0, 7, 100); // volume
  control(channel, 0, 10, PANS[channel]);
  control(channel, 0, REVERB, 72);
  control(channel, 0, 93, 0); // chorus
  control(channel, 0, EXPRESSION, 100);
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
// The build-up's strings let go as the drop comes (their last moment, as first made).
control(TREMOLO, DROP - 0.01, EXPRESSION, 92);

// ── The signature, as the mark comes up ─────────────────────────────────────────────────────
// Each is struck on the drop: something low and heavy (timpani, the piano's lowest D) under
// something bright (harp, bells, celesta), so it's heard at once, then the sustained voices
// swell out of the strike and ring on. Whatever is still held when the strike has faded
// becomes the resonance below, so the two are one sound.

/** Brass and strings held as one chord on `pitches`, swelling from `from` to a peak at `peakAt`, then easing back to `rest`. */
function bloom(channel, pitches, velocity, { from, peak, peakAt, rest, restAt, until = SILENT }) {
  control(channel, DROP - 0.01, EXPRESSION, from);
  ramp(channel, DROP, peakAt, from, peak);
  ramp(channel, peakAt + 0.05, restAt, peak, rest);
  pitches.forEach((pitch) => note(channel, DROP, until - DROP, pitch, velocity));
}

/** What each signature leaves sounding: the channels that breathe on, and the shimmer's notes above. */
const SIGNATURES = {
  // One bloom: D with its ninth, struck low (timpani, piano) and bright (a rolled harp chord,
  // a bell on the octave), then brass and strings swelling out of it.
  A() {
    note(TIMPANI, DROP, 3.5, 38, 96); // D2
    [26, 38].forEach((pitch) => note(PIANO, DROP, 5, pitch, 92)); // D1 D2
    [50, 57, 64, 66, 69, 74].forEach((pitch, k) => note(HARP, DROP + k * 0.014, 5, pitch, 96 - k * 4)); // D3 A3 E4 F#4 A4 D5
    note(BELLS, DROP, 6, 74, 72); // D5
    note(DRUMS, DROP, 3, CRASH, 30);
    for (const channel of [HORNS, TROMBONES, TUBA, HARP, BELLS, PIANO]) control(channel, DROP - 0.01, REVERB, 104);
    bloom(TUBA, [38], 78, { from: 46, peak: 110, peakAt: DROP + 0.7, rest: 0, restAt: DROP + 5, until: DROP + 5.2 });
    bloom(TROMBONES, [50, 57], 74, { from: 40, peak: 112, peakAt: DROP + 0.75, rest: 0, restAt: DROP + 5, until: DROP + 5.2 });
    bloom(HORNS, [64, 66, 69], 84, { from: 44, peak: 116, peakAt: DROP + 0.8, rest: 0, restAt: DROP + 5.4, until: DROP + 5.6 });
    bloom(LOW_STRINGS, [38, 45], 84, { from: 60, peak: 104, peakAt: DROP + 1, rest: 72, restAt: DROP + 3.5 });
    bloom(VIOLINS, [76, 78, 81, 86], 72, { from: 50, peak: 100, peakAt: DROP + 1.1, rest: 64, restAt: DROP + 3.6 });
    return { breathing: [VIOLINS, LOW_STRINGS], shimmer: [86, 90, 93, 100], accent: 93 };
  },
  // A rise in fifths: D, A, then E, each held, stacking into the same open chord; the third
  // lands later than the second and the bloom swells out of it.
  B() {
    const RISE = [
      [0, 62, 88], // D4, as the mark comes up
      [0.28, 69, 94], // A4
      [0.64, 76, 104], // E5, the landing
    ];
    note(TIMPANI, DROP, 3, 38, 90); // D2
    [26, 38].forEach((pitch) => note(PIANO, DROP, 4.5, pitch, 86)); // D1 D2
    for (const channel of [HORNS, TROMBONES, TUBA, HARP, BELLS, PIANO, CELESTA]) control(channel, DROP - 0.01, REVERB, 104);
    control(HORNS, DROP - 0.01, EXPRESSION, 100);
    ramp(HORNS, DROP + 1.4, DROP + 5.6, 100, 0);
    for (const [after, pitch, velocity] of RISE) {
      const at = DROP + after;
      note(HORNS, at, DROP + 5.8 - at, pitch, velocity);
      note(HARP, at, 4, pitch - 12, velocity - 6);
      note(HARP, at, 4, pitch, velocity - 12);
      note(CELESTA, at, 3, pitch + 12, Math.round(velocity * 0.5));
    }
    const LAND = DROP + 0.64;
    note(BELLS, LAND, 6, 76, 70); // E5
    note(TIMPANI, LAND, 3, 45, 64); // A2
    note(DRUMS, LAND, 3, CRASH, 28);
    // The bloom out of the landing: D with its ninth, brass and strings.
    const out = (peakAt) => ({ peakAt: LAND + peakAt });
    bloom(TUBA, [38], 72, { from: 0, peak: 100, ...out(0.7), rest: 0, restAt: DROP + 5, until: DROP + 5.2 });
    bloom(TROMBONES, [50, 57], 68, { from: 0, peak: 102, ...out(0.75), rest: 0, restAt: DROP + 5, until: DROP + 5.2 });
    bloom(LOW_STRINGS, [38, 45], 84, { from: 50, peak: 104, ...out(0.9), rest: 72, restAt: DROP + 3.8 });
    bloom(VIOLINS, [66, 69, 76, 81], 68, { from: 0, peak: 96, ...out(1), rest: 64, restAt: DROP + 3.9 });
    return { breathing: [VIOLINS, LOW_STRINGS], shimmer: [88, 93, 98, 100], accent: 88 };
  },
  // Bells and voices: a tubular bell's D over its fifth, vibraphone and celesta ringing the
  // chord above, a choir swelling out of the strike and staying, voices above it.
  C() {
    note(TIMPANI, DROP, 3, 38, 74); // D2
    note(PIANO, DROP, 4.5, 26, 80); // D1
    note(BELLS, DROP, 7, 62, 96); // D4
    note(BELLS, DROP, 7, 69, 74); // A4
    [74, 78, 81, 88].forEach((pitch) => note(VIBES, DROP, 6, pitch, 80)); // D5 F#5 A5 E6
    [86, 93].forEach((pitch) => note(CELESTA, DROP, 3, pitch, 54)); // D6 A6
    for (const channel of [BELLS, VIBES, CELESTA, CHOIR, VOICES, PIANO, PAD]) control(channel, DROP - 0.01, REVERB, 110);
    bloom(CHOIR, [50, 57, 66, 76], 96, { from: 24, peak: 118, peakAt: DROP + 1.1, rest: 84, restAt: DROP + 3.8 });
    bloom(VOICES, [69, 74], 80, { from: 0, peak: 104, peakAt: DROP + 1.6, rest: 70, restAt: DROP + 4 });
    bloom(PAD, [38, 45], 76, { from: 30, peak: 96, peakAt: DROP + 1.2, rest: 78, restAt: DROP + 3.5 });
    bloom(LOW_STRINGS, [38], 72, { from: 50, peak: 92, peakAt: DROP + 1, rest: 66, restAt: DROP + 3.5 });
    return { breathing: [CHOIR, VOICES, PAD, LOW_STRINGS], shimmer: [86, 90, 93, 98], accent: 90 };
  },
};
if (!(SIGNATURE in SIGNATURES)) throw new Error(`no signature ${SIGNATURE}: A, B or C`);
const after = SIGNATURES[SIGNATURE]();

// ── The resonance ───────────────────────────────────────────────────────────────────────────
// No beat and no tune: what the signature left holds on, its loudness breathing slowly in and
// out, each part in its own time, while a soft pad joins underneath; from SETTLE it all eases
// down to silence.
const settle = (at) => 1 - smooth((at - SETTLE) / (SILENT - 0.2 - SETTLE));
after.breathing.forEach((channel, k) => {
  const from = DROP + 3.8 + k * 0.1;
  const period = 3.4 + k * 0.9;
  for (let at = from; at <= SILENT; at += 0.04) {
    const breath = 0.5 - 0.5 * Math.cos((2 * Math.PI * (at - from)) / period);
    control(channel, at, EXPRESSION, (66 + 18 * breath) * settle(at));
  }
});
if (SIGNATURE !== "C") {
  control(PAD, 0, REVERB, 110);
  control(PAD, DROP + 1.4, EXPRESSION, 0);
  [50, 57, 64].forEach((pitch) => note(PAD, DROP + 1.5, SILENT - DROP - 1.5, pitch, 70)); // D3 A3 E4
  for (let at = DROP + 1.5; at <= SILENT; at += 0.04) {
    const breath = 0.5 - 0.5 * Math.cos((2 * Math.PI * (at - DROP - 1.5)) / 4.6);
    control(PAD, at, EXPRESSION, smooth((at - DROP - 1.5) / 2.5) * (70 + 22 * breath) * settle(at));
  }
}
// The button: one soft celesta note high in the chord.
control(CELESTA, CONTINUE - 0.1, REVERB, 120);
note(CELESTA, CONTINUE, 2.5, after.accent, 40);

/**
 * The shimmer, synthesized: soft pairs of sine tones high above the chord, each pair a little
 * out of tune with itself so it slowly beats, each swelling and fading in its own time.
 */
function shimmer(left, right, pitches) {
  const from = DROP + 1.2;
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
shimmer(left, right, after.shimmer);

// ── The whole ───────────────────────────────────────────────────────────────────────────────
// A quiet fade to silence at the end, no clicks, at the build-up's level.
const BUILT = Math.round(SR * (DROP - 0.012));
let build = 0;
for (const out of [left, right]) {
  for (let i = 0; i < N; i++) {
    const time = i / SR;
    const end = Math.min(1, Math.max(0, (DURATION - time) / 1.2));
    out[i] *= end * end * (3 - 2 * end) * Math.min(1, time / 0.01);
    if (i < BUILT) build = Math.max(build, Math.abs(out[i]));
  }
}
let peak = 0;
let squares = 0;
for (const out of [left, right]) {
  for (let i = 0; i < N; i++) {
    out[i] = (out[i] / build) * BUILD_PEAK;
    peak = Math.max(peak, Math.abs(out[i]));
    squares += out[i] * out[i];
  }
}
if (peak >= 0.5) throw new Error(`the signature peaks at ${(20 * Math.log10(peak)).toFixed(1)} dBFS, above -6`);
const rms = Math.sqrt(squares / (N * 2));
const wav = Buffer.from(audioToWav([left, right], SR));
console.log(`signature ${SIGNATURE}: peak ${(20 * Math.log10(peak)).toFixed(1)} dBFS, rms ${(20 * Math.log10(rms)).toFixed(1)} dBFS, ${DURATION.toFixed(2)} s`);
console.log(`orb ${ORB}, open ${OPEN}, drop ${DROP}, heading ${HEADING}, continue ${CONTINUE}, settle ${SETTLE}, silent ${SILENT}`);

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
