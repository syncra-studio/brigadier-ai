import { useEffect, useRef } from "react";

/**
 * The first launch's comet, the mark it becomes, the burst the mark ends in and the waves left
 * after it, drawn live with WebGL (no video or image files). A bright comet streaks in on a
 * curve and slows at the window's centre; its tail wraps round it and splits into the mark's
 * fifteen strokes as its head settles into the disc: the comet becomes Brigadier's mark, crisp
 * and white, which holds for a beat. Then the mark draws in, white-hot, and bursts like a dying
 * star: a flash, its strokes flung outward as streaks, sparks, and a shockwave whose inside is
 * the window, pure black, opening out past the window's corners. Faint white waves then move
 * over the black, on a loop for as long as they show.
 *
 * `start` is when the intro started (a `performance.now()` time); until then it draws nothing.
 * `settled` shows the waves alone, at once; `still` keeps them from moving (reduced motion).
 * Draws at the screen's pixel density (up to 2×), so the mark's edges are sharp.
 */

/** The intro's beats the canvas draws, in seconds from its start (tokens.css lists them all). */
const COMET_IN = 0.25;
/** The comet's head reaches the centre (slowing all the way). */
const COMET_ARRIVE = 2.2;
/** The tail wraps and splits into the strokes, and the head becomes the disc, over this span. */
const FORM_FROM = 1.95;
const FORM_TO = 2.9;
/** The formed mark holds until here, then draws in, white-hot... */
const COLLAPSE = 3.55;
/** ...and bursts. */
const DETONATE = 3.68;
/** The shockwave has passed the window's corners: the window's own chrome comes back. */
export const WINDOW_SHAPED = 4.05;
/** The waves come up on the black over this span. */
const WAVES_FROM = 3.9;
const WAVES_TO = 5.2;

/**
 * The comet's path: where its head is at `u` (0 entering, 1 at the centre), in window
 * half-sizes from the centre (x right, y up). Swap `COMET_PATH` to try another way in.
 */
type CometPath = (u: number) => [number, number];

/** Streaks in from beyond the lower left, sweeping up and round into the centre. */
const streak: CometPath = (u) => {
  const v = 1 - u;
  const bezier = (a: number, b: number, c: number, d: number) =>
    v * v * v * a + 3 * v * v * u * b + 3 * v * u * u * c + u * u * u * d;
  return [bezier(-1.25, -0.55, 0.62, 0), bezier(-0.85, 0.95, 0.55, 0)];
};

const COMET_PATH: CometPath = streak;

/** Points along the tail (the head first), and how far back in time the last one is. */
const TAIL_POINTS = 24;
const TAIL_SECONDS = 0.7;
/** How wide the whole mark is, as a share of the window's width, once formed. */
const MARK_WIDTH = 0.34;
/** How far the mark draws in before it bursts. */
const COLLAPSE_BY = 0.1;
/** The window's corner radius, in points, that the burst opens into. */
const WINDOW_RADIUS = 12;
/** The most device pixels drawn per point. */
const MAX_DENSITY = 2;

const VERTEX = `#version 300 es
in vec2 position;
void main() { gl_Position = vec4(position, 0.0, 1.0); }
`;

