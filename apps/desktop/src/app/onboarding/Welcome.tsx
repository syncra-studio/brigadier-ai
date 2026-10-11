import { type FormEvent, useEffect, useId, useRef, useState } from "react";

import { errorText } from "@/app/dialogs/fields";
import { useFocusOnMount } from "@/app/onboarding/parts";
import { playWelcomeSound, preloadWelcomeSound } from "@/app/onboarding/welcomeSound";
import { BrigadierGlyph } from "@/components/glyphs/brand-glyph";
import { Spinner } from "@/components/glyphs/spinner";
import { Button } from "@/components/ui/button";
import { DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Kbd } from "@/components/ui/kbd";

/** How long the intro waits for its sound to load before it starts without it. */
const SOUND_WAIT_MS = 1500;
/** How long the mark turns, from just after it lands. */
const TURN_MS = 1000;
const TURN_AFTER_LAND_MS = 100;

/** The first launch's heading, a word at a time (styles/onboarding.css staggers them). */
const HEADING = ["Welcome ", "to ", "Brigadier"];

/** A time token from tokens.css ("2.2s", "450ms") in milliseconds. */
function tokenMs(name: string): number {
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  const amount = Number.parseFloat(value);
  return value.endsWith("ms") ? amount : amount * 1000;
}

function reducedMotion(): boolean {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * The first launch's welcome, then the name. The intro starts with its sound: the mark's
 * strokes gather and its core lands as the sound blooms, the mark turns once, rises above
 * "Welcome to Brigadier", and Continue appears last; only then can it be used (Enter too, as
 * it takes the focus). Escape shows the end at once, as reduced motion does. The name step
 * keeps the mark where it came to rest and asks what to call the user.
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
  /** The sound started: stops it (fading) if the setup closes before it ends. */
  onSound: (stop: () => void) => void;
  onContinue: () => void;
  onName: (name: string) => Promise<void>;
}) {
  const [started, setStarted] = useState(false);
  const [assembling, setAssembling] = useState(true);
  const [spinning, setSpinning] = useState(false);
  const [settled, setSettled] = useState(() => step !== "intro" || reducedMotion());
  const [ready, setReady] = useState(settled);
  const onSoundRef = useRef(onSound);
  useEffect(() => {
    onSoundRef.current = onSound;
  }, [onSound]);

  // The intro starts when its sound does (or without it, if it can't play).
  useEffect(() => {
    if (step !== "intro") return;
    let live = true;
    const begin = () => {
      if (live) setStarted(true);
    };
    const waited = new Promise<"late">((resolve) => setTimeout(() => resolve("late"), SOUND_WAIT_MS));
    void Promise.race([preloadWelcomeSound(), waited])
      .then((loaded) => {
        if (!live) return;
        if (loaded === "late") {
          console.error("the welcome sound took too long to load; the intro starts without it");
          begin();
          return;
        }
        return playWelcomeSound().then((stop) => {
          if (!live) {
            stop();
            return;
          }
          onSoundRef.current(stop);
          begin();
        });
      })
      .catch((error: unknown) => {
        console.error("the welcome sound can't play", error);
        begin();
      });
    return () => {
      live = false;
    };
    // The intro plays once per mount; a later Escape doesn't restart it.
  }, [step]);

  // Once the core has landed, the strokes stop being animated and the mark turns once.
  useEffect(() => {
    if (!started || settled) return;
    const land = tokenMs("--intro-land") + TURN_AFTER_LAND_MS;
    const timers = [
      setTimeout(() => {
        setAssembling(false);
        setSpinning(true);
      }, land),
      setTimeout(() => setSpinning(false), land + TURN_MS),
    ];
    return () => timers.forEach(clearTimeout);
  }, [started, settled]);

  // Escape during the intro shows its end.
  useEffect(() => {
    if (step !== "intro" || settled) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopImmediatePropagation();
      setSettled(true);
      setSpinning(false);
      setReady(true);
    };
    window.addEventListener("keydown", onKeyDown, { capture: true });
    return () => window.removeEventListener("keydown", onKeyDown, { capture: true });
  }, [step, settled]);

  return (
    <div
      className="welcome"
      data-step={step}
      data-started={started || undefined}
      data-assembling={(started && assembling && !settled) || undefined}
      data-settled={settled || undefined}
      data-tauri-drag-region
    >
      <div className="welcome-glow" aria-hidden />
      <div className="welcome-mark" aria-hidden>
        <BrigadierGlyph spinning={spinning} />
      </div>
      {step === "intro" ? (
        <>
          <div className="welcome-copy">
            <DialogTitle asChild>
              <h1 className="welcome-heading" aria-label={HEADING.join("")}>
                {HEADING.map((word) => (
                  <span key={word} aria-hidden>
                    {word}
                  </span>
                ))}
              </h1>
            </DialogTitle>
            <p className="welcome-line text-muted-foreground mt-3 text-lg">Your next idea starts here.</p>
          </div>
          <div
            className="welcome-continue"
            onAnimationEnd={(event) => {
              if (event.target === event.currentTarget && event.animationName === "intro-continue-in") {
                setReady(true);
              }
            }}
          >
            <ContinueButton ready={ready} onClick={onContinue} />
          </div>
        </>
      ) : (
        <NameForm name={name} onName={onName} />
      )}
    </div>
  );
}

function ContinueButton({
  ready,
  busy = false,
  onClick,
  form,
}: {
  ready: boolean;
  busy?: boolean;
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
      disabled={!ready || busy}
      onClick={onClick}
      className="min-w-32 gap-2"
    >
      {busy && <Spinner className="animate-spin" />}
      Continue
      <Kbd>⏎</Kbd>
    </Button>
  );
}

/** "What should we call you?": one field, Continue (or Enter) saves it. */
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
    <>
      <form id={id} className="welcome-copy" onSubmit={(event) => void submit(event)}>
        <DialogTitle asChild>
          <h1 className="welcome-heading">What should we call you?</h1>
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
          className="h-control-lg mt-8 max-w-xs text-center text-lg"
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
    </>
  );
}
