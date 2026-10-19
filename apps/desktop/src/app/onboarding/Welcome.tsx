import { type FormEvent, useCallback, useEffect, useId, useRef, useState } from "react";
import { ArrowRight } from "@openai/apps-sdk-ui/components/Icon";

import { errorText } from "@/app/dialogs/fields";
import { IntroCanvas, WINDOW_SHAPED } from "@/app/onboarding/IntroCanvas";
import { useFocusOnMount } from "@/app/onboarding/parts";
import { StaggerText } from "@/app/onboarding/StaggerText";
import { playWelcomeSound, preloadWelcomeSound } from "@/app/onboarding/welcomeSound";
import { BrigadierGlyph } from "@/components/glyphs/brand-glyph";
import { Spinner } from "@/components/glyphs/spinner";
import { Button } from "@/components/ui/button";
import { DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Kbd } from "@/components/ui/kbd";
import { introBegin, introFinish, introRelease, introShaped, onIntroEnded } from "@/ipc/client";

/** How long the intro waits for its sound to load before it starts without it. */
const SOUND_WAIT_MS = 1500;
/** How long the window's page takes to show clear before the window does. */
const STAGE_PAINT_MS = 50;
/** The desktop's dim fading out, early (Escape, the window ending it). */
const QUICK_LIFT_MS = 300;

/**
 * The welcome's title, a line at a time: the first shorter than the second. Others to try
 * (swap one in):
 *   ["Meet the", "brigadier, march!"]
 *   ["Let Brigadier", "command your fleet."]
 */
const TITLE = ["Brigadier,", "at your command."] as const;

/** A time token from tokens.css ("2.2s", "450ms") in milliseconds. */
function tokenMs(name: string): number {
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  const amount = Number.parseFloat(value);
  return value.endsWith("ms") ? amount : amount * 1000;
}