// Everything is placed in points (CSS pixels), whatever the canvas's pixel density: `uDensity`
// is the canvas's device pixels per point, and only edges' softness uses it.
//
// The mark's geometry is the master's (src-tauri/icons/mark.svg, brand-glyph.tsx): a disc of
// radius 59 at the centre of a 256-unit grid, bars 9.6 wide and strokes 4.4 wide in columns
// 14.4 apart, the first at x 27.2, all with round ends, all one colour.
const FRAGMENT = `#version 300 es
precision highp float;
uniform vec2 uSize;
uniform vec2 uDensity;
uniform float uTime;
uniform vec2 uTail[${TAIL_POINTS}];
uniform float uComet;
uniform float uForm;
uniform float uMark;
uniform float uScale;
uniform float uSoft;
uniform float uHot;
uniform float uBlast;
uniform float uReach;
uniform float uWaves;
uniform float uOpen;
uniform float uRadius;
out vec4 outColor;

const float PI = 3.14159265;
const float GAP = 14.4;
const float FIRST = 27.2 - 128.0;
const float DISC = 59.0;
const float STROKES[15] = float[15](38.4, 108.8, 145.6, 171.2, 185.6, 198.4, 204.8, 211.2,
  204.8, 198.4, 185.6, 171.2, 145.6, 108.8, 38.4);
const float BARS[15] = float[15](0.0, 0.0, 57.6, 97.6, 124.8, 144.0, 153.6, 156.8, 153.6,
  144.0, 124.8, 97.6, 57.6, 0.0, 0.0);

const vec3 PEARL = vec3(0.949, 0.961, 1.0);
const vec3 MARK_EDGE = vec3(0.553, 0.604, 0.710);

// 3D simplex noise (after Ashima Arts / Stefan Gustavson, public domain).
vec4 permute(vec4 x) { return mod(((x * 34.0) + 1.0) * x, 289.0); }
vec4 taylorInvSqrt(vec4 r) { return 1.79284291400159 - 0.85373472095314 * r; }
float snoise(vec3 v) {
  const vec2 C = vec2(1.0 / 6.0, 1.0 / 3.0);
  vec3 i = floor(v + dot(v, C.yyy));
  vec3 x0 = v - i + dot(i, C.xxx);
  vec3 g = step(x0.yzx, x0.xyz);
  vec3 l = 1.0 - g;
  vec3 i1 = min(g.xyz, l.zxy);
  vec3 i2 = max(g.xyz, l.zxy);
  vec3 x1 = x0 - i1 + C.xxx;
  vec3 x2 = x0 - i2 + 2.0 * C.xxx;
  vec3 x3 = x0 - 1.0 + 3.0 * C.xxx;
  i = mod(i, 289.0);
  vec4 p = permute(permute(permute(i.z + vec4(0.0, i1.z, i2.z, 1.0)) + i.y
    + vec4(0.0, i1.y, i2.y, 1.0)) + i.x + vec4(0.0, i1.x, i2.x, 1.0));
  vec3 ns = vec3(0.285714285714, -0.928571428571, 0.142857142857);
  vec4 j = p - 49.0 * floor(p * ns.z * ns.z);
  vec4 x_ = floor(j * ns.z);
  vec4 y_ = floor(j - 7.0 * x_);
  vec4 x = x_ * ns.x + ns.yyyy;
  vec4 y = y_ * ns.x + ns.yyyy;
  vec4 h = 1.0 - abs(x) - abs(y);
  vec4 b0 = vec4(x.xy, y.xy);
  vec4 b1 = vec4(x.zw, y.zw);
  vec4 s0 = floor(b0) * 2.0 + 1.0;
  vec4 s1 = floor(b1) * 2.0 + 1.0;
  vec4 sh = -step(h, vec4(0.0));
  vec4 a0 = b0.xzyw + s0.xzyw * sh.xxyy;
  vec4 a1 = b1.xzyw + s1.xzyw * sh.zzww;
  vec3 p0 = vec3(a0.xy, h.x);
  vec3 p1 = vec3(a0.zw, h.y);
  vec3 p2 = vec3(a1.xy, h.z);
  vec3 p3 = vec3(a1.zw, h.w);
  vec4 norm = taylorInvSqrt(vec4(dot(p0, p0), dot(p1, p1), dot(p2, p2), dot(p3, p3)));
  p0 *= norm.x; p1 *= norm.y; p2 *= norm.z; p3 *= norm.w;
  vec4 m = max(0.6 - vec4(dot(x0, x0), dot(x1, x1), dot(x2, x2), dot(x3, x3)), 0.0);
  m = m * m;
  return 42.0 * dot(m * m, vec4(dot(p0, x0), dot(p1, x1), dot(p2, x2), dot(p3, x3)));
}

float fbm(vec3 p, int octaves) {
  float sum = 0.0;
  float amp = 0.5;
  for (int i = 0; i < octaves; i++) {
    sum += amp * snoise(p);
    p = p * 2.03 + vec3(1.7, 9.2, 3.1);
    amp *= 0.5;
  }
  return sum;
}

float hash(float x) { return fract(sin(x * 12.9898) * 43758.5453); }

// How much of a shape at signed distance d is covered, its edge softened over w either side.
float cover(float d, float w) { return 1.0 - smoothstep(-w, w, d); }

// Distance to the nearest round-ended column of a set (width w, centred on y 0), in mark
// units; columns grow in from the middle out as the mark forms.
float columns(vec2 p, float w, bool bars) {
  float k0 = floor((p.x - FIRST) / GAP + 0.5);
  float d = 1e5;
  for (int o = -1; o <= 1; o++) {
    int k = int(k0) + o;
    if (k < 0 || k > 14) continue;
    float order = abs(float(k) - 7.0) / 7.0;
    float h = (bars ? BARS[k] : STROKES[k]) * smoothstep(0.15 + order * 0.45, 0.5 + order * 0.5, uForm);
    if (h <= 0.0) continue;
    float r = min(w, h) * 0.5;
    vec2 q = p - vec2(FIRST + float(k) * GAP, 0.0);
    q.y = max(abs(q.y) - (h * 0.5 - r), 0.0);
    d = min(d, length(q) - r);
  }
  return d;
}

// The strokes and bars together (the disc aside).
float allColumns(vec2 p) {
  return min(columns(p, 9.6, true), columns(p, 4.4, false));
}

float roundedBox(vec2 p, vec2 half_, float r) {
  vec2 q = abs(p) - half_ + r;
  return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}

// The comet: a bright head and a tail that thins and fades behind it, in points.
vec4 comet(vec2 px) {
  float core = 0.0;
  float halo = 0.0;
  for (int i = 0; i < ${TAIL_POINTS - 1}; i++) {
    vec2 a = uTail[i];
    vec2 b = uTail[i + 1];
    vec2 ab = b - a;
    float h = clamp(dot(px - a, ab) / max(dot(ab, ab), 1e-4), 0.0, 1.0);
    float d = length(px - a - ab * h);
    float along = (float(i) + h) / ${TAIL_POINTS - 1}.0;
    float fade = pow(1.0 - along, 1.6);
    float width = mix(5.0, 0.8, along);
    core = max(core, fade * exp(-(d * d) / (width * width)));
    halo = max(halo, fade * exp(-d / (width * 5.0)) * 0.4);
  }
  float head = length(px - uTail[0]);
  core = max(core, exp(-(head * head) / 90.0));
  halo = max(halo, exp(-head / 34.0) * 0.6);
  float a = clamp(core + halo, 0.0, 1.0);
  vec3 col = mix(MARK_EDGE, PEARL, clamp(core * 1.4, 0.0, 1.0));
  return vec4(col, a);
}

// A round-ended upright bar of width w and height h, centred on q's origin.
float capsule(vec2 q, float w, float h) {
  if (h <= 0.0) return 1e5;
  float r = min(w, h) * 0.5;
  q.y = max(abs(q.y) - (h * 0.5 - r), 0.0);
  return length(q) - r;
}

// The mark's strokes broken into pieces and flung straight out from the centre, each its own
// size, at one of a few speeds, streaking a little behind it. A point is traced back to the
// pieces that could be over it, one speed at a time.
const float PIECE = 16.0;
float debris(vec2 px, float pixel) {
  float x = clamp(uBlast / 0.7, 0.0, 1.0);
  float travel = 1.0 - pow(1.0 - x, 3.0);
  vec2 p = px / uScale;
  float soft = pixel / uScale + 0.5 * x;
  float lit = 0.0;
  for (int s = 0; s < 5; s++) {
    float push = 5.0 * (0.55 + 0.25 * float(s)) * travel;
    vec2 guess = p / (1.0 + push);
    float k0 = floor((guess.x - FIRST) / GAP + 0.5);
    float b0 = floor(guess.y / PIECE);
    for (int i = -1; i <= 1; i++) {
      float k = k0 + float(i);
      if (k < 0.0 || k > 14.0) continue;
      int column = int(k);
      for (int j = -1; j <= 1; j++) {
        float b = b0 + float(j);
        float id = k * 31.0 + b;
        if (floor(hash(id + 0.37) * 5.0) != float(s) || hash(id + 5.1) < 0.15) continue;
        vec2 centre = vec2(FIRST + k * GAP, (b + 0.5) * PIECE);
        // What the disc covered goes in the flash.
        if (length(centre) < DISC * 0.85) continue;
        for (int t = 0; t < 3; t++) {
          float behind = float(t) / 2.0;
          vec2 q = p - centre * push * (1.0 - (0.05 + 0.2 * (1.0 - x)) * behind);
          vec2 c = q - vec2(centre.x, 0.0);
          float d = min(capsule(c, 4.4, STROKES[column]), capsule(c, 9.6, BARS[column]));
          d = max(d, abs(q.y - centre.y) - (PIECE * 0.5 - 0.8));
          lit = max(lit, cover(d, soft) * (1.0 - 0.75 * behind));
        }
      }
    }
  }
  return lit * pow(1.0 - x, 1.8);
}

// Sparks thrown out in every direction, one at most per sliver of angle.
float sparks(vec2 px, float r, float slivers, float seed, float reach) {
  float x = clamp(uBlast / 1.0, 0.0, 1.0);
  float angle = atan(px.y, px.x);
  float b = floor((angle + PI) / (2.0 * PI) * slivers);
  float h1 = hash(b + seed);
  float h2 = hash(b * 1.7 + seed + 3.0);
  float h3 = hash(b * 2.3 + seed + 7.0);
  if (hash(b * 3.1 + seed + 11.0) < 0.35) return 0.0;
  float aim = (b + 0.5 + (h2 - 0.5) * 0.6) / slivers * 2.0 * PI - PI;
  float speed = 0.45 + 0.9 * h1;
  float head = DISC * uScale + reach * speed * (1.0 - pow(1.0 - x, 3.0));
  float len = (12.0 + 70.0 * h3) * (1.0 - x) + 3.0;
  float off = r * abs(sin(angle - aim));
  float width = 0.6 + 0.8 * h3;
  float along = clamp((r - head + len) / len, 0.0, 1.0) * step(r, head);
  return exp(-(off * off) / (width * width)) * along * along * pow(1.0 - x, 1.2) * (0.5 + 0.5 * h1);
}

// A shockwave's bright edge at radius R and the glow trailing inside it.
float shock(float r, float x, float reach) {
  float R = reach * (1.0 - pow(1.0 - x, 3.0));
  float w = 2.0 + 8.0 * x;
  float edge = exp(-pow((r - R) / w, 2.0));
  float inside = exp(-max(R - r, 0.0) / (20.0 + 140.0 * x)) * step(r, R);
  return (1.0 - x) * (edge + 0.35 * inside);
}

float waves(vec2 pt) {
  // Broad, soft veils of white light drifting over one another on the black, faint enough
  // that the title stays the brightest thing on screen.
  vec2 uv = pt / uSize.y;
  float t = uTime * 0.06;
  vec2 q = vec2(fbm(vec3(uv * 0.55, t), 2), fbm(vec3(uv * 0.55 + vec2(5.2, 1.3), t + 3.0), 2));
  float f = fbm(vec3(uv * 0.75 + 0.9 * q, t * 1.3 + 7.0), 3);
  return 0.015 * smoothstep(-0.7, -0.25, f)
    + 0.03 * smoothstep(-0.3, 0.2, f)
    + 0.065 * smoothstep(0.05, 0.5, f + 0.25 * q.x)
    + 0.03 * smoothstep(0.35, 0.75, f + 0.2 * q.y);
}

void main() {
  vec2 pt = gl_FragCoord.xy / uDensity;
  vec2 px = pt - uSize * 0.5;
  float r = length(px);
  // Half a device pixel, in points: the softest an edge gets.
  float pixel = 0.5 / uDensity.x;
  bool blown = uBlast >= 0.0;

  // The mark, crisp and white, in mark units; its disc grows from the comet's head. Bursting,
  // the disc goes in the flash and the strokes become the debris.
  vec2 p = px / uScale;
  float edge = pixel / uScale + uSoft;
  float disc = cover(r / uScale - DISC * smoothstep(0.1, 0.75, uForm), edge * 2.0);
  float strokes = blown ? 0.0 : cover(allColumns(p), edge);
  float markA = max(disc * (1.0 - smoothstep(0.0, 0.06, uBlast)), strokes) * uMark;
  // White-hot as it draws in.
  float shape = min(r / uScale - DISC, allColumns(p));
  float glow = uHot * 0.6 * exp(-max(shape, 0.0) * uScale / (4.0 + 16.0 * uHot));

  // The comet over it, fading as the mark takes its place.
  vec4 c = comet(px);
  float cometA = c.a * uComet;
  vec3 figure = c.rgb * cometA + vec3(markA * (1.0 - cometA));
  float figureA = cometA + markA * (1.0 - cometA);

  // The burst: a flash at the core, the debris, sparks and the shockwave, all white light.
  float light = glow;
  float inside = 0.0;
  if (blown) {
    float x = clamp(uBlast / 0.5, 0.0, 1.0);
    float size = DISC * uScale * (1.0 + 5.0 * uBlast);
    light += exp(-uBlast / 0.2) * exp(-pow(r / size, 2.0));
    light += debris(px, pixel);
    light += sparks(px, r, 140.0, 0.0, uReach * 1.05) + 0.7 * sparks(px, r, 67.0, 41.0, uReach * 0.7);
    light += shock(r, x, uReach);
    // Inside the shockwave is the window, black.
    inside = cover(r - uReach * (1.0 - pow(1.0 - x, 3.0)), 1.5);
    // The whole of it lit for a moment.
    light += 0.12 * exp(-uBlast / 0.15) * inside;
  }
  light = clamp(light, 0.0, 1.0);
  figure = figure * (1.0 - light) + vec3(light);
  figureA = figureA + light * (1.0 - figureA);

  // The window: black, with the waves on it; the full canvas once the window has its shape.
  float windowA = max(inside * cover(roundedBox(px, uSize * 0.5, uRadius), pixel), uOpen);
  vec3 windowCol = vec3(windowA > 0.0 && uWaves > 0.0 ? waves(pt) * uWaves : 0.0) * windowA;

  vec3 col = figure + windowCol * (1.0 - figureA);
  float a = figureA + windowA * (1.0 - figureA);
  // A little grain, so the faint waves don't band.
  float grain = fract(sin(dot(gl_FragCoord.xy, vec2(12.9898, 78.233))) * 43758.5453) - 0.5;
  col += grain / 255.0 * 1.5 * a;
  outColor = vec4(clamp(col, 0.0, a), a);
}
`;

