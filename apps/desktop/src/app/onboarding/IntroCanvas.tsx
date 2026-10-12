import { useEffect, useRef } from "react";

/**
 * The first launch's orb, the window it opens into and the waves on it, drawn live with WebGL
 * (no video or image files). A soft orb of pearl and silver light comes into focus at the
 * window's centre and idles there, slowly changing shape and growing, its light moving round
 * inside it. Then it opens out, blurred, into the window's rounded shape; its light settles
 * onto the shell's own dark, and faint light waves move over it, on a loop for as long as they
 * show. (Brigadier's mark, which comes up next, is the page's: Welcome.tsx.)
 *
 * `start` is when the intro started (a `performance.now()` time); until then it draws nothing.
 * `settled` shows the waves alone, at once; `still` keeps them from moving (reduced motion).
 * Draws at the screen's pixel density (up to 2×).
 */

/** The intro's beats the canvas draws, in seconds from its start (tokens.css lists them all). */
const ORB_IN = 0.75;
/** The orb comes into focus over this long. */
const ORB_FOCUS = 0.45;
/** The orb opens out into the window... */
const OPEN = 4.25;
/** ...filling it this long after... */
const OPEN_FOR = 0.13;
/** ...and its light settles onto the dark by here. */
const SETTLED = 4.85;
/** The window's chrome comes back. */
export const WINDOW_SHAPED = 4.8;
/** The waves come up on the dark over this span. */
const WAVES_FROM = 4.4;
const WAVES_TO = 5.8;

/** The orb's radius as a share of the window's width, as it comes in and as it opens. */
const ORB_FROM = 0.075;
const ORB_TO = 0.11;
/** The window's corner radius, in points, that the orb opens into. */
const WINDOW_RADIUS = 12;
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
uniform float uFocus;
uniform float uOpen;
uniform float uBlur;
uniform float uField;
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

// The orb's light at px (points from the centre): a soft body whose edge slowly changes shape,
// a bright patch circling inside it and a dimmer one opposite, and a faint halo.
vec4 orb(vec2 px, float R) {
  float r = length(px);
  vec2 dir = r > 0.0 ? px / r : vec2(1.0, 0.0);
  float t = uTime;
  float wobble = 0.1 * snoise(vec3(dir * 0.55, t * 0.2)) + 0.035 * snoise(vec3(dir * 1.1 + 4.0, t * 0.3));
  float edge = R * (1.0 + wobble);
  float soft = R * (0.42 + 0.9 * (1.0 - uFocus));
  float body = 1.0 - smoothstep(edge - soft, edge + soft, r);
  float halo = 0.3 * exp(-max(r - edge, 0.0) / (R * 0.8));
  // The light moving round inside it.
  float a = t * 2.0 * PI / 2.7;
  vec2 bright = R * 0.42 * vec2(cos(a), sin(a * 0.83 + 0.6));
  vec2 dim = -R * 0.38 * vec2(cos(a * 0.9 + 1.1), sin(a + 0.2));
  float lit = exp(-dot(px - bright, px - bright) / (R * R * 0.22));
  float shade = exp(-dot(px - dim, px - dim) / (R * R * 0.3));
  float inner = 1.0 - smoothstep(0.0, edge, r);
  vec3 col = mix(SILVER * 0.9, PEARL * 0.82, 0.55 * inner + 0.2);
  col = mix(col, PEARL, 0.75 * lit) * (1.0 - 0.28 * shade);
  float alpha = clamp(body * 0.96 + halo * (1.0 - body), 0.0, 1.0);
  return vec4(col * alpha, alpha);
}

