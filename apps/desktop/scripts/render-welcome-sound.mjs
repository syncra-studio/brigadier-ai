// Renders the first launch's welcome sound, src/app/onboarding/welcome-sound.m4a.
//
//   node scripts/render-welcome-sound.mjs [out.m4a]
//
// Everything is synthesized here, no samples: detuned harmonic voices moving through
// F(add9), Bbmaj7 and F6/9, quiet filtered air, and diffuse reflections of the voices. The
// air swells as the comet streaks in, into the moment it has become the crisp mark (LAND),
// where a soft bell and a low swell arrive with it; the air draws in as the mark does
// (COLLAPSE), and the burst (BURST) comes with a soft low thump, a deep bloom and a breath of
// air flung outward; the chords change as the title and then its button come in. Those times
// match the intro's (the comet's beats in IntroCanvas.tsx, --intro-heading and
// --intro-continue in tokens.css).
//
// Writes a WAV, then encodes it to AAC with macOS's afconvert (on other systems the WAV is
// left next to the output instead).

import { execFileSync } from "node:child_process";
import { writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const OUT = resolve(process.argv[2] ?? new URL("../src/app/onboarding/welcome-sound.m4a", import.meta.url).pathname);

const SR = 44_100;
const DURATION = 10.8;
/**
 * When the comet has become the mark, the mark draws in, it bursts, the title starts and its
 * button appears, in seconds.
 */
const LAND = 2.9;
const COLLAPSE = 3.55;
const BURST = 3.68;
const HEADING = 4.7;
const CONTINUE = 6.4;
/** When the whole starts fading out. */
const RELEASE = 8.6;
/** The loudest sample, as a fraction of full scale (about -7.5 dBFS). */
const PEAK = 0.42;

const N = Math.round(SR * DURATION);

// A seeded generator, so every render is the same sound.
let seed = 9_102_026;
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

// The chords: when each starts, its MIDI notes, its level.
const CHORDS = [
  { at: 0, notes: [41, 53, 60, 67, 69], level: 0.8 }, // F(add9)
  { at: HEADING - 0.4, notes: [46, 53, 57, 65, 69], level: 0.5 }, // Bbmaj7
  { at: CONTINUE - 0.2, notes: [41, 53, 60, 62, 67], level: 0.52 }, // F6/9
];
const DETUNE_CENTS = [-6.5, -1.8, 2.4, 6.1];
const HARMONICS = 13;

CHORDS.forEach((chord, index) => {
  const next = CHORDS[index + 1];
  const envelope = (time) =>
    smooth(time, chord.at, chord.at + 1.7) * (next ? 1 - smooth(time, next.at, next.at + 2.3) : 1);
  const last = next ? Math.min(N, Math.ceil((next.at + 2.3) * SR)) : N;
  chord.notes.forEach((note, noteIndex) => {
    const frequency = hz(note);
    for (let c = 0; c < 2; c++) {
      const voices = DETUNE_CENTS.map((cents) => ({
        f: frequency * 2 ** ((cents + (c - 0.5) * 1.3) / 1200),
        phases: Array.from({ length: HARMONICS }, () => random() * 2 * Math.PI),
      }));
      const out = channels[c];
      for (let i = Math.floor(chord.at * SR); i < last; i++) {
        const time = i / SR;
        const level = envelope(time);
        if (level === 0) continue;
        // Upper harmonics open up as the sound swells into the landing.
        const brightness = 3 + 5 * smooth(time, 0.5, LAND + 0.4);
        let voice = 0;
        for (const { f, phases } of voices) {
          const phase = 2 * Math.PI * f * time + 0.018 * Math.sin(2 * Math.PI * 0.23 * time + noteIndex + c);
          for (let h = 1; h <= HARMONICS; h++) {
            voice += Math.sin(h * phase + phases[h - 1]) * (1 / h ** 1.65) * Math.exp(-h / brightness);
          }
        }
        const movement = 1 + 0.09 * Math.sin(2 * Math.PI * (0.15 + noteIndex * 0.019) * time + c * 1.3);
        out[i] += (voice * level * movement * chord.level * 0.022) / (1 + noteIndex * 0.1);
      }
    }
  });
});

// Soft air that swells into the landing and thins out after it, draws in with the mark and is
// flung out with the burst.
for (let c = 0; c < 2; c++) {
  const noise = Float64Array.from({ length: N }, normal);
  const width = 75;
  let sum = 0;
  for (let i = 0; i < N; i++) {
    sum += noise[i] - (i >= width ? noise[i - width] : 0);
    const time = i / SR;
    const swell = smooth(time, 0.1, LAND) * (1 - smooth(time, LAND, LAND + 2.2));
    const bed = 0.35 * smooth(time, LAND, LAND + 1) * (1 - smooth(time, 7.6, 10));
    const burst = time < BURST ? 0.8 * smooth(time, COLLAPSE - 0.25, BURST) : 1.1 * Math.exp(-(time - BURST) / 0.9);
    channels[c][i] += (sum / width) * 0.09 * Math.max(swell, bed, burst);
  }
}

// The landing: a soft bell over the chord and a low swell under it.
const BELL = [
  { note: 77, level: 0.03 }, // F5
  { note: 81, level: 0.016 }, // A5
  { note: 84, level: 0.012 }, // C6
];
for (let c = 0; c < 2; c++) {
  for (let i = Math.floor(LAND * SR); i < N; i++) {
    const time = i / SR;
    const since = time - LAND;
    const attack = Math.min(1, since / 0.06);
    let bell = 0;
    for (const { note, level } of BELL) {
      const f = hz(note) * 2 ** (((c - 0.5) * 2) / 1200);
      bell +=
        level * Math.exp(-since / 1.25) * (Math.sin(2 * Math.PI * f * since) + 0.18 * Math.sin(4 * Math.PI * f * since));
    }
    const low = 0.06 * smooth(since, 0, 0.18) * Math.exp(-since / 1.6) * Math.sin(2 * Math.PI * hz(41) * since);
    channels[c][i] += attack * bell + low;
  }
}

// The burst: a soft, rounded thump falling in pitch, then a deep bloom an octave below the
// landing's swell, with its octave and fifth, that rings out slowly.
const BLOOM = [
  { note: 29, level: 0.05 }, // F1
  { note: 41, level: 0.07 }, // F2
  { note: 48, level: 0.03 }, // C3
];
for (let c = 0; c < 2; c++) {
  let thump = 0;
  for (let i = Math.floor(BURST * SR); i < N; i++) {
    const since = i / SR - BURST;
    thump += (2 * Math.PI * (50 + 60 * Math.exp(-since / 0.08))) / SR;
    let bloom = 0;
    for (const { note, level } of BLOOM) bloom += level * Math.sin(2 * Math.PI * hz(note) * since + c * 0.4);
    channels[c][i] +=
      0.08 * smooth(since, 0, 0.015) * Math.exp(-since / 0.12) * Math.sin(thump) +
      smooth(since, 0, 0.04) * Math.exp(-since / 2.2) * bloom;
  }
}

// A high shimmer that blooms after the landing.
for (let c = 0; c < 2; c++) {
  for (let i = 0; i < N; i++) {
    const time = i / SR;
    const level = 0.008 * smooth(time, LAND, LAND + 1.6) * (1 - smooth(time, 4.4, RELEASE));
    if (level > 0) channels[c][i] += level * Math.sin(2 * Math.PI * 698.456 * time + 0.02 * Math.sin(time * 1.8 + c));
  }
}

// Diffuse reflections, each channel fed from the other.
const dry = [Float64Array.from(left), Float64Array.from(right)];
const REFLECTIONS = 28;
for (let r = 0; r < REFLECTIONS; r++) {
  const delay = 0.061 + ((1.9 - 0.061) * r) / (REFLECTIONS - 1);
  const gain = 0.075 * Math.exp(-delay);
  for (let c = 0; c < 2; c++) {
    const shift = Math.round((delay + c * 0.008) * SR);
    const from = dry[1 - c];
    const out = channels[c];
    for (let i = shift; i < N; i++) out[i] += from[i - shift] * gain;
  }
}

// The whole: a quiet start that swells into the landing, a gentle release, no clicks.
let peak = 0;
for (let c = 0; c < 2; c++) {
  const out = channels[c];
  let mean = 0;
  for (let i = 0; i < N; i++) mean += out[i] / N;
  for (let i = 0; i < N; i++) {
    const time = i / SR;
    const envelope = (0.035 + 0.965 * smooth(time, 0, LAND)) * (1 - smooth(time, RELEASE, DURATION));
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
console.log(`peak ${(20 * Math.log10(PEAK)).toFixed(1)} dBFS, rms ${(20 * Math.log10(rms)).toFixed(1)} dBFS, ${DURATION} s`);

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
