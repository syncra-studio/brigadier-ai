import { useRef, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { useListedModels } from "@/app/settings/routing/pickers";
import { ruleNames, ruleText, ruleTitle, ruleWarnings } from "@/app/settings/routing/ruleWords";
import { rowModelName, workRows } from "@/app/settings/routing/words";
import { SettingsSection } from "@/app/settings/parts";
import { type ModelGroup, SettingsCard } from "@/components/shared";
import { Button } from "@/components/controls";
import { Plus, Spinner, Warning, X } from "@/components/icons";
import type { ModelRule, ModelRules } from "@/ipc/generated";
import { readModelRules, saveModelRules, useModelRules } from "@/state/modelRules";
import { toast } from "@/state/toasts";

export const RULES = {
  label: "Rules",
  description: "Things every session must follow, in your own words.",
} as const;

/** Rules to click into the box, to start from. */
export const RULE_EXAMPLES = ["Never go above High effort", "Never use Sonnet", "Only use Codex for reviews"] as const;

/**
 * Rules: each one plain sentence with a × to remove it; one kept as advice says so in grey; a
 * rule that stops a kind of work from starting has one warning line. Add rule opens one text
 * box with examples to fill it; Add has a model rewrite the words into exact rules (a spinner
 * while it does), shows what Brigadier will save, and Add again saves it.
 */
export function Rules({ rules: file, groups }: { rules: ModelRules | null; groups: readonly ModelGroup[] }) {
  const rules = file?.rules ?? [];
  const [adding, setAdding] = useState(false);
  const remove = useAction();
  const models = useListedModels();
  const names = ruleNames(groups);
  const save = (next: readonly ModelRule[]) => (file ? saveModelRules(file, file.rows, next) : Promise.resolve());
  const warnings = ruleWarnings(rules, workRows(file), models, names, (row) => rowModelName(groups, row.agent, row.model));
  return (
    <SettingsSection
      title={RULES.label}
      description={RULES.description}
      actions={
        !adding && (
          <Button variant="soft" size="sm" data-slot="add-rule" disabled={!file} onClick={() => setAdding(true)}>
            <Plus aria-hidden />
            Add rule
          </Button>
        )
      }
    >
      {file && (rules.length > 0 || adding) && (
        <SettingsCard inset data-slot="routing-rules">
          {adding && (
            <AddBox
              file={file}
              names={names}
              onClose={() => setAdding(false)}
              onSaved={(before) => {
                setAdding(false);
                toast("Rules added", { actions: [{ label: "Undo", run: () => void undoAdd(before) }] });
              }}
            />
          )}
          {rules.map((rule) => (
            <RuleRow
              key={ruleText(rule)}
              rule={rule}
              title={ruleTitle(rule, names)}
              warning={warnings.get(ruleText(rule)) ?? null}
              busy={remove.busy}
              onRemove={() => remove.run(() => save(rules.filter((other) => other !== rule)))}
            />
          ))}
        </SettingsCard>
      )}
      {remove.error && (
        <p role="alert" className="text-destructive text-xs">
          {remove.error}
        </p>
      )}
    </SettingsSection>
  );
}

/** Undo outlives the render that added the rules, so it saves over the file as it is now. */
function undoAdd(before: readonly ModelRule[]): Promise<void> {
  const view = useModelRules.getState().view;
  if (!view) return Promise.resolve();
  return saveModelRules(view, view.rows, before).catch(() => {
    toast("Couldn't undo: your rules changed.");
  });
}

/** One rule: its sentence, "advice" in grey if it is only advice, a × to remove it; a warning line when it stops something. */
function RuleRow({
  rule,
  title,
  warning,
  busy,
  onRemove,
}: {
  rule: ModelRule;
  title: string;
  warning: string | null;
  busy: boolean;
  onRemove: () => void;
}) {
  return (
    <div data-slot="routing-rule" data-setting={title} className="flex flex-col gap-1.5 py-3.5">
      <div className="flex items-center gap-3">
        <span className="text-label text-foreground min-w-0 flex-1 truncate">
          {title}
          {rule.type === "advice" && <span className="text-muted-foreground ms-2 text-xs">advice</span>}
        </span>
        <button
          type="button"
          aria-label={`Remove “${title}”`}
          disabled={busy}
          onClick={onRemove}
          className="text-muted-foreground hover:text-foreground shrink-0 disabled:opacity-50"
        >
          <X aria-hidden className="size-3.75" />
        </button>
      </div>
      {warning && (
        <span data-slot="routing-rule-warning" className="text-warning flex items-center gap-1.5 text-xs">
          <Warning aria-hidden className="size-3.25 shrink-0" />
          {warning}
        </span>
      )}
    </div>
  );
}

/** The box's moment: writing, Brigadier rewriting the words, or its rules shown before saving. */
type Phase = { at: "writing" } | { at: "reading" } | { at: "read"; rules: ModelRule[] };

/** Add rule: a text box, examples that fill it, and (once read) what Brigadier will save. */
function AddBox({
  file,
  names,
  onClose,
  onSaved,
}: {
  file: ModelRules;
  names: ReturnType<typeof ruleNames>;
  onClose: () => void;
  onSaved: (before: readonly ModelRule[]) => void;
}) {
  const [text, setText] = useState("");
  const [phase, setPhase] = useState<Phase>({ at: "writing" });
  const [error, setError] = useState<string | null>(null);
  // Each read's number: a reply to a read that was cancelled or replaced is dropped.
  const reads = useRef(0);
  const add = useAction();
  const reading = phase.at === "reading";
  const read = async () => {
    const id = ++reads.current;
    setError(null);
    setPhase({ at: "reading" });
    try {
      const rules = await readModelRules(text);
      if (reads.current === id) setPhase({ at: "read", rules });
    } catch (cause) {
      if (reads.current !== id) return;
      setPhase({ at: "writing" });
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  };
  const write = (next: string) => {
    setText(next);
    setError(null);
    if (phase.at === "read") setPhase({ at: "writing" });
  };
  const cancel = () => {
    reads.current++;
    // While Brigadier rewrites, Cancel stops waiting and keeps the words; else it closes the box.
    if (reading) setPhase({ at: "writing" });
    else onClose();
  };
  const submit = () => {
    if (phase.at !== "read") return void read();
    const known = new Set(file.rules.map(ruleText));
    const next = [...file.rules, ...phase.rules.filter((rule) => !known.has(ruleText(rule)))];
    add.run(async () => {
      await saveModelRules(file, file.rows, next);
      onSaved(file.rules);
    });
  };
  const saved = phase.at === "read" ? phase.rules : [];
  return (
    <div data-slot="routing-rule-form" className="flex flex-col gap-3 py-4">
      <textarea
        aria-label="A rule, in your own words"
        placeholder="e.g. Never go above High effort"
        value={text}
        onChange={(event) => write(event.target.value)}
        disabled={reading}
        rows={2}
        spellCheck={false}
        autoFocus
        className="border-input bg-popover rounded-control text-label placeholder:text-muted-foreground min-h-16 w-full resize-none border px-3 py-2 outline-none disabled:opacity-60"
      />
      {reading ? (
        <div role="status" className="bg-foreground/4 rounded-control text-foreground/65 text-label flex items-center gap-2 px-3 py-2.5">
          <Spinner aria-hidden className="size-3.5 shrink-0 animate-spin" />
          Rewriting your rules…
        </div>
      ) : phase.at === "read" ? (
        <div data-slot="routing-rule-read" className="bg-foreground/4 rounded-control flex flex-col gap-1.5 px-3 py-2.5">
          {saved
            .filter((rule) => rule.type !== "advice")
            .map((rule) => (
              <span key={ruleText(rule)} className="text-label">
                <span className="text-foreground/65">Brigadier will: </span>
                <span className="text-foreground">{ruleTitle(rule, names)}</span>
              </span>
            ))}
          {saved
            .filter((rule) => rule.type === "advice")
            .map((rule) => (
              <span key={ruleText(rule)} className="text-label">
                <span className="text-foreground/65">Saved as advice: </span>
                <span className="text-foreground">{ruleTitle(rule, names)}</span>
              </span>
            ))}
        </div>
      ) : (
        <>
          <p className="text-foreground/65 text-xs">Brigadier rewrites your words into exact rules and shows them before saving.</p>
          <div className="flex flex-wrap items-center gap-2">
            {RULE_EXAMPLES.map((example) => (
              <button
                key={example}
                type="button"
                onClick={() => write(text.trim() ? `${text.trimEnd()}\n${example}` : example)}
                className="border-divider bg-foreground/3 hover:bg-foreground/6 rounded-capsule text-foreground/80 h-7 border px-3 text-xs"
              >
                {example}
              </button>
            ))}
          </div>
        </>
      )}
      {(error ?? add.error) && (
        <p role="alert" className="text-destructive text-xs">
          {error ?? add.error}
        </p>
      )}
      <div className="flex items-center justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={cancel}>
          Cancel
        </Button>
        <Button size="sm" disabled={!text.trim() || reading || add.busy} onClick={submit}>
          {reading ? "Reading…" : "Add"}
        </Button>
      </div>
    </div>
  );
}