const ease = (x: number) => x * x * (3 - 2 * x);
const clamp01 = (x: number) => Math.min(1, Math.max(0, x));
const easeOut = (x: number) => 1 - (1 - x) ** 2;

/** Where the comet's head is at `t` seconds, in points from the centre. */
function cometAt(t: number, width: number, height: number): [number, number] {
  const [x, y] = COMET_PATH(easeOut(clamp01((t - COMET_IN) / (COMET_ARRIVE - COMET_IN))));
  return [(x * width) / 2, (y * height) / 2];
}

interface Frame {
  tail: Float32Array;
  comet: number;
  form: number;
  mark: number;
  scale: number;
  soft: number;
  hot: number;
  /** Seconds since the burst (negative before it). */
  blast: number;
  waves: number;
  open: number;
}

/** Everything drawn `t` seconds in (null: not started), on a canvas of `width` × `height` points. */
function frame(t: number | null, settled: boolean, width: number, height: number): Frame {
  const base = (MARK_WIDTH * width) / 206;
  const tail = new Float32Array(TAIL_POINTS * 2);
  const rest = { tail, comet: 0, form: 1, mark: 0, scale: base, soft: 0, hot: 0 };
  if (settled) return { ...rest, blast: 60, waves: 1, open: 1 };
  if (t === null) return { ...rest, blast: -1, waves: 0, open: 0 };
  const form = clamp01((t - FORM_FROM) / (FORM_TO - FORM_FROM));
  // The tail curls round the head as the mark forms, out to the strokes' ring, turning as it goes.
  const wrap = ease(clamp01(form / 0.6));
  const ring = 110 * base;
  for (let i = 0; i < TAIL_POINTS; i++) {
    const along = i / (TAIL_POINTS - 1);
    const [x, y] = cometAt(t - along * TAIL_SECONDS, width, height);
    const angle = Math.PI * 0.75 - along * Math.PI * 1.8 - form * 2.4;
    const radius = ring * ease(clamp01(along / 0.4));
    tail[i * 2] = x + (Math.cos(angle) * radius - x) * wrap;
    tail[i * 2 + 1] = y + (Math.sin(angle) * radius - y) * wrap;
  }
  const collapse = ease(clamp01((t - COLLAPSE) / (DETONATE - COLLAPSE)));
  return {
    tail,
    comet: clamp01((t - COMET_IN) / 0.15) * (1 - ease(clamp01((form - 0.35) / 0.55))),
    form,
    mark: ease(clamp01((form - 0.1) / 0.5)),
    scale: base * (1 - COLLAPSE_BY * collapse),
    // Soft as it forms, sharp once formed (in mark units).
    soft: 7 * (1 - form) ** 1.5,
    hot: t < DETONATE ? collapse : 0,
    blast: t - DETONATE,
    waves: ease(clamp01((t - WAVES_FROM) / (WAVES_TO - WAVES_FROM))),
    open: clamp01((t - WINDOW_SHAPED) / 0.1),
  };
}

