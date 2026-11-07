/**
 * What can move a Browser tab's page, heard as events, so a still page costs no frames at all.
 *
 * A size change is heard by a ResizeObserver on the targets and their ancestors, which runs after
 * layout and before paint: whatever a follower does there reaches the same frame as the change.
 * Anything else (a menu mounting, a class or an inline style changing, a scroll, a transition
 * starting) asks for one frame, in which every follower looks again. Frames go on only while a
 * transition or animation runs on a target or one of its ancestors, which moves it with no event.
 */

type Follower = {
  /** What to keep in place; their ancestors move them too. Read again whenever anything changes. */
  targets: () => (Element | null | undefined)[];
  run: () => void;
  observer: ResizeObserver;
  observed: Element[];
};

const followers = new Set<Follower>();
let frame = 0;
let stopListening: (() => void) | null = null;

const WATCHED_ATTRIBUTES = ["style", "class", "hidden", "inert", "data-state", "data-above-page", "dir"];
const MOTION_EVENTS = ["transitionrun", "transitionend", "transitioncancel", "animationstart", "animationend", "animationcancel"];

/** Each target and its ancestors. */
function chainOf(targets: readonly (Element | null | undefined)[]): Element[] {
  const chain = new Set<Element>();
  for (const target of targets) {
    for (let at = target ?? null; at; at = at.parentElement) chain.add(at);
  }
  return [...chain];
}

/** Observes what `follower` depends on now, when that has changed (a composer that came, say). */
function refresh(follower: Follower): void {
  const chain = chainOf(follower.targets());
  if (chain.length === follower.observed.length && chain.every((each, index) => each === follower.observed[index])) return;
  follower.observer.disconnect();
  for (const each of chain) follower.observer.observe(each);
  follower.observed = chain;
}

/** Whether a running animation is carrying one of the followers' targets along. */
function moving(): boolean {
  const animations = document.getAnimations();
  if (animations.length === 0) return false;
  const carried = new Set<Element>();
  for (const follower of followers) for (const each of follower.observed) carried.add(each);
  return animations.some((animation) => {
    if (animation.playState !== "running" || !(animation.effect instanceof KeyframeEffect)) return false;
    const target = animation.effect.target;
    return target !== null && carried.has(target);
  });
}

function flush(): void {
  frame = 0;
  for (const follower of followers) {
    refresh(follower);
    follower.run();
  }
  if (followers.size > 0 && moving()) schedule();
}

function schedule(): void {
  if (frame === 0) frame = requestAnimationFrame(flush);
}

function listen(): () => void {
  const mutations = new MutationObserver(schedule);
  mutations.observe(document.documentElement, {
    subtree: true,
    childList: true,
    attributes: true,
    attributeFilter: WATCHED_ATTRIBUTES,
  });
  const options = { capture: true, passive: true };
  for (const name of MOTION_EVENTS) document.addEventListener(name, schedule, options);
  document.addEventListener("scroll", schedule, options);
  window.addEventListener("resize", schedule);
  return () => {
    mutations.disconnect();
    for (const name of MOTION_EVENTS) document.removeEventListener(name, schedule, options);
    document.removeEventListener("scroll", schedule, options);
    window.removeEventListener("resize", schedule);
    cancelAnimationFrame(frame);
    frame = 0;
  };
}

/**
 * Calls `run` now and whenever `targets` may have moved, until the returned function is called.
 * `run` looks at where things are and acts only on a change: it can be called with nothing new.
 */
export function followLayout(targets: Follower["targets"], run: () => void): () => void {
  const follower: Follower = {
    targets,
    run,
    observer: new ResizeObserver(() => run()),
    observed: [],
  };
  followers.add(follower);
  stopListening ??= listen();
  refresh(follower);
  run();
  // Something may be under way already, such as the panel this tab is opening in.
  if (moving()) schedule();
  return () => {
    follower.observer.disconnect();
    followers.delete(follower);
    if (followers.size === 0) {
      stopListening?.();
      stopListening = null;
    }
  };
}
