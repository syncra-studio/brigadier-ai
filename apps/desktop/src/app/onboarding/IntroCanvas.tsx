import { useEffect, useRef } from "react";

/**
 * The first launch's comet, the mark it becomes, and the gradient the mark opens into, drawn
 * live with WebGL (no video or image files). A bright comet streaks in on a curve and slows at
 * the window's centre; its tail wraps round it and splits into the mark's fifteen strokes as
 * its head settles into the core: the comet becomes Brigadier's mark, soft and glowing, its
 * columns drifting through the disc like the mark's turn. Then the mark flares out past the
 * window's edges and settles into the window's shape, filled with faint white waves moving
 * over the shell's own dark (the colour the conversation sits on), which loop for as long as
 * they show.
 *
 * `start` is when the intro started (a `performance.now()` time); until then it draws nothing.
 * `settled` shows the waves alone, at once; `still` keeps them from moving (reduced motion).
 * Draws at one pixel per point: everything in it is soft.
 */

/** The intro's beats the canvas draws, in seconds from its start (tokens.css lists them all). */
const COMET_IN = 0.25;
/** The comet's head reaches the centre (slowing all the way). */
const COMET_ARRIVE = 2.2;
/** The tail wraps and splits into the strokes, and the head becomes the core, over this span. */
const FORM_FROM = 1.95;
const FORM_TO = 2.9;
const BURST = 3.5;
const BURST_LENGTH = 0.45;
/** The mark has all but filled the window: the window's own chrome comes back. */
export const WINDOW_SHAPED = 3.9;

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
/** How many times bigger the mark gets as it bursts. */
const BURST_SCALE = 9;
/** The columns' drift through the disc, in mark units per second (a column is 14.4 apart). */
const DRIFT_SPEED = 5;
/** The window's corner radius, in points, that the mark settles into. */
const WINDOW_RADIUS = 12;

const VERTEX = `#version 300 es
in vec2 position;
void main() { gl_Position = vec4(position, 0.0, 1.0); }
`;

