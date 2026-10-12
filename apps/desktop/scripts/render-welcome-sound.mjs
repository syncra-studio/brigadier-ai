// Renders the first launch's welcome music, src/app/onboarding/welcome-sound.m4a.
//
//   node scripts/render-welcome-sound.mjs [out.m4a]
//
// Everything is synthesized here, no samples, in F major at 88 beats a minute. It builds,
// then drops, then resolves:
//   - Before the drop there is no beat: a warm pad with no third (so it never settles) rises
//     from silence before anything is seen, with air that swells as the orb comes up and
//     idles, and rises towards the moment it opens into the window (OPEN), where a soft
//     swish of air goes with it; the music keeps building through it, breathing in.
//   - The drop comes as Brigadier's mark comes up in the window (DROP): a soft low thump and
//     a fling of air, and the groove starts. It has a round bass on the beats, a soft mallet
//     playing eighths through each bar's chord, a felt-soft pulse under them, and the pad
//     moving a chord a bar.
//   - The title comes in on the next bar (HEADING), with a chime, and its button on that
//     bar's last beat (CONTINUE), with a two-note accent.
//   - Two more bars carry the settled welcome. Then the parts drop out one by one and the
//     pad comes to rest on F, the first chord with its third, ringing out to silence.
// Those times match the intro's (IntroCanvas.tsx, and the --intro-* tokens in tokens.css).
//
// Writes a WAV, then encodes it to AAC with macOS's afconvert (on other systems the WAV is
// left next to the output instead).