export function IntroCanvas({
  start,
  settled,
  still,
  onUnavailable,
  className,
  onTransitionEnd,
}: {
  start: number | null;
  settled: boolean;
  still: boolean;
  /** WebGL can't draw here: the intro shows its end instead. */
  onUnavailable: () => void;
  className?: string;
  onTransitionEnd?: () => void;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const props = useRef({ start, settled, still, onUnavailable });
  useEffect(() => {
    props.current = { start, settled, still, onUnavailable };
  }, [start, settled, still, onUnavailable]);

  useEffect(() => {
    const element = canvas.current;
    const gl = element?.getContext("webgl2", { antialias: false, premultipliedAlpha: true, alpha: true });
    const program = gl && link(gl);
    if (!element || !gl || !program) {
      console.error("the intro can't draw with WebGL; it shows its end");
      props.current.onUnavailable();
      return;
    }
    // Called through the prototype: a bare `gl.useProgram` reads as a React hook to the linter.
    WebGL2RenderingContext.prototype.useProgram.call(gl, program);
    const buffer = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    const position = gl.getAttribLocation(program, "position");
    gl.enableVertexAttribArray(position);
    gl.vertexAttribPointer(position, 2, gl.FLOAT, false, 0, 0);
    const uniform = (name: string) => gl.getUniformLocation(program, name);
    const u = {
      size: uniform("uSize"),
      density: uniform("uDensity"),
      time: uniform("uTime"),
      tail: uniform("uTail"),
      comet: uniform("uComet"),
      form: uniform("uForm"),
      mark: uniform("uMark"),
      scale: uniform("uScale"),
      soft: uniform("uSoft"),
      hot: uniform("uHot"),
      blast: uniform("uBlast"),
      reach: uniform("uReach"),
      waves: uniform("uWaves"),
      open: uniform("uOpen"),
      radius: uniform("uRadius"),
    };
    // The waves keep moving from wherever the noise is, settled or not.
    const epoch = performance.now();
    let raf = 0;
    let drawn = "";
    const draw = (now: number) => {
      raf = requestAnimationFrame(draw);
      const width = element.clientWidth;
      const height = element.clientHeight;
      const density = Math.min(window.devicePixelRatio || 1, MAX_DENSITY);
      const { start: begun, settled: done, still: frozen } = props.current;
      // Held still, it draws again only when something changes.
      const key = `${width}x${height}@${density}:${begun}:${done}`;
      if (frozen && key === drawn) return;
      drawn = key;
      const pixelsWide = Math.max(1, Math.round(width * density));
      const pixelsHigh = Math.max(1, Math.round(height * density));
      if (element.width !== pixelsWide || element.height !== pixelsHigh) {
        element.width = pixelsWide;
        element.height = pixelsHigh;
      }
      gl.viewport(0, 0, pixelsWide, pixelsHigh);
      const state = frame(begun === null ? null : (now - begun) / 1000, done, width, height);
      gl.uniform2f(u.size, width, height);
      // The actual device pixels per point, so everything lands where it would at 1×.
      gl.uniform2f(u.density, pixelsWide / Math.max(width, 1), pixelsHigh / Math.max(height, 1));
      gl.uniform1f(u.time, frozen ? 20 : (now - epoch) / 1000 + 20);
      gl.uniform2fv(u.tail, state.tail);
      gl.uniform1f(u.comet, state.comet);
      gl.uniform1f(u.form, state.form);
      gl.uniform1f(u.mark, state.mark);
      gl.uniform1f(u.scale, state.scale);
      gl.uniform1f(u.soft, state.soft);
      gl.uniform1f(u.hot, state.hot);
      gl.uniform1f(u.blast, state.blast);
      // A little past the window's corners.
      gl.uniform1f(u.reach, Math.hypot(width, height) * 0.54 + 24);
      gl.uniform1f(u.waves, state.waves);
      gl.uniform1f(u.open, state.open);
      gl.uniform1f(u.radius, WINDOW_RADIUS);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    };
    raf = requestAnimationFrame(draw);
    return () => {
      cancelAnimationFrame(raf);
      gl.deleteBuffer(buffer);
      gl.deleteProgram(program);
    };
  }, []);

  return <canvas ref={canvas} aria-hidden className={className} onTransitionEnd={onTransitionEnd} />;
}

function link(gl: WebGL2RenderingContext): WebGLProgram | null {
  const shader = (type: number, source: string) => {
    const made = gl.createShader(type);
    if (!made) return null;
    gl.shaderSource(made, source);
    gl.compileShader(made);
    if (!gl.getShaderParameter(made, gl.COMPILE_STATUS)) {
      console.error("an intro shader didn't compile", gl.getShaderInfoLog(made));
      return null;
    }
    return made;
  };
  const vertex = shader(gl.VERTEX_SHADER, VERTEX);
  const fragment = shader(gl.FRAGMENT_SHADER, FRAGMENT);
  const program = gl.createProgram();
  if (!vertex || !fragment || !program) return null;
  gl.attachShader(program, vertex);
  gl.attachShader(program, fragment);
  gl.linkProgram(program);
  gl.deleteShader(vertex);
  gl.deleteShader(fragment);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    console.error("the intro's shaders didn't link", gl.getProgramInfoLog(program));
    return null;
  }
  return program;
}