// The mark's geometry is the master's (src-tauri/icons/mark.svg, brand-glyph.tsx): a disc of
// radius 59 at the centre of a 256-unit grid, bars 9.6 wide and strokes 4.4 wide in columns
// 14.4 apart, the first at x 27.2.
const FRAGMENT = `#version 300 es
precision highp float;
uniform vec2 uSize;
uniform float uTime;
uniform vec2 uTail[${TAIL_POINTS}];
uniform float uComet;
uniform float uForm;
uniform float uMark;
uniform float uScale;
uniform float uBlur;
uniform float uDrift;
uniform float uOpen;
uniform float uFlash;
uniform float uRadius;
uniform vec3 uShell;
out vec4 outColor;

const float GAP = 14.4;
const float FIRST = 27.2 - 128.0;
const float STROKES[15] = float[15](38.4, 108.8, 145.6, 171.2, 185.6, 198.4, 204.8, 211.2,
  204.8, 198.4, 185.6, 171.2, 145.6, 108.8, 38.4);
const float BARS[15] = float[15](0.0, 0.0, 57.6, 97.6, 124.8, 144.0, 153.6, 156.8, 153.6,
  144.0, 124.8, 97.6, 57.6, 0.0, 0.0);

const vec3 WHITE = vec3(1.0);
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

// A column's height at its drifted place: the height of the column ahead, taken over as it
// goes; columns grow in from the middle out as the mark forms.
float columnHeight(int k, float f, bool bars) {
  float a = (k >= 0 && k < 15) ? (bars ? BARS[k] : STROKES[k]) : 0.0;
  float b = (k + 1 >= 0 && k + 1 < 15) ? (bars ? BARS[k + 1] : STROKES[k + 1]) : 0.0;
  float order = abs(float(k) + f - 7.0) / 7.0;
  return mix(a, b, f) * smoothstep(0.15 + order * 0.45, 0.5 + order * 0.5, uForm);
}

// Distance to the nearest upright capsule of a column set (width w, centred on y 0).
float columns(vec2 p, float w, bool bars) {
  float f = uDrift / GAP;
  float k0 = floor((p.x - FIRST - uDrift) / GAP + 0.5);
  float d = 1e5;
  for (int o = -1; o <= 1; o++) {
    int k = int(k0) + o;
    float h = columnHeight(k, f, bars);
    if (h <= 0.0) continue;
    float r = min(w, h) * 0.5;
    vec2 q = p - vec2(FIRST + float(k) * GAP + uDrift, 0.0);
    q.y = max(abs(q.y) - (h * 0.5 - r), 0.0);
    d = min(d, length(q) - r);
  }
  return d;
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

vec3 waves(vec2 px) {
  // Broad, soft veils of white light drifting over one another on the shell's dark, faint
  // enough that the heading stays the brightest thing on screen.
  vec2 uv = px / uSize.y;
  float t = uTime * 0.06;
  vec2 q = vec2(fbm(vec3(uv * 0.55, t), 2), fbm(vec3(uv * 0.55 + vec2(5.2, 1.3), t + 3.0), 2));
  float f = fbm(vec3(uv * 0.75 + 0.9 * q, t * 1.3 + 7.0), 3);
  float veil = 0.015 * smoothstep(-0.7, -0.25, f)
    + 0.03 * smoothstep(-0.3, 0.2, f)
    + 0.065 * smoothstep(0.05, 0.5, f + 0.25 * q.x)
    + 0.03 * smoothstep(0.35, 0.75, f + 0.2 * q.y);
  return mix(uShell, WHITE, veil);
}

void main() {
  vec2 px = gl_FragCoord.xy - uSize * 0.5;

  // The mark, soft and glowing, in mark units; its disc grows from the comet's head.
  vec2 p = px / uScale;
  float r = length(p);
  float angle = atan(p.y, p.x);
  float wobble = 0.03 * snoise(vec3(cos(angle) * 1.2, sin(angle) * 1.2, uTime * 0.45));
  float disc = r - 59.0 * smoothstep(0.1, 0.75, uForm) * (1.0 + wobble);
  float bars = columns(p, 9.6, true);
  float strokes = columns(p, 4.4, false);
  float discA = 1.0 - smoothstep(-uBlur * 2.5, uBlur * 2.5, disc);
  float barA = 1.0 - smoothstep(-uBlur * 1.6, uBlur * 1.6, bars);
  // The columns give way as it bursts, so the disc alone opens into the window.
  float columnsLeft = 1.0 - smoothstep(0.0, 0.45, uOpen);
  float coreA = max(discA, barA * 0.9 * columnsLeft);
  float strokeA = (1.0 - smoothstep(-uBlur, uBlur, strokes)) * 0.7 * columnsLeft;
  float glow = exp(-max(disc, 0.0) / 38.0) * 0.32 * smoothstep(0.2, 0.8, uForm);
  float swirl = snoise(vec3(p / 48.0, uTime * 0.35));
  vec3 markCol = mix(MARK_EDGE, PEARL, clamp(0.6 + 0.3 * swirl - 0.4 * smoothstep(20.0, 62.0, r), 0.0, 1.0));
  float rim = pow(max(cos(angle - uTime * 1.25), 0.0), 8.0) * smoothstep(30.0, 58.0, r) * (1.0 - smoothstep(58.0, 70.0, r));
  markCol = mix(markCol, PEARL, rim * 0.9);
  float shape = max(max(coreA, strokeA), glow);
  float markA = shape * uMark;
  vec3 markOut = mix(MARK_EDGE, markCol, shape > 0.0 ? coreA / shape : 0.0);

  // The comet over it, fading as the mark takes its place.
  vec4 c = comet(px);
  float cometA = c.a * uComet;
  float figureA = cometA + markA * (1.0 - cometA);
  vec3 figure = (c.rgb * cometA + markOut * markA * (1.0 - cometA)) / max(figureA, 1e-4);

  // The window it opens into.
  float edge = mix(140.0, 0.75, uOpen * uOpen);
  float box = roundedBox(px, uSize * 0.5 - 0.5, uRadius);
  float boxA = 1.0 - smoothstep(-edge, edge, box);
  vec3 windowCol = waves(gl_FragCoord.xy) + PEARL * uFlash;

  float a = mix(figureA, boxA, smoothstep(0.4, 1.0, uOpen));
  vec3 col = mix(figure, windowCol, smoothstep(0.35, 1.0, uOpen));
  // A little grain, so the dark waves don't band.
  float grain = fract(sin(dot(gl_FragCoord.xy, vec2(12.9898, 78.233))) * 43758.5453) - 0.5;
  col += grain / 255.0 * 1.5;
  outColor = vec4(clamp(col, 0.0, 1.0) * a, a);
}
`;