float waves(vec2 pt) {
  // Broad, soft veils of light drifting over one another on the dark, faint enough that the
  // title stays the brightest thing on screen.
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
  // Half a device pixel, in points: the softest an edge gets.
  float pixel = 0.5 / uDensity.x;

  // The orb, before it opens.
  vec4 light = uOrb > 0.0 ? orb(px, uRadius) * uOrb : vec4(0.0);

  // Opening: a growing blurred circle, held inside the window's rounded shape, of the orb's
  // light settling onto the dark.
  float reach = length(uSize * 0.5) * 1.15;
  float grow = 1.0 - pow(1.0 - clamp(uOpen, 0.0, 1.0), 3.0);
  float circle = length(px) - mix(uRadius, reach, grow);
  float box = roundedBox(px, uSize * 0.5, uCorner);
  float windowA = (1.0 - smoothstep(-uBlur, uBlur, max(circle, box))) * step(0.0, uOpen);
  windowA *= 1.0 - smoothstep(-pixel, pixel, box);
  // The orb's light, smeared over the window, fading as the dark settles.
  vec2 uv = pt / uSize.y;
  float cloud = 0.55 + 0.45 * snoise(vec3(uv * 1.3, uTime * 0.3));
  vec3 field = mix(SILVER, PEARL, cloud) * (0.25 + 0.55 * cloud) * uField;
  vec3 windowCol = uDark + (1.0 - uDark) * (waves(pt) * uWaves);
  windowCol = windowCol * (1.0 - uField * 0.85) + field;

  vec3 col = light.rgb * (1.0 - windowA) + windowCol * windowA;
  float a = light.a * (1.0 - windowA) + windowA;
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
  focus: number;
  /** 0 → 1 as the orb opens out (negative before it). */
  open: number;
  blur: number;
  field: number;
  waves: number;
}

/** Everything drawn `t` seconds in (null: not started), on a canvas of `width` points. */
function frame(t: number | null, settled: boolean, width: number): Frame {
  const rest = { orb: 0, radius: ORB_FROM * width, focus: 1, blur: 1, field: 0 };
  if (settled) return { ...rest, open: 1, waves: 1 };
  if (t === null) return { ...rest, open: -1, waves: 0 };
  const shown = ease(clamp01((t - ORB_IN) / ORB_FOCUS));
  const settling = clamp01((t - OPEN - OPEN_FOR) / (SETTLED - OPEN - OPEN_FOR));
  return {
    orb: t < OPEN + OPEN_FOR ? shown : 0,
    // Slowly growing, and swelling a little as it opens.
    radius: width * (ORB_FROM + (ORB_TO - ORB_FROM) * ease(clamp01((t - ORB_IN) / (OPEN - ORB_IN)))),
    focus: shown,
    open: t < OPEN ? -1 : (t - OPEN) / OPEN_FOR,
    // Blurred as it opens, sharp once settled.
    blur: 1 + 70 * (1 - ease(settling)),
    field: t < OPEN ? 0 : 1 - ease(settling),
    waves: ease(clamp01((t - WAVES_FROM) / (WAVES_TO - WAVES_FROM))),
  };
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
      focus: uniform("uFocus"),
      open: uniform("uOpen"),
      blur: uniform("uBlur"),
      field: uniform("uField"),
      waves: uniform("uWaves"),
      corner: uniform("uCorner"),
      dark: uniform("uDark"),
    };
    gl.uniform3fv(u.dark, shellDark());
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
      const state = frame(begun === null ? null : (now - begun) / 1000, done, width);
      gl.uniform2f(u.size, width, height);
      // The actual device pixels per point, so everything lands where it would at 1×.
      gl.uniform2f(u.density, pixelsWide / Math.max(width, 1), pixelsHigh / Math.max(height, 1));
      gl.uniform1f(u.time, frozen ? 20 : (now - epoch) / 1000 + 20);
      gl.uniform1f(u.orb, state.orb);
      gl.uniform1f(u.radius, state.radius);
      gl.uniform1f(u.focus, state.focus);
      gl.uniform1f(u.open, state.open);
      gl.uniform1f(u.blur, state.blur);
      gl.uniform1f(u.field, state.field);
      gl.uniform1f(u.waves, state.waves);
      gl.uniform1f(u.corner, WINDOW_RADIUS);
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
