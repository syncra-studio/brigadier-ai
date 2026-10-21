import {
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
  useEffect,
  useEffectEvent,
  useId,
  useRef,
  useState,
} from "react";

import { CardDictate } from "@/app/conversation/CardDictate";
import { inField, useCardKeys } from "@/app/conversation/cardKeys";
import { ActionCard, ActionCardQuestion, ActionFreeText, ActionOption, actionButton, staggered } from "@/components/conversation";
import type { AnswerInput } from "@/ipc/generated";
import { cancelDictation, dictationOwner, registerInserter } from "@/state/dictation";

/** How long a picked answer shows its dot before it goes. */
const COMMIT_MS = 180;

/** A button's click as the user gave it: by the pointer, or by Enter or Space on it. */
function clickInput(event: MouseEvent): AnswerInput {
  return event.detail === 0 ? "enter" : "click";
}

/**
 * The card shared by questions and "Implement this plan?": numbered answers (1–9 light, ↑/↓
 * move, Enter picks the lit one; a pick shows its dot, then goes), then the free-text row
 * with Skip, which turns into Submit once something is typed (Next, before a round's last
 * question; ←/→ page through the round). Esc puts it aside; ×, with `onClose`, does that.
 *
 * Enter picks the lit answer only from the focused card, never from elsewhere in the window: a
 * click in the thread and Enter answers nothing. Every answer may change code or be hard to undo
 * (a merge, building a plan), so nothing is lit until the user moves to an answer, even when the
 * card took focus on its own: ↑/↓ or 1–9 light one (the pointer passing over lights nothing),
 * and the recommended answer is only marked. A click on an answer picks it.
 */
