import { useLayoutEffect, useRef } from "react";

/**
 * Text that comes in a character at a time: each one fades in, comes into focus, turns
 * upright from its bottom-left corner and rises a line into place on a light spring, a little
 * after the one before. Words never break apart. Read out as one text (the caller labels the
 * element it sits in); shown at rest while `play` is false, or once it turns false.
 */
export function StaggerText({
  text,
  play,
  delayMs,
  staggerMs,
}: {
  text: string;
  play: boolean;
  /** When the first character starts, from when `play` turns true. */
  delayMs: number;
  /** How long each character waits after the one before. */
  staggerMs: number;
}) {
  const root = useRef<HTMLSpanElement>(null);

  // Before paint, so no character shows at rest for a frame first.
  useLayoutEffect(() => {
    if (!play || !root.current) return;
    const animations = [...root.current.querySelectorAll<HTMLElement>(".stagger-char")].map(
      (char, index) =>
        char.animate(SPRING_KEYFRAMES, {
          duration: SPRING_MS,
          delay: delayMs + index * staggerMs,
          easing: "linear",
          fill: "backwards",
        }),
    );
    // Stopping (settled early, or gone) leaves every character at rest.
    return () => animations.forEach((animation) => animation.cancel());
  }, [play, delayMs, staggerMs]);

  let index = 0;
  return (
    <span ref={root} aria-hidden>
      {text.split(" ").map((word, w) => (
        <span key={w}>
          {w > 0 && " "}
          <span className="stagger-word">
            {[...word].map((char) => (
              <span key={index++} className="stagger-char">
                {char}
              </span>
            ))}
          </span>
        </span>
      ))}
    </span>
  );
}

/**
 * A spring that settles in about 0.6 s with a little bounce (damping ratio 0.74), sampled as
 * keyframes so it plays the same on every WebKit the app supports (CSS `linear()` needs a
 * newer one).
 */
const SPRING_MS = 900;
const SPRING_RESPONSE_S = 0.6;
const SPRING_DAMPING = 0.74;
/** How far out of focus, turned and low each character starts. */
const START_BLUR_PX = 10;
const START_TURN_DEG = 20;

function springAt(seconds: number): number {
  const omega = (2 * Math.PI) / SPRING_RESPONSE_S;
  const decay = SPRING_DAMPING * omega;
  const damped = omega * Math.sqrt(1 - SPRING_DAMPING ** 2);
  return 1 - Math.exp(-decay * seconds) * (Math.cos(damped * seconds) + (decay / damped) * Math.sin(damped * seconds));
}

const SPRING_KEYFRAMES: Keyframe[] = Array.from({ length: 37 }, (_, step) => {
  const offset = step / 36;
  const value = step === 36 ? 1 : springAt((offset * SPRING_MS) / 1000);
  const left = 1 - value;
  return {
    offset,
    opacity: Math.min(1, Math.max(0, value)),
    filter: `blur(${(Math.max(0, left) * START_BLUR_PX).toFixed(2)}px)`,
    transform: `translateY(${left.toFixed(4)}em) rotate(${(left * START_TURN_DEG).toFixed(3)}deg)`,
  };
});