function reducedMotion(): boolean {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/** The page shows clear while the window is staged, so only the orb is seen (onboarding.css). */
function setPageStaged(staged: boolean): void {
  if (staged) document.documentElement.dataset.intro = "staged";
  else delete document.documentElement.dataset.intro;
}

/**
 * The first launch's welcome, then the name. The intro starts with its music, a moment before
 * anything is seen. On macOS's first launch the desktop dims and a soft orb of light comes into
 * focus at the window's centre, idling there, slowly changing shape, while the music builds.
 * It opens out, blurred, into the window, and its light settles onto the shell's dark, with
 * faint waves of light moving over it; the window's chrome comes back. Brigadier's mark comes
 * up at the centre, glowing, read just as the music drops, held while the desktop's dim lifts,
 * then fades away; the music's motion stops with it and its chord rings on, breathing. The
 * title comes in a letter at a time; the arrow button under it appears last, with a soft note,
 * and only then can it be used (Enter too, as it takes the focus). The sound settles and fades
 * out a few seconds later, into the name if need be. Elsewhere it plays the same inside the
 * window, without the dim. Escape shows the end at once, as reduced motion does. Continuing fades
 * the title and the waves out, and the name step comes in on the same dark, with the mark above
 * the question.
 */
export function Welcome({
  step,
  name,
  onSound,
  onContinue,
  onName,
}: {
  step: "intro" | "name";
  /** The name already given, if any. */
  name: string;
  /** The sound started: stops it (fading) if the setup or the window closes before it ends. */
  onSound: (stop: () => void) => void;
  onContinue: () => void;
  onName: (name: string) => Promise<void>;
}) {
  const [start, setStart] = useState<number | null>(null);
  const [settled, setSettled] = useState(() => step !== "intro" || reducedMotion());
  const [ready, setReady] = useState(settled);
  // The gradient stays until it has faded out behind the name.
  const [canvasShown, setCanvasShown] = useState(step === "intro");
  // The window is staged for the intro (intro.rs) until its dim starts lifting.
  const staged = useRef(false);
  // Continuing: the title and the waves fade out before the name comes in.
  const [leaving, setLeaving] = useState(false);
  const leaveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const onContinueRef = useRef(onContinue);
  useEffect(() => {
    onContinueRef.current = onContinue;
  }, [onContinue]);
  const onSoundRef = useRef(onSound);
  useEffect(() => {
    onSoundRef.current = onSound;
  }, [onSound]);

  // Settled before the window was staged (WebGL can't draw, or Escape): it isn't staged.
  const settledEarly = useRef(settled);
  // Shows the intro's end at once; the window's own state comes back with it.
  const settle = useCallback(() => {
    settledEarly.current = true;
    setSettled(true);
    setReady(true);
    setPageStaged(false);
    if (staged.current) {
      staged.current = false;
      void introFinish(QUICK_LIFT_MS).catch((error: unknown) => console.error(error));
    }
  }, []);

  // The intro starts with its sound (or without it, if it can't play), once the window shows.
  useEffect(() => {
    if (step !== "intro") return;
    let live = true;
    const still = reducedMotion();
    const waited = new Promise<"late">((resolve) => setTimeout(() => resolve("late"), SOUND_WAIT_MS));
    void (async () => {
      const loaded = await Promise.race([
        preloadWelcomeSound().then(
          () => "loaded" as const,
          (error: unknown) => {
            console.error("the welcome sound can't load", error);
            return "failed" as const;
          },
        ),
        waited,
      ]);
      if (!live) return;
      if (loaded === "late") console.error("the welcome sound took too long to load; the intro starts without it");
      if (still || settledEarly.current) {
        await introRelease().catch((error: unknown) => console.error(error));
      } else {
        setPageStaged(true);
        await new Promise((resolve) => setTimeout(resolve, STAGE_PAINT_MS));
        const shown = await introBegin().catch((error: unknown) => {
          console.error("the window can't stage the intro", error);
          return false;
        });
        if (!live) {
          setPageStaged(false);
          if (shown) void introFinish(0).catch((error: unknown) => console.error(error));
          return;
        }
        staged.current = shown;
        if (!shown) setPageStaged(false);
      }
      if (loaded === "loaded") {
        try {
          const stop = await playWelcomeSound();
          if (!live) {
            stop();
            return;
          }
          onSoundRef.current(stop);
        } catch (error) {
          console.error("the welcome sound can't play", error);
        }
      }
      if (live) setStart(performance.now());
    })();
    return () => {
      live = false;
    };
    // The intro plays once per mount; a later Escape doesn't restart it.
  }, [step]);

  // The window's beats, from the intro's start.
  useEffect(() => {
    if (start === null || settled) return;
    const at = (ms: number, beat: () => void) => setTimeout(beat, start + ms - performance.now());
    const timers = [
      at(WINDOW_SHAPED * 1000, () => {
        setPageStaged(false);
        if (staged.current) void introShaped().catch((error: unknown) => console.error(error));
      }),
      at(tokenMs("--intro-dim-lift-at"), () => {
        if (!staged.current) return;
        staged.current = false;
        void introFinish(tokenMs("--intro-dim-lift")).catch((error: unknown) => console.error(error));
      }),
    ];
    return () => timers.forEach(clearTimeout);
  }, [start, settled]);

  // Escape during the intro shows its end; so does the window ending the intro itself.
  useEffect(() => {
    if (step !== "intro" || settled) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopImmediatePropagation();
      settle();
    };
    window.addEventListener("keydown", onKeyDown, { capture: true });
    const ended = onIntroEnded(settle);
    return () => {
      window.removeEventListener("keydown", onKeyDown, { capture: true });
      void ended.then((unlisten) => unlisten()).catch(() => {});
    };
  }, [step, settled, settle]);

  const leave = useCallback(() => {
    if (leaveTimer.current) return;
    setLeaving(true);
    leaveTimer.current = setTimeout(() => onContinueRef.current(), reducedMotion() ? 0 : tokenMs("--intro-leave"));
  }, []);

  // Leaving with the window still staged (the setup closed) restores it.
  useEffect(
    () => () => {
      if (leaveTimer.current) clearTimeout(leaveTimer.current);
      setPageStaged(false);
      if (staged.current) void introFinish(0).catch((error: unknown) => console.error(error));
    },
    [],
  );

  const playing = start !== null && !settled;
  return (
    <div
      className="welcome"
      data-step={step}
      data-started={start !== null || undefined}
      data-settled={settled || undefined}
      data-leaving={(step === "intro" && leaving) || undefined}
      data-tauri-drag-region
    >
      {canvasShown && (
        <IntroCanvas
          start={start}
          settled={settled}
          still={reducedMotion()}
          onUnavailable={settle}
          className="welcome-canvas"
          onTransitionEnd={() => {
            if (step !== "intro") setCanvasShown(false);
          }}
        />
      )}
      {step === "intro" && (
        <div className="welcome-intro-mark" aria-hidden>
          <BrigadierGlyph />
        </div>
      )}
      {step === "intro" ? (
        <div className="welcome-copy">
          <DialogTitle asChild>
            <h1 className="welcome-title" aria-label={TITLE.join(" ")}>
              {TITLE.map((line, index) => (
                <span key={line} className="welcome-title-line">
                  <StaggerText
                    text={line}
                    play={playing}
                    // The second line follows on from the first's last letter.
                    delayMs={tokenMs("--intro-heading") + lettersBefore(index) * tokenMs("--intro-letter-stagger")}
                    staggerMs={tokenMs("--intro-letter-stagger")}
                  />
                </span>
              ))}
            </h1>
          </DialogTitle>
          <div
            className="welcome-continue"
            // Out of reach until it has faded in, without the disabled look.
            inert={!ready}
            onAnimationEnd={(event) => {
              if (event.target === event.currentTarget && event.animationName === "intro-continue-in") {
                setReady(true);
              }
            }}
          >
            <ContinueButton ready={ready} arrow onClick={leave} />
          </div>
        </div>
      ) : (
        <NameForm name={name} onName={onName} />
      )}
    </div>
  );
}

