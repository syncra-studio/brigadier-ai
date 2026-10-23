import { useEffect, useRef } from "react";

/**
 * The first launch's orb, the window it opens into and the waves on it, drawn live with WebGL
 * (no video or image files). A faint glow at the window's centre grows into a soft, lumpy orb
 * of pearl and silver light, always a little blurred, with lights moving round its rim; it
 * idles there, slowly changing shape and settling a touch smaller. Just before it opens it
 * swells and dims a little, then it bursts out, blurred, filling the window's rounded shape,
 * which shows faintly around it as it grows. Its light spreads into a mist over the whole
 * window, evens out and dims, and the shell's own dark comes into focus under it, with faint
 * light waves moving over it, on a loop for as long as they show. (Brigadier's mark, which
 * comes up next, is the page's: Welcome.tsx.)
 *
 * `start` is when the intro started (a `performance.now()` time); until then it draws nothing.
 * `settled` shows the waves alone, at once; `still` keeps them from moving (reduced motion).
 * Draws at the screen's pixel density (up to 2×).
 */

/** The intro's beats the canvas draws, in seconds from its start (tokens.css lists them all). */
const ORB_IN = 0.58;
/** The orb's light comes up over this long, and it grows from a faint glow to its full size
 *  over the longer one. */
const ORB_LIGHT = 0.55;
const ORB_GROW = 0.75;
/** It swells a little over this long before it opens... */
const SWELL = 0.23;
/** ...opens out into the window (its rounded shape showing faintly a moment after)... */
const OPEN = 4.07;
/** ...filling it this long after... */
const OPEN_FOR = 0.27;
/** ...then its light evens out into a mist over the window... */
const EVEN = { from: OPEN + 0.2, to: OPEN + 0.5 };
/** ...and the shell's dark and its waves come into focus under it. */
const SETTLE = { from: OPEN + 0.48, to: OPEN + 0.82 };
/** The window's chrome, shadow and colour come back, as the dark comes into focus. */
export const WINDOW_SHAPED = OPEN + 0.62;

/** The orb's radius as a share of the window's width, grown, and as it has settled by the swell. */
const ORB_FULL = 0.14;
const ORB_IDLE = 0.122;
/** A growing glow starts at this share of the orb's size. */
const ORB_SEED = 0.25;
/** The orb sits this share of the window's height above its centre. */
const ORB_RISE = 0.03;
/** The window's corner radius, in points, that the orb opens into, if it can't be read from the
 *  page (tokens.css --radius-window: macOS's rounded corners; square on Windows). */
const FALLBACK_RADIUS = 12;
/** The most device pixels drawn per point. */
const MAX_DENSITY = 2;
/** The shell's dark, if it can't be read from the page (tokens.css --background). */
const FALLBACK_DARK: [number, number, number] = [0.094, 0.094, 0.094];

