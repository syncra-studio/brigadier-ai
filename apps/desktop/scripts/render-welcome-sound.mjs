// Renders the first launch's welcome music, src/app/onboarding/welcome-sound.m4a.
//
//   node scripts/render-welcome-sound.mjs [out.m4a]
//
// A short orchestral piece in D, played on sampled instruments: the FluidR3 Mono GM SoundFont
// (MIT licence; see NOTICE.md next to the sound), rendered offline by spessasynth_core. The
// SoundFont isn't kept in the repo: it's downloaded once into a cache and checked against its
// SHA-256 (or set BRIGADIER_SOUNDFONT to a local copy). It builds, then moves, then resolves:
//   - Before the drop there is no beat. Low strings and a low brass pedal on D rise from
//     silence before anything is seen; trembling strings and violins join as the orb comes up
//     and idles, open fifths with no third (so it never settles), leaning on a suspended G.
//     A timpani roll and a cymbal swell grow towards the moment the orb opens into the window
//     (OPEN) and crest with it; then the music breathes in.
//   - The drop comes as Brigadier's mark comes up in the window (DROP): the whole orchestra on
//     a full D major chord, timpani and a soft cymbal under it. The motion starts there: cellos
//     driving in eighths, timpani marking the bars, horns and strings holding the chords.
//   - The title comes in on the next bar (HEADING), with the horns' call, and its button on
//     that bar's last beat (CONTINUE), as the call reaches its fourth note.
//   - Two more bars carry the settled welcome. Then the motion stops, and the strings and
//     horns come to rest on D major, from G, ringing out to silence.
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
const bar = (n) => DROP + n * BAR;
/** The title starts on the bar after the drop; its button comes on that bar's last beat. */
const HEADING = bar(1);
const CONTINUE = bar(1) + 3 * BEAT;
/** The motion stops here; the orchestra comes to rest a bar later and rings out. */
const THIN = bar(4);
const REST = bar(5);
const DURATION = REST + 5;
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
/** A smooth change of a controller (the expression, by default) from `from` to `to`. */
function ramp(channel, start, end, from, to, number = EXPRESSION) {
  for (let at = start; at <= end + 1e-9; at += 0.02) {
    const x = Math.min(1, (at - start) / (end - start));
    control(channel, at, number, from + (to - from) * x * x * (3 - 2 * x));
  }
}

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
const PROGRAMS = { [VIOLINS]: 48, [LOW_STRINGS]: 48, [TREMOLO]: 44, [HORNS]: 60, [TROMBONES]: 57, [TUBA]: 58, [TIMPANI]: 47, [BRASS]: 61, [CELLOS]: 42 };
const PANS = { [VIOLINS]: 44, [LOW_STRINGS]: 80, [TREMOLO]: 52, [HORNS]: 50, [TROMBONES]: 76, [TUBA]: 70, [TIMPANI]: 64, [BRASS]: 60, [CELLOS]: 84, [DRUMS]: 66 };
for (const channel of [VIOLINS, LOW_STRINGS, TREMOLO, HORNS, TROMBONES, TUBA, TIMPANI, BRASS, CELLOS, DRUMS]) {
  if (channel in PROGRAMS) events.push({ at: 0, order: -1, message: [PROGRAM | channel, PROGRAMS[channel]] });
  control(channel, 0, 7, 100); // volume
  control(channel, 0, 10, PANS[channel]);
  control(channel, 0, 91, 72); // reverb
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

// ── The drop: the whole orchestra on D major ────────────────────────────────────────────────
for (const channel of [VIOLINS, LOW_STRINGS, TREMOLO, HORNS, TROMBONES, TUBA, BRASS]) control(channel, DROP - 0.01, EXPRESSION, 92);
note(TIMPANI, DROP, 1.5, 38, 96);
note(DRUMS, DROP, 2.5, CRASH, 48);
note(TUBA, DROP, 2 * BEAT, 38, 84); // D2
[50, 54, 57].forEach((pitch) => note(TROMBONES, DROP, 2 * BEAT, pitch, 76)); // D3 F#3 A3
[57, 62, 66].forEach((pitch) => note(HORNS, DROP, 2 * BEAT, pitch, 86)); // A3 D4 F#4
[62, 66, 69].forEach((pitch) => note(BRASS, DROP, 1.5 * BEAT, pitch, 54)); // D4 F#4 A4
[38, 50].forEach((pitch) => note(LOW_STRINGS, DROP, 4 * BEAT, pitch, 86)); // D2 D3
[74, 78, 81].forEach((pitch) => note(VIOLINS, DROP, 4 * BEAT, pitch, 80)); // D5 F#5 A5
ramp(HORNS, DROP + 0.3, DROP + 2 * BEAT, 92, 80);
ramp(TROMBONES, DROP + 0.3, DROP + 2 * BEAT, 92, 78);
ramp(VIOLINS, DROP + 0.4, bar(1) - 0.1, 92, 78);

// ── The motion ──────────────────────────────────────────────────────────────────────────────
// A chord each half bar: its bass (low strings, tuba, cellos and timpani), and the tones the
// violins and (outside the horns' call) the horns hold.
const HALVES = [
  // bar 0: D (after the drop's chord)
  { bass: 38, upper: [74, 78, 81], horns: [57, 62, 66], timpani: 38 },
  { bass: 38, upper: [74, 78, 81], horns: [57, 62, 66], timpani: 45 },
  // bar 1: Bb over D, under the horns' call
  { bass: 38, upper: [74, 77, 82], horns: null, timpani: 38 },
  { bass: 38, upper: [74, 77, 82], horns: null, timpani: 45 },
  // bar 2: C, then G over B
  { bass: 36, upper: [72, 76, 79], horns: null, timpani: 36 },
  { bass: 35, upper: [71, 74, 79], horns: null, timpani: 43 },
  // bar 3: A with its fourth, then A
  { bass: 33, upper: [74, 76, 81], horns: null, timpani: 33 },
  { bass: 33, upper: [73, 76, 81], horns: null, timpani: 40 },
];
// The cellos' eighths, by weight: driving, leaning on the first and the fourth beats.
const EIGHTHS = [96, 52, 66, 50, 82, 54, 64, 58];

HALVES.forEach(({ bass, upper, horns, timpani }, index) => {
  const at = DROP + index * 2 * BEAT;
  const first = index === 0;
  // The timpani, on each half bar's first beat (the drop's stroke is already there).
  if (!first) note(TIMPANI, at, 1, timpani, index % 2 === 0 ? 92 : 70);
  // The cellos: eighths on the bass, an octave up.
  for (let eighth = 0; eighth < 4; eighth++) {
    const weight = EIGHTHS[(index % 2) * 4 + eighth];
    note(CELLOS, at + (eighth * BEAT) / 2, BEAT / 2 - 0.06, bass + 12, Math.round(weight * (first ? 0.75 : 0.88)));
  }
  // Low strings and tuba hold the bass; violins and horns the chord.
  if (index >= 2) {
    note(LOW_STRINGS, at, 2 * BEAT - 0.03, bass, 70);
    note(LOW_STRINGS, at, 2 * BEAT - 0.03, bass + 12, 62);
    note(TUBA, at, 2 * BEAT - 0.05, bass, 50);
    upper.forEach((pitch) => note(VIOLINS, at, 2 * BEAT - 0.03, pitch, 60));
  }
  if (horns && !first) horns.forEach((pitch) => note(HORNS, at, 2 * BEAT - 0.05, pitch, 58));
});
ramp(TUBA, bar(1), bar(1) + 0.1, 92, 90);
ramp(LOW_STRINGS, bar(1), bar(1) + 0.1, 92, 96);

// The horns' call, from the title: [bar, beat, MIDI note, beats held, velocity].
const CALL = [
  [1, 0, 65, 1, 96], // F4, as the title comes in
  [1, 1, 70, 1, 100], // Bb4
  [1, 2, 72, 1, 102], // C5
  [1, 3, 74, 1, 106], // D5, as its button comes in
  [2, 0, 76, 2, 108], // E5
  [2, 2, 74, 1, 100], // D5
  [2, 3, 71, 1, 96], // B4
  [3, 0, 74, 2, 104], // D5
  [3, 2, 73, 2, 100], // C#5
  [4, 0, 74, 4, 96], // D5, coming to rest
];
control(HORNS, bar(1) - 0.05, EXPRESSION, 94);
for (const [n, beat, pitch, beats, velocity] of CALL) {
  note(HORNS, bar(n) + beat * BEAT, beats * BEAT - 0.04, pitch, velocity);
  // The trombones an octave below, softer, from the second bar of the call.
  if (n >= 2) note(TROMBONES, bar(n) + beat * BEAT, beats * BEAT - 0.04, pitch - 12, Math.round(velocity * 0.6));
}
note(DRUMS, HEADING, 2, CRASH, 40);

// ── Coming to rest ──────────────────────────────────────────────────────────────────────────
// The motion stops: D, then G over D, all softening; then D major, low and wide, ringing out.
ramp(HORNS, THIN + 0.3, REST, 94, 64);
note(TIMPANI, THIN, 1.4, 38, 74);
note(TUBA, THIN, 4 * BEAT, 38, 60);
note(LOW_STRINGS, THIN, 4 * BEAT - 0.05, 38, 78);
[62, 66, 69, 74].forEach((pitch) => note(VIOLINS, THIN, 2 * BEAT - 0.03, pitch, 70)); // D F# A D
[62, 67, 71, 74].forEach((pitch) => note(VIOLINS, THIN + 2 * BEAT, 2 * BEAT, pitch, 64)); // D G B D
[55, 59].forEach((pitch) => note(TROMBONES, THIN + 2 * BEAT, 2 * BEAT, pitch, 52)); // G B
ramp(VIOLINS, THIN, REST - 0.1, 100, 80);
ramp(LOW_STRINGS, THIN, REST - 0.1, 100, 80);

const RING = DURATION - REST - 1.4;
for (const channel of [VIOLINS, LOW_STRINGS, HORNS, TROMBONES, TUBA]) {
  control(channel, REST - 0.01, EXPRESSION, 92);
  ramp(channel, REST + 0.6, REST + RING, 92, 30);
}
note(TIMPANI, REST, 2.5, 38, 66);
note(DRUMS, REST, 3, CRASH, 30);
note(TUBA, REST, RING, 38, 58); // D2
note(LOW_STRINGS, REST, RING, 38, 72); // D2
note(LOW_STRINGS, REST, RING, 45, 66); // A2
[50, 54, 57].forEach((pitch) => note(TROMBONES, REST, RING, pitch, 48)); // D3 F#3 A3
[62, 66].forEach((pitch) => note(HORNS, REST, RING, pitch, 60)); // D4 F#4
[69, 74, 78].forEach((pitch) => note(VIOLINS, REST, RING, pitch, 66)); // A4 D5 F#5

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
console.log(`peak ${(20 * Math.log10(PEAK)).toFixed(1)} dBFS, rms ${(20 * Math.log10(rms)).toFixed(1)} dBFS, ${DURATION.toFixed(2)} s`);
console.log(
  `orb ${ORB}, open ${OPEN}, drop ${DROP}, heading ${HEADING.toFixed(3)}, continue ${CONTINUE.toFixed(3)}, thin ${THIN.toFixed(3)}, rest ${REST.toFixed(3)}`,
);

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