/** The shell's background (`--background`, what the conversation sits on) as sRGB, 0–1. */
function shellColour(): [number, number, number] {
  const probe = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  const value = getComputedStyle(document.documentElement).getPropertyValue("--background").trim();
  if (!probe || !value) return [0.095, 0.095, 0.091];
  probe.fillStyle = value;
  probe.fillRect(0, 0, 1, 1);
  const [r = 24, g = 24, b = 23] = probe.getImageData(0, 0, 1, 1).data;
  return [r / 255, g / 255, b / 255];
}

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
  blur: number;
  drift: number;
  open: number;
  flash: number;
}

/** Everything drawn `t` seconds in (null: not started), on a canvas of `width` × `height` points. */
function frame(t: number | null, settled: boolean, width: number, height: number): Frame {
  const base = (MARK_WIDTH * width) / 206;
  const tail = new Float32Array(TAIL_POINTS * 2);
  const rest = { tail, comet: 0, form: 1, mark: 0, scale: base, blur: 2.2, drift: 0, flash: 0 };
  if (settled) return { ...rest, open: 1 };
  if (t === null) return { ...rest, open: 0 };
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
  const burst = clamp01((t - BURST) / BURST_LENGTH);
  return {
    tail,
    comet: clamp01((t - COMET_IN) / 0.15) * (1 - ease(clamp01((form - 0.35) / 0.55))),
    form,
    mark: ease(clamp01((form - 0.1) / 0.5)),
    scale: base * BURST_SCALE ** burst,
    // Soft as it forms; the burst blurs it again as it grows.
    blur: 2.2 + 6 * (1 - form) + 6 * burst,
    drift: ((t - FORM_TO) * DRIFT_SPEED + 40 * burst ** 2 + 14.4 * 10) % 14.4,
    open: ease(burst),
    flash: 0.05 * (1 - ease(clamp01((t - BURST - 0.25) / 0.6))) * clamp01(burst * 3),
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
      time: uniform("uTime"),
      tail: uniform("uTail"),
      comet: uniform("uComet"),
      form: uniform("uForm"),
      mark: uniform("uMark"),
      scale: uniform("uScale"),
      blur: uniform("uBlur"),
      drift: uniform("uDrift"),
      open: uniform("uOpen"),
      flash: uniform("uFlash"),
      radius: uniform("uRadius"),
      shell: uniform("uShell"),
    };
    gl.uniform3f(u.shell, ...shellColour());
    // The waves keep moving from wherever the noise is, settled or not.
    const epoch = performance.now();
    let raf = 0;
    let drawn = "";
    const draw = (now: number) => {
      raf = requestAnimationFrame(draw);
      const width = element.clientWidth;
      const height = element.clientHeight;
      const { start: begun, settled: done, still: frozen } = props.current;
      // Held still, it draws again only when something changes.
      const key = `${width}x${height}:${begun}:${done}`;
      if (frozen && key === drawn) return;
      drawn = key;
      if (element.width !== width || element.height !== height) {
        element.width = width;
        element.height = height;
      }
      gl.viewport(0, 0, width, height);
      const state = frame(begun === null ? null : (now - begun) / 1000, done, width, height);
      gl.uniform2f(u.size, width, height);
      gl.uniform1f(u.time, frozen ? 20 : (now - epoch) / 1000 + 20);
      gl.uniform2fv(u.tail, state.tail);
      gl.uniform1f(u.comet, state.comet);
      gl.uniform1f(u.form, state.form);
      gl.uniform1f(u.mark, state.mark);
      gl.uniform1f(u.scale, state.scale);
      gl.uniform1f(u.blur, state.blur);
      gl.uniform1f(u.drift, state.drift);
      gl.uniform1f(u.open, state.open);
      gl.uniform1f(u.flash, state.flash);
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