const VERTEX = `#version 300 es
in vec2 position;
void main() { gl_Position = vec4(position, 0.0, 1.0); }
`;
// Everything is placed in points (CSS pixels), whatever the canvas's pixel density: `uDensity`
// is the canvas's device pixels per point, and only edges' softness uses it.
const FRAGMENT = `#version 300 es
precision highp float;
uniform vec2 uSize;
uniform vec2 uDensity;
uniform float uTime;
uniform float uOrb;
uniform float uRadius;
uniform vec2 uCentre;
uniform float uSoft;
uniform float uOpen;
uniform float uBox;
uniform float uEven;
uniform float uSettle;
uniform float uWaves;
uniform float uCorner;
uniform vec3 uDark;
out vec4 outColor;

const float PI = 3.14159265;
const vec3 PEARL = vec3(0.949, 0.961, 1.0);
const vec3 SILVER = vec3(0.553, 0.604, 0.710);

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


float roundedBox(vec2 p, vec2 half_, float r) {
  vec2 q = abs(p) - half_ + r;
  return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}

// The orb's light at c (points from its centre), premultiplied: a soft body whose edge is
// lumpy and slowly changes shape, always a little blurred, with lights moving round its rim
// and a deeper shade drifting through it. Its light is placed in the orb's own size, so as it
// bursts out its lights spread with it, over the window.
vec4 orb(vec2 c, float R) {
  float r = length(c);
  vec2 dir = r > 0.0 ? c / r : vec2(1.0, 0.0);
  float t = uTime;
  // Its outline: a few slow, rounded lobes, each growing and fading on its own.
  float th = atan(dir.y, dir.x);
  float lump = 0.1 * (0.6 + 0.6 * snoise(vec3(t * 0.21, 1.0, 0.0))) * sin(2.0 * th + t * 0.47 + 1.3)
    + 0.08 * (0.6 + 0.6 * snoise(vec3(t * 0.17, 5.0, 0.0))) * sin(3.0 * th - t * 0.38 + 0.2)
    + 0.04 * (0.6 + 0.6 * snoise(vec3(t * 0.25, 9.0, 0.0))) * sin(4.0 * th + t * 0.61 + 2.2);
  float edge = R * (1.0 + lump);
  float soft = R * uSoft;
  float body = 1.0 - smoothstep(edge - soft, edge + soft, r);
  vec2 q = c / R;
  float a = t * 2.0 * PI / 3.4;
  vec2 rim1 = 0.6 * vec2(cos(a), sin(a * 0.9 + 0.5));
  vec2 rim2 = 0.64 * vec2(cos(-a * 0.73 + 2.4), sin(-a * 0.81 + 1.9));
  vec2 deep = 0.4 * vec2(cos(a * 0.55 + 4.0), sin(a * 0.6 + 1.0));
  // The lights are smeared by the noise, so they're never round.
  vec2 w = 0.12 * vec2(snoise(vec3(q * 0.8, t * 0.35)), snoise(vec3(q * 0.8 + 7.0, t * 0.35)));
  float lit = exp(-dot(q + w - rim1, q + w - rim1) / 0.12) + 0.7 * exp(-dot(q + w - rim2, q + w - rim2) / 0.09);
  float shade = exp(-dot(q + w - deep, q + w - deep) / 0.2);
  float centre = exp(-dot(q, q) / 0.6);
  vec3 col = mix(SILVER * 1.05, PEARL * 0.78, 0.45 + 0.25 * centre);
  col = mix(col, PEARL, clamp(0.7 * lit, 0.0, 1.0)) * (1.0 - 0.2 * shade);
  return vec4(col * body, body);
}

// Broad, soft veils of light drifting over one another on the dark, faint enough that the
// title stays the brightest thing on screen; focus below 1 blurs them, wider and softer.
float waves(vec2 pt, float focus) {
  vec2 uv = pt / uSize.y;
  float t = uTime * 0.06;
  vec2 q = vec2(fbm(vec3(uv * 0.55, t), 2), fbm(vec3(uv * 0.55 + vec2(5.2, 1.3), t + 3.0), 2));
  float f = fbm(vec3(uv * 0.75 + 0.9 * q, t * 1.3 + 7.0), 3);
  float b = 0.5 * (1.0 - focus);
  return 0.015 * smoothstep(-0.7 - b, -0.25 + b, f)
    + 0.03 * smoothstep(-0.3 - b, 0.2 + b, f)
    + 0.065 * smoothstep(0.05 - b, 0.5 + b, f + 0.25 * q.x)
    + 0.03 * smoothstep(0.35 - b, 0.75 + b, f + 0.2 * q.y);
}

// The S-curve the orb bursts out on: a slow start, most of it in the middle, easing to a stop.
float burst(float x) {
  x = pow(clamp(x, 0.0, 1.0), 1.25);
  return x * x * (3.0 - 2.0 * x);
}

void main() {
  vec2 pt = gl_FragCoord.xy / uDensity;
  vec2 px = pt - uSize * 0.5;
  // Half a device pixel, in points: the softest an edge gets.
  float pixel = 0.5 / uDensity.x;

  // The orb, growing out past the window's corners as it bursts.
  float reach = length(uSize * 0.5) * 1.45;
  float R = uOpen < 0.0 ? uRadius : mix(uRadius, reach, burst(uOpen));
  vec4 light = orb(px - uCentre, R) * uOrb;
  // Spread out, its light thins into a mist, which evens out and dims.
  float spread = uOpen < 0.0 ? 0.0 : burst(uOpen);
  light.rgb *= 1.0 - 0.4 * spread;
  vec3 mist = mix(light.rgb, vec3(0.32, 0.33, 0.36) * light.a, uEven);

  // The window's rounded shape: it shows faintly round the growing orb, then holds all of it.
  float box = roundedBox(px, uSize * 0.5, uCorner);
  float inside = 1.0 - smoothstep(-pixel, pixel, box);
  float shown = uOpen < 0.0 ? 1.0 : inside;
  float boxA = uBox * inside;
  vec3 col = mist * shown + uDark * boxA * (1.0 - light.a * shown);
  float a = light.a * shown + boxA * (1.0 - light.a * shown);

  // Under the mist, the shell's dark comes into focus, with its waves.
  vec3 settled = uDark + (1.0 - uDark) * (waves(pt, uSettle) * uWaves);
  col = mix(col, settled * inside, uSettle);
  a = mix(a, inside, uSettle);
  // A little grain, so the faint waves don't band.
  float grain = fract(sin(dot(gl_FragCoord.xy, vec2(12.9898, 78.233))) * 43758.5453) - 0.5;
  col += grain / 255.0 * 1.5 * a;
  outColor = vec4(clamp(col, 0.0, a), a);
}
`;

const ease = (x: number) => x * x * (3 - 2 * x);
const clamp01 = (x: number) => Math.min(1, Math.max(0, x));

interface Frame {
  orb: number;
  radius: number;
  centre: [number, number];
  /** How blurred the orb's edge is, as a share of its radius. */
  soft: number;
  /** 0 → 1 as the orb opens out (negative before it). */
  open: number;
  box: number;
  even: number;
  settle: number;
  waves: number;
}