import { execFileSync } from "node:child_process";
import { writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const OUT = resolve(process.argv[2] ?? new URL("../src/app/onboarding/welcome-sound.m4a", import.meta.url).pathname);

const SR = 44_100;
/** When the orb comes up, when it opens into the window, and the drop, in seconds. */
const ORB = 0.75;
const OPEN = 4.25;
const DROP = 4.9;
/** The groove's beat and bar, from the drop. */
const BEAT = 60 / 88;
const BAR = 4 * BEAT;
const bar = (n) => DROP + n * BAR;
/** The title starts on the bar after the drop; its button comes on that bar's last beat. */
const HEADING = bar(1);
const CONTINUE = bar(1) + 3 * BEAT;
/** The groove's last bar starts thinning out here; the pad comes to rest on F a bar later. */
const THIN = bar(4);
const REST = bar(5);
const DURATION = REST + 3.6;
/** The loudest sample, as a fraction of full scale (about -7.5 dBFS). */
const PEAK = 0.42;

const N = Math.round(SR * DURATION);

// A seeded generator, so every render is the same sound.
let seed = 12_102_026;
function random() {
  seed = (seed + 0x6d2b79f5) | 0;
  let x = Math.imul(seed ^ (seed >>> 15), 1 | seed);
  x = (x + Math.imul(x ^ (x >>> 7), 61 | x)) ^ x;
  return ((x ^ (x >>> 14)) >>> 0) / 4_294_967_296;
}
function normal() {
  return Math.sqrt(-2 * Math.log(1 - random())) * Math.cos(2 * Math.PI * random());
}

/** 0 before `start`, 1 after `end`, an S-curve between. */
function smooth(time, start, end) {
  const x = Math.min(1, Math.max(0, (time - start) / (end - start)));
  return x * x * (3 - 2 * x);
}

const hz = (note) => 440 * 2 ** ((note - 69) / 12);

const left = new Float64Array(N);
const right = new Float64Array(N);
const channels = [left, right];

/** Adds `voice(since, channel)` from `at` for `length` seconds. */
function add(at, length, voice) {
  const from = Math.max(0, Math.floor(at * SR));
  const to = Math.min(N, Math.ceil((at + length) * SR));
  for (let c = 0; c < 2; c++) {
    const out = channels[c];
    for (let i = from; i < to; i++) out[i] += voice(i / SR - at, c);
  }
}

// ── The pad ─────────────────────────────────────────────────────────────────────────────────
// Detuned harmonic voices, a chord at a time, each fading into the next.
const PADS = [
  // Before the drop: F with its second and sixth but no third, over C, rising from silence.
  { at: 0, notes: [41, 48, 55, 60, 62], level: 0.55, fadeIn: 3.2 },
  // The groove, a chord a bar: Bbmaj9, Gm11, Dm9, Csus4(add9).
  { at: bar(0), notes: [46, 53, 57, 60, 62], level: 1.1, fadeIn: 0.12 },
  { at: bar(1), notes: [43, 53, 58, 60, 65], level: 1.1, fadeIn: 0.3 },
  { at: bar(2), notes: [50, 53, 57, 60, 64], level: 1.1, fadeIn: 0.3 },
  { at: bar(3), notes: [48, 53, 55, 60, 62], level: 1.1, fadeIn: 0.3 },
  // Thinning out: Bbmaj9 again, then resting on F(add9), with its third at last.
  { at: THIN, notes: [46, 53, 57, 62, 65], level: 0.75, fadeIn: 0.4 },
  { at: REST, notes: [41, 48, 57, 60, 67], level: 0.6, fadeIn: 0.6 },
];
const DETUNE_CENTS = [-6.5, -1.8, 2.4, 6.1];
const HARMONICS = 12;

PADS.forEach((pad, index) => {
  const next = PADS[index + 1];
  const overlap = 0.5;
  const end = next ? next.at + overlap : DURATION;
  const envelope = (time) =>
    smooth(time, pad.at, pad.at + pad.fadeIn) * (next ? 1 - smooth(time, next.at, next.at + overlap) : 1);
  pad.notes.forEach((note, noteIndex) => {
    const frequency = hz(note);
    for (let c = 0; c < 2; c++) {
      const voices = DETUNE_CENTS.map((cents) => ({
        f: frequency * 2 ** ((cents + (c - 0.5) * 1.3) / 1200),
        phases: Array.from({ length: HARMONICS }, () => random() * 2 * Math.PI),
      }));
      const out = channels[c];
      for (let i = Math.floor(pad.at * SR); i < Math.min(N, Math.ceil(end * SR)); i++) {
        const time = i / SR;
        const level = envelope(time);
        if (level === 0) continue;
        // Darker as it rises, opening up towards the drop, softer again as it rests.
        const brightness = 2.5 + 3.5 * smooth(time, 0.5, DROP) - 1.5 * smooth(time, THIN, REST + 2);
        let voice = 0;
        for (const { f, phases } of voices) {
          const phase = 2 * Math.PI * f * time + 0.018 * Math.sin(2 * Math.PI * 0.23 * time + noteIndex + c);
          for (let h = 1; h <= HARMONICS; h++) {
            voice += Math.sin(h * phase + phases[h - 1]) * (1 / h ** 1.65) * Math.exp(-h / brightness);
          }
        }
        const movement = 1 + 0.08 * Math.sin(2 * Math.PI * (0.15 + noteIndex * 0.019) * time + c * 1.3);
        out[i] += (voice * level * movement * pad.level * 0.016) / (1 + noteIndex * 0.12);
      }
    }
  });
});

// ── Air ─────────────────────────────────────────────────────────────────────────────────────
// Soft filtered noise: it swells as the orb comes up, rises and brightens towards the opening,
// swishes with it, breathes in before the drop and is flung out with it, then lies low under
// the groove.
for (let c = 0; c < 2; c++) {
  const noise = Float64Array.from({ length: N }, normal);
  let dark = 0;
  let bright = 0;
  for (let i = 0; i < N; i++) {
    const time = i / SR;
    dark += 0.012 * (noise[i] - dark);
    bright += 0.06 * (noise[i] - bright);
    const swell = smooth(time, ORB - 0.3, ORB + 1.6) * (1 - 0.4 * smooth(time, OPEN, DROP));
    const riser = smooth(time, ORB + 1.6, OPEN) * (time < OPEN ? 1 : Math.exp(-(time - OPEN) / 0.25));
    const inhale = time < DROP ? 0.7 * smooth(time, OPEN + 0.3, DROP) : 0;
    const fling = time < DROP ? 0 : 1.2 * Math.exp(-(time - DROP) / 0.7);
    const bed = 0.25 * smooth(time, DROP, DROP + 1) * (1 - smooth(time, THIN, REST + 2));
    const level = Math.max(swell * 0.6, bed) + 0.4 * riser + inhale + fling;
    const tone = Math.min(1, riser + inhale + 0.6 * fling);
    channels[c][i] += 0.045 * level * ((1 - tone) * dark * 3.5 + tone * bright * 1.2);
  }
}

// ── Bells ───────────────────────────────────────────────────────────────────────────────────
/** A soft bell: each note with a faint octave, ringing out over `ring` seconds. */
function bell(at, notes, ring) {
  add(at, ring * 5, (since, c) => {
    const attack = Math.min(1, since / 0.004);
    let sum = 0;
    for (const { note, level, delay = 0 } of notes) {
      const t = since - delay;
      if (t < 0) continue;
      const f = hz(note) * 2 ** (((c - 0.5) * 2) / 1200);
      sum +=
        level *
        Math.min(1, t / 0.004) *
        Math.exp(-t / ring) *
        (Math.sin(2 * Math.PI * f * t) + 0.16 * Math.exp(-t / 0.3) * Math.sin(2 * Math.PI * 2.76 * f * t));
    }
    return attack * sum;
  });
}

// The title.
bell(HEADING, [
  { note: 81, level: 0.018 }, // A5
  { note: 86, level: 0.01 }, // D6
], 1.4);
// Its button.
bell(CONTINUE, [
  { note: 84, level: 0.022 }, // C6
  { note: 89, level: 0.016, delay: BEAT / 4 }, // F6
], 0.9);

// ── The drop ────────────────────────────────────────────────────────────────────────────────
// A soft, rounded thump falling in pitch, and a low bloom under the groove's first bar.
add(DROP, 4, (since, c) => {
  const pitch = 2 * Math.PI * (48 * since + 0.08 * 62 * (1 - Math.exp(-since / 0.08)));
  const thump = 0.09 * Math.min(1, since / 0.012) * Math.exp(-since / 0.14) * Math.sin(pitch);
  const bloom = Math.min(1, since / 0.03) * Math.exp(-since / 1.6) * 0.035 * Math.sin(2 * Math.PI * hz(34) * since + c * 0.4);
  return thump + bloom;
});

// ── The groove ──────────────────────────────────────────────────────────────────────────────
// Each bar's root (MIDI, bass octave) and the tones the mallet walks through (an octave or
// two above the pad).
const GROOVE = [
  { root: 46, tones: [65, 69, 72, 74, 77] }, // Bbmaj9
  { root: 43, tones: [65, 70, 72, 74, 77] }, // Gm11
  { root: 50, tones: [62, 65, 69, 72, 76] }, // Dm9
  { root: 48, tones: [60, 65, 67, 72, 74] }, // Csus4(add9)
  { root: 46, tones: [62, 65, 69, 74, 77] }, // Bbmaj9, thinning out
];

/** A round bass note: a sine with a little of its octave and twelfth, a soft attack. */
function bass(at, note, length, level) {
  const f = hz(note);
  add(at, length + 0.5, (since, c) => {
    const envelope = Math.min(1, since / 0.025) * Math.exp(-since / 1.4) * (1 - smooth(since, length, length + 0.4));
    const phase = 2 * Math.PI * f * since + c * 0.1;
    return level * envelope * (Math.sin(phase) + 0.32 * Math.sin(2 * phase) + 0.1 * Math.sin(3 * phase));
  });
}

/** A soft mallet: its partials die away, the upper ones first. */
function mallet(at, note, level) {
  const f = hz(note) * 2 ** ((random() - 0.5) * 6 / 1200);
  const pan = 0.5 + (random() - 0.5) * 0.5;
  add(at, 1.4, (since, c) => {
    const attack = Math.min(1, since / 0.006);
    const phase = 2 * Math.PI * f * since;
    const tone =
      Math.exp(-since / 0.42) * Math.sin(phase) +
      0.28 * Math.exp(-since / 0.12) * Math.sin(2 * phase) +
      0.08 * Math.exp(-since / 0.05) * Math.sin(3.98 * phase);
    return level * attack * tone * (c === 0 ? 1 - pan : pan) * 2;
  });
}

/** A felt-soft pulse, more felt than heard. */
function pulse(at, level) {
  add(at, 0.5, (since) => {
    const pitch = 2 * Math.PI * (44 * since + 0.05 * 40 * (1 - Math.exp(-since / 0.05)));
    return level * Math.min(1, since / 0.008) * Math.exp(-since / 0.13) * Math.sin(pitch);
  });
}

// The mallet's walk through each bar, a chord tone (by index) per eighth, with its weight.
const WALK = [
  [0, 1], [2, 0.55], [4, 0.75], [1, 0.5], [3, 0.8], [2, 0.5], [4, 0.65], [1, 0.45],
];

GROOVE.forEach(({ root, tones }, n) => {
  const start = bar(n);
  const thinning = n === GROOVE.length - 1;
  // The bass: the root on the first beat, held; again on the third; the fifth on the fourth.
  if (!thinning) {
    bass(start, root, BEAT * 1.8, 0.07);
    bass(start + 2 * BEAT, root, BEAT * 0.8, 0.055);
    bass(start + 3 * BEAT, root + 7, BEAT * 0.8, 0.05);
  } else {
    bass(start, root, BEAT * 3, 0.06);
  }
  // The pulse, on the first and third beats.
  if (!thinning) {
    pulse(start, 0.045);
    pulse(start + 2 * BEAT, 0.03);
  }
  // The mallet; the last bar fades out across it, every other eighth.
  WALK.forEach(([tone, weight], eighth) => {
    if (thinning && eighth % 2 === 1) return;
    const fade = thinning ? 1 - eighth / 8 : 1;
    // The first bar comes in a little softer after the drop, growing into the second.
    const grow = n === 0 ? 0.75 + 0.25 * (eighth / 8) : 1;
    mallet(start + (eighth * BEAT) / 2, tones[tone], 0.04 * weight * fade * grow);
  });
});
// One last low F, as the pad comes to rest.
bass(REST, 41, 2.6, 0.04);
mallet(REST, 77, 0.016);

// ── Diffuse reflections, each channel fed from the other ────────────────────────────────────
const dry = [Float64Array.from(left), Float64Array.from(right)];
const REFLECTIONS = 32;
for (let r = 0; r < REFLECTIONS; r++) {
  const delay = 0.053 + ((2.1 - 0.053) * r) / (REFLECTIONS - 1);
  const gain = 0.07 * Math.exp(-delay * 1.1);
  for (let c = 0; c < 2; c++) {
    const shift = Math.round((delay + c * 0.008) * SR);
    const from = dry[1 - c];
    const out = channels[c];
    for (let i = shift; i < N; i++) out[i] += from[i - shift] * gain;
  }
}

// ── The whole ───────────────────────────────────────────────────────────────────────────────
// Rising from silence, a quiet fade to silence at the end, no clicks.
let peak = 0;
for (let c = 0; c < 2; c++) {
  const out = channels[c];
  let mean = 0;
  for (let i = 0; i < N; i++) mean += out[i] / N;
  for (let i = 0; i < N; i++) {
    const time = i / SR;
    const envelope = smooth(time, 0, 1.6) * (1 - smooth(time, REST + 1.2, DURATION));
    const edges = Math.min(1, time / 0.025) * Math.min(1, (DURATION - time) / 0.04);
    out[i] = (out[i] - mean) * envelope * edges;
    peak = Math.max(peak, Math.abs(out[i]));
  }
}

const pcm = Buffer.alloc(44 + N * 4);
pcm.write("RIFF", 0);
pcm.writeUInt32LE(36 + N * 4, 4);
pcm.write("WAVEfmt ", 8);
pcm.writeUInt32LE(16, 16);
pcm.writeUInt16LE(1, 20);
pcm.writeUInt16LE(2, 22);
pcm.writeUInt32LE(SR, 24);
pcm.writeUInt32LE(SR * 4, 28);
pcm.writeUInt16LE(4, 32);
pcm.writeUInt16LE(16, 34);
pcm.write("data", 36);
pcm.writeUInt32LE(N * 4, 40);
let squares = 0;
for (let i = 0; i < N; i++) {
  for (let c = 0; c < 2; c++) {
    const value = (channels[c][i] / peak) * PEAK;
    squares += value * value;
    pcm.writeInt16LE(Math.round(value * 32_767), 44 + (i * 2 + c) * 2);
  }
}
const rms = Math.sqrt(squares / (N * 2));
console.log(`peak ${(20 * Math.log10(PEAK)).toFixed(1)} dBFS, rms ${(20 * Math.log10(rms)).toFixed(1)} dBFS, ${DURATION.toFixed(2)} s`);
console.log(
  `orb ${ORB}, open ${OPEN}, drop ${DROP}, heading ${HEADING.toFixed(3)}, continue ${CONTINUE.toFixed(3)}, thin ${THIN.toFixed(3)}, rest ${REST.toFixed(3)}`,
);

if (process.platform !== "darwin") {
  const wav = OUT.replace(/\.m4a$/, ".wav");
  writeFileSync(wav, pcm);
  console.log(`wrote ${wav} (encode it to AAC to replace ${OUT})`);
} else {
  const wav = join(tmpdir(), `welcome-sound-${process.pid}.wav`);
  writeFileSync(wav, pcm);
  try {
    execFileSync("afconvert", ["-f", "m4af", "-d", "aac", "-b", "96000", "-q", "127", wav, OUT]);
  } finally {
    rmSync(wav, { force: true });
  }
  console.log(`wrote ${OUT}`);
}