export function ChoiceCard({
  name,
  title,
  kind,
  detail,
  extra,
  choices,
  kept,
  initialText = "",
  placeholder,
  pager,
  last = true,
  onPage,
  onChoose,
  onText,
  onTyped,
  onLit,
  onSkip,
  skipLabel = "Skip",
  onDismiss,
  onClose,
  busy,
  error,
  footer,
  stagger,
}: {
  name: "question" | "plan";
  title: ReactNode;
  /** A few words over the question (a round's title). */
  kind?: ReactNode;
  detail?: ReactNode;
  /** Between the question and the answers (the files asked about). */
  extra?: ReactNode;
  choices: { label: string; description?: ReactNode; recommended?: boolean }[];
  /** The answer lit when the card was last shown, before the user went elsewhere. */
  kept?: number | null | undefined;
  /** What the free-text row holds at first (an answer typed before paging back). */
  initialText?: string;
  placeholder: string;
  /** A round's "‹ 1 of 3 ›", beside the dismiss button. */
  pager?: ReactNode;
  /** The round's last question: its button submits the round. */
  last?: boolean;
  /** ←/→ to the round's previous or next question. */
  onPage?: (step: -1 | 1) => void;
  /** An answer picked, and how: a click or Enter. */
  onChoose: (index: number, input: AnswerInput) => void;
  /** Words typed in the free-text row. */
  onText: (text: string) => void;
  /** What the free-text row holds as it is typed, so paging away and back keeps it. */
  onTyped?: (text: string) => void;
  /** The answer lit as it changes, so a visit elsewhere keeps it. */
  onLit?: (index: number | null) => void;
  onSkip: (input: AnswerInput) => void;
  /** What the button reads while nothing is typed. */
  skipLabel?: string;
  onDismiss: () => void;
  /** What × does instead of putting the card aside, and its label. */
  onClose?: { label: string; run: () => void };
  busy: boolean;
  error: string | null;
  footer: ReactNode;
  /** Fade the rows in one after another. */
  stagger: boolean;
}) {
  // The lit answer; -1 is the free-text row, null none yet.
  const [highlight, setHighlight] = useState<number | null>(
    choices.length === 0 ? -1 : (kept ?? null),
  );
  const lit = useEffectEvent((index: number | null) => onLit?.(index));
  useEffect(() => lit(highlight), [highlight]);
  const [chosen, setChosen] = useState<number | null>(null);
  const [typed, setTyped] = useState(initialText);
  // Dictated words go after what is typed, into this card alone: each mounted card (one per
  // question of a round, in each chat open) owns its own dictation.
  const owner = `card:${name}:${useId()}`;
  const dictated = useEffectEvent((words: string) => {
    const next = typed.trim() ? `${typed.trimEnd()} ${words}` : words;
    setTyped(next);
    onTyped?.(next);
    setHighlight(-1);
  });
  useEffect(() => {
    const unregister = registerInserter(owner, async (words) => dictated(words));
    return () => {
      unregister();
      // Paged away or answered while dictating: the words go nowhere rather than into another card.
      if (dictationOwner() === owner) cancelDictation();
    };
  }, [owner]);
  const card = useRef<HTMLElement>(null);
  const field = useRef<HTMLInputElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  // The card takes focus (the field, when there is nothing to pick), unless the user is
  // typing somewhere.
  const pickable = choices.length > 0;
  useEffect(() => {
    if (!inField(document.activeElement))
      (pickable ? card : field).current?.focus();
  }, [pickable]);
  useEffect(() => () => clearTimeout(timer.current), []);

  const idle = !busy && chosen === null;
  const commit = (index: number, input: AnswerInput) => {
    if (!idle || choices[index] === undefined) return;
    setChosen(index);
    setHighlight(index);
    timer.current = setTimeout(() => {
      setChosen(null);
      onChoose(index, input);
    }, COMMIT_MS);
  };
  const text = typed.trim();
  const send = (input: AnswerInput) => {
    if (!idle) return;
    if (text) onText(text);
    else onSkip(input);
  };
  useCardKeys(
    idle,
    {
      enter: () => {
        if (highlight !== null && highlight >= 0) commit(highlight, "enter");
      },
      escape: onDismiss,
    },
    card,
  );
  const onKeyDown = (event: KeyboardEvent) => {
    if (
      !idle ||
      inField(event.target) ||
      event.metaKey ||
      event.ctrlKey ||
      event.altKey
    )
      return;
    const digit = Number(event.key);
    const lastChoice = choices.length - 1;
    if (
      Number.isInteger(digit) &&
      digit >= 1 &&
      digit <= Math.min(9, choices.length)
    ) {
      event.preventDefault();
      setHighlight(digit - 1);
    } else if (digit === choices.length + 1 && digit <= 9) {
      event.preventDefault();
      field.current?.focus();
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      if (highlight === null) setHighlight(0);
      else if (highlight >= lastChoice) field.current?.focus();
      else setHighlight(highlight + 1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      if (highlight === null || highlight === -1) setHighlight(lastChoice);
      else if (highlight > 0) setHighlight(highlight - 1);
    } else if (onPage && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
      event.preventDefault();
      onPage(event.key === "ArrowLeft" ? -1 : 1);
    }
  };

  const row = (index: number) => staggered(stagger, index);
  return (
    <ActionCard
      ref={card}
      container="request"
      tabIndex={0}
      aria-label={name === "plan" ? "Implement this plan?" : "Question"}
      data-action={name}
      onKeyDown={onKeyDown}
    >
      <div {...row(0)}>
        <ActionCardQuestion
          onDismiss={() => {
            if (!onClose) onDismiss();
            else if (idle) onClose.run();
          }}
          dismissLabel={onClose?.label}
          kind={kind}
          detail={detail}
          aside={pager}
        >
          {title}
        </ActionCardQuestion>
      </div>
      <div className="flex flex-col gap-3 pt-1 pb-2">
        {extra && <div {...row(1)}>{extra}</div>}
        <div className="flex flex-col gap-1 px-2">
          {choices.length > 0 && (
            <div
              role="radiogroup"
              aria-label="Answers"
              className="flex flex-col gap-1"
            >
              {choices.map((choice, index) => (
                <ActionOption
                  key={choice.label}
                  role="radio"
                  aria-checked={index === highlight}
                  // "Yes, implement this plan" alone: an empty square.
                  number={name === "plan" ? null : index + 1}
                  label={choice.label}
                  description={choice.description ?? undefined}
                  recommended={choice.recommended}
                  highlighted={index === highlight}
                  chosen={index === chosen}
                  disabled={busy}
                  onClick={(event) => commit(index, clickInput(event))}
                  {...row(index + 2)}
                />
              ))}
            </div>
          )}
          <ActionFreeText
            ref={field}
            value={typed}
            aria-label={name === "plan" ? "What should change" : "Your answer"}
            placeholder={placeholder}
            marker={choices.length > 0}
            highlighted={choices.length > 0 && highlight === -1}
            disabled={busy}
            onFocus={() => setHighlight(-1)}
            onChange={(event) => {
              setTyped(event.target.value);
              onTyped?.(event.target.value);
              if (event.target.value) setHighlight(-1);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                // Never the composer's form around the card.
                event.preventDefault();
                if (text) send("enter");
              } else if (event.key === "ArrowUp" && choices.length > 0) {
                event.preventDefault();
                setHighlight(choices.length - 1);
                card.current?.focus();
              } else if (event.key === "Escape") {
                // Back to the answers; a second Esc puts the card aside.
                event.preventDefault();
                card.current?.focus();
              }
            }}
            {...row(choices.length + 2)}
          >
            <CardDictate owner={owner} disabled={busy} />
            <button
              type="button"
              className={actionButton(
                text ? "primary" : "outline",
                "font-medium",
              )}
              disabled={busy}
              onClick={(event) => send(clickInput(event))}
            >
              {!text ? skipLabel : last ? "Submit" : "Next"}
            </button>
          </ActionFreeText>
        </div>
      </div>
      {error && (
        <p role="alert" className="text-destructive px-4 pb-2 text-xs">
          {error}
        </p>
      )}
      {footer}
    </ActionCard>
  );
}