/** How many letters (spaces aside) the title's lines before `line` hold. */
function lettersBefore(line: number): number {
  return TITLE.slice(0, line).reduce((sum, text) => sum + text.replaceAll(" ", "").length, 0);
}

function ContinueButton({
  ready,
  busy = false,
  arrow = false,
  onClick,
  form,
}: {
  ready: boolean;
  busy?: boolean;
  /** Just an arrow, not the word or its key (Enter still works). */
  arrow?: boolean;
  onClick?: () => void;
  form?: string;
}) {
  const button = useRef<HTMLButtonElement>(null);
  // Once usable it takes the focus, so Enter continues.
  useEffect(() => {
    if (ready && !form) button.current?.focus();
  }, [ready, form]);
  return (
    <Button
      ref={button}
      size="lg"
      type={form ? "submit" : "button"}
      form={form}
      disabled={arrow ? busy : !ready || busy}
      onClick={onClick}
      aria-label={arrow ? "Continue" : undefined}
      className={arrow ? "px-7" : "min-w-32 gap-2"}
    >
      {busy && <Spinner className="animate-spin" />}
      {arrow ? <ArrowRight /> : "Continue"}
      {!arrow && <Kbd>↩</Kbd>}
    </Button>
  );
}

/**
 * "What should we call you?" under the mark: one borderless field, focused as the step opens;
 * Continue (or Return) saves it once it holds more than spaces.
 */
function NameForm({ name, onName }: { name: string; onName: (name: string) => Promise<void> }) {
  const id = useId();
  const [value, setValue] = useState(name);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const trimmed = value.trim();
  const input = useFocusOnMount<HTMLInputElement>();

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!trimmed || busy) return;
    setBusy(true);
    setError(null);
    try {
      await onName(trimmed);
    } catch (cause) {
      setError(errorText(cause));
      setBusy(false);
    }
  };

  return (
    <div className="welcome-name">
      <form id={id} className="welcome-name-body" onSubmit={(event) => void submit(event)}>
        <div className="welcome-mark" aria-hidden>
          <BrigadierGlyph />
        </div>
        <DialogTitle asChild>
          <h1 className="welcome-heading text-balance">What should we call you?</h1>
        </DialogTitle>
        <label htmlFor={`${id}-name`} className="sr-only">
          Your name
        </label>
        <Input
          ref={input}
          id={`${id}-name`}
          autoComplete="given-name"
          placeholder="Your name"
          maxLength={80}
          value={value}
          disabled={busy}
          aria-invalid={error !== null || undefined}
          onChange={(event) => setValue(event.target.value)}
          // Borderless: the words alone, centred under the question.
          className="placeholder:text-muted-foreground/70 focus-visible:bg-transparent disabled:bg-transparent h-auto max-w-xs rounded-none bg-transparent p-0 text-center text-lg outline-none"
        />
        {error && (
          <p role="alert" className="text-destructive mt-3 text-sm">
            {error}
          </p>
        )}
      </form>
      <div className="welcome-continue">
        <ContinueButton ready={trimmed !== ""} busy={busy} form={id} />
      </div>
    </div>
  );
}