/** Everything drawn `t` seconds in (null: not started), on a canvas of `width` × `height` points. */
function frame(t: number | null, settled: boolean, width: number, height: number): Frame {
  const rest = { orb: 0, radius: ORB_IDLE * width, centre: [0, 0] as [number, number], soft: 0.48, box: 1, even: 1 };
  if (settled) return { ...rest, open: 1, settle: 1, waves: 1 };
  if (t === null) return { ...rest, open: -1, box: 0, even: 0, settle: 0, waves: 0 };
  // Growing from a faint glow, part S-curve and part easing out (as measured off the motion
  // this follows), its light coming up quicker than its size; its edge firms up a little once
  // grown, then softens again as it idles.
  const since = t - ORB_IN;
  const grow = (x: number) => 0.5 * (ease(x) + 1 - (1 - x) ** 2);
  const size = grow(clamp01(since / ORB_GROW));
  const light = 1 - (1 - clamp01(since / ORB_LIGHT)) ** 2;
  const firm = ease(clamp01((since - 0.45) / 0.25)) * (1 - ease(clamp01((since - 1.15) / 0.35)));
  // Idling, it settles a touch smaller; then it swells and dims a little just before it opens.
  const idle = ease(clamp01((t - ORB_IN - ORB_GROW) / (OPEN - SWELL - ORB_IN - ORB_GROW)));
  const swell = ease(clamp01((t - OPEN + SWELL) / SWELL));
  const full = ORB_FULL + (ORB_IDLE - ORB_FULL) * idle;
  const radius = width * full * (ORB_SEED + (1 - ORB_SEED) * size) * (1 + 0.17 * swell);
  // It drifts a little round its place.
  const drift = 0.016 * width;
  const centre: [number, number] = [
    drift * Math.sin(t * 0.9 + 0.4),
    ORB_RISE * height + drift * 0.8 * Math.sin(t * 0.7 + 2.1),
  ];
  return {
    orb: since < 0 ? 0 : light * (1 - 0.1 * swell),
    radius,
    centre,
    soft: 0.48 + 0.12 * (1 - size) - 0.26 * firm,
    open: t < OPEN ? -1 : (t - OPEN) / OPEN_FOR,
    box: t < OPEN ? 0 : ease(clamp01((t - OPEN - 0.06) / 0.12)),
    even: ease(clamp01((t - EVEN.from) / (EVEN.to - EVEN.from))),
    settle: ease(clamp01((t - SETTLE.from) / (SETTLE.to - SETTLE.from))),
    waves: 1,
  };
}

/** The page's --radius-window, in points. */
function windowRadius(): number {
  const root = document.documentElement;
  const value = getComputedStyle(root).getPropertyValue("--radius-window").trim();
  const size = Number.parseFloat(value);
  if (!Number.isFinite(size)) return FALLBACK_RADIUS;
  return value.endsWith("rem") ? size * Number.parseFloat(getComputedStyle(root).fontSize) : size;
}

/** The page's --background, as 0–1 sRGB (read through a 2D canvas, which resolves any colour). */
function shellDark(): [number, number, number] {
  const value = getComputedStyle(document.documentElement).getPropertyValue("--background").trim();
  const context = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  if (!value || !context) return FALLBACK_DARK;
  context.fillStyle = value;
  context.fillRect(0, 0, 1, 1);
  const [r = 0, g = 0, b = 0] = context.getImageData(0, 0, 1, 1).data;
  return [r / 255, g / 255, b / 255];
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
      orb: uniform("uOrb"),
      radius: uniform("uRadius"),
      centre: uniform("uCentre"),
      soft: uniform("uSoft"),
      open: uniform("uOpen"),
      box: uniform("uBox"),
      even: uniform("uEven"),
      settle: uniform("uSettle"),
      waves: uniform("uWaves"),
      corner: uniform("uCorner"),
      dark: uniform("uDark"),
    };
    gl.uniform3fv(u.dark, shellDark());
    const corner = windowRadius();
    // The orb and the waves keep moving from wherever the noise is, settled or not.
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
      gl.uniform1f(u.orb, state.orb);
      gl.uniform1f(u.radius, state.radius);
      gl.uniform2f(u.centre, state.centre[0], state.centre[1]);
      gl.uniform1f(u.soft, state.soft);
      gl.uniform1f(u.open, state.open);
      gl.uniform1f(u.box, state.box);
      gl.uniform1f(u.even, state.even);
      gl.uniform1f(u.settle, state.settle);
      gl.uniform1f(u.waves, state.waves);
      gl.uniform1f(u.corner, corner);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    };
    raf = requestAnimationFrame(draw);
    return () => {
      cancelAnimationFrame(raf);
      gl.deleteBuffer(buffer);
      gl.deleteProgram(program);
      // Its window-sized buffers go now, not whenever the canvas is collected (#569).
      element.width = 1;
      element.height = 1;
      gl.getExtension("WEBGL_lose_context")?.loseContext();
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
