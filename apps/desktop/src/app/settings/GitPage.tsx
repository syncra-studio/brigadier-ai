import { useEffect, useId, useRef, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import {
  SettingsButton,
  SettingsCard,
  SettingsPage,
  SettingsRow,
  SettingsSection,
  SettingsSelect,
  selectTrigger,
  SwitchSetting,
} from "@/app/settings/parts";
import { findModel, ModelSelector } from "@/components/assistant-ui/elements/model-selector";
import { Check } from "@/components/ui/icon";
import { Input } from "@/components/ui/input";
import type { Choice } from "@/components/ui/segmented";
import type { GitStyle, ModelChoice } from "@/ipc/generated";
import {
  AUTOSAVE_MS,
  autosave,
  begin,
  edited,
  external,
  failed,
  landed,
  quiet,
  SAVED_MS,
  shown,
  status,
  type Autosave,
} from "@/lib/autosave";
import { branchPrefixProblem } from "@/lib/refname";
import { builtInDefault, useAvailableModelGroups, useModelGroups, withChoice } from "@/lib/setup";
import { cn } from "@/lib/utils";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

const WRITER_LABEL = "Write with";
const COMMIT_LABEL = "Commit messages";
const PR_LABEL = "Pull requests";

/** The Git page's rows, for the page and for Settings search. */
export const GIT_ROWS = {
  writer: {
    label: WRITER_LABEL,
    description:
      "The model that writes a commit message when you commit from the Source panel without one",
  },
  commitStyle: {
    label: COMMIT_LABEL,
    description:
      "How commit messages are written: the ones Brigadier writes for you and the agents’ own commits",
  },
  commitInstructions: {
    label: "Commit instructions",
    description: "Your own guidance for commit messages, with Custom instructions",
    target: COMMIT_LABEL,
  },
  prStyle: {
    label: PR_LABEL,
    description:
      "How an agent writes a pull request’s title and description when you ask it to open one",
  },
  prInstructions: {
    label: "Pull request instructions",
    description: "Your own guidance for pull requests, with Custom instructions",
    target: PR_LABEL,
  },
  branchPrefix: {
    label: "Branch prefix",
    description: "Starts the names of new branches Brigadier makes or suggests; a project’s own prefix wins",
  },
  omitAiCoauthors: {
    label: "Leave out AI co-author lines",
    description:
      "Commits from the Source panel and the agents' commits leave out Co-authored-by lines that name an AI, not people's",
  },
  autoMerge: {
    label: "Merge on its own when the work is ready",
    description:
      "The agent checks its work, opens a pull request when the project is on GitHub, and merges it. Off, it tells you the work is ready and you merge",
  },
} as const;

const AUTOMATIC_HINT =
  "A fast, light model Brigadier picks for each message. When an agent is out of quota, another one writes.";

const STYLE_CHOICES = (what: string): Choice<GitStyle>[] => [
  {
    value: "repository",
    label: "Follow this repository",
    hint: `Written like the repository’s recent ${what}.`,
  },
  {
    value: "conventional",
    label: "Conventional Commits",
    hint: "A type and a short summary, like “fix: keep the session after a restart”.",
  },
  { value: "custom", label: "Custom instructions", hint: "Your own guidance, written here." },
];

/**
 * How Brigadier writes and names things in git: the model for the messages it writes, the
 * style of commit messages and pull requests (for it and its agents), the prefix of the
 * branches it makes, and co-author lines.
 */
export function GitPage() {
  const commitStyle = useApp((s) => s.settings.commitStyle);
  const prStyle = useApp((s) => s.settings.prStyle);
  return (
    <SettingsPage
      title="Git"
      description="How Brigadier and its agents write commit messages and pull requests, merge the work, and name new branches."
    >
      <SettingsSection title="Writing">
        <SettingsCard>
          <WriterRow />
        </SettingsCard>
        <SettingsCard>
          <StyleRow
            setting="commitStyle"
            row={GIT_ROWS.commitStyle}
            value={commitStyle}
            choices={STYLE_CHOICES("commits")}
          />
          {commitStyle === "custom" && (
            <InstructionsBox
              setting="commitInstructions"
              label={GIT_ROWS.commitInstructions.label}
              placeholder="Add commit message guidance…"
            />
          )}
          <SwitchSetting setting="omitAiCoauthors" row={GIT_ROWS.omitAiCoauthors} />
        </SettingsCard>
        <SettingsCard>
          <StyleRow
            setting="prStyle"
            row={GIT_ROWS.prStyle}
            value={prStyle}
            choices={STYLE_CHOICES("pull requests")}
          />
          {prStyle === "custom" && (
            <InstructionsBox
              setting="prInstructions"
              label={GIT_ROWS.prInstructions.label}
              placeholder="Add pull request guidance…"
            />
          )}
        </SettingsCard>
      </SettingsSection>

      <SettingsSection title="Merging">
        <SettingsCard>
          <SwitchSetting setting="autoMerge" row={GIT_ROWS.autoMerge} />
        </SettingsCard>
      </SettingsSection>

      <SettingsSection title="Branches">
        <SettingsCard>
          <BranchPrefixRow />
        </SettingsCard>
      </SettingsSection>
    </SettingsPage>
  );
}

/** "Write with": Automatic (null) or a picked agent, model and effort. */
function WriterRow() {
  const groups = useAvailableModelGroups();
  const all = useModelGroups();
  const value = useApp((s) => s.settings.gitWriter);
  const save = useAction();
  const change = (choice: ModelChoice | null) => save.run(() => setSetting("gitWriter", choice));
  // A saved model made unavailable on Providers stays saved; Automatic writes meanwhile.
  const gone = value !== null && findModel(all, value) !== null && findModel(groups, value) === null;
  return (
    <SettingsRow
      label={GIT_ROWS.writer.label}
      // Automatic's own line is in the menu, beside it.
      description={gone ? "Not available now, so Automatic writes them" : GIT_ROWS.writer.description}
      error={save.error}
    >
      <ModelSelector
        label={GIT_ROWS.writer.label}
        groups={value ? withChoice(groups, all, value) : groups}
        value={value ?? builtInDefault(groups)}
        defaultChoice={builtInDefault(groups)}
        onChange={change}
        automatic={{ chosen: value === null, hint: AUTOMATIC_HINT, onChoose: () => change(null) }}
        className={cn(selectTrigger, "text-foreground")}
      />
    </SettingsRow>
  );
}

function StyleRow({
  setting,
  row,
  value,
  choices,
}: {
  setting: "commitStyle" | "prStyle";
  row: { label: string; description: string };
  value: GitStyle;
  choices: Choice<GitStyle>[];
}) {
  const save = useAction();
  return (
    <SettingsRow label={row.label} description={row.description} error={save.error}>
      <SettingsSelect<GitStyle>
        label={row.label}
        value={value}
        options={choices}
        onChange={(style) => save.run(() => setSetting(setting, style))}
      />
    </SettingsRow>
  );
}

/**
 * The text of Custom instructions, saved by itself once typing pauses: Save while there's an
 * edit (⌘S too), "Saved" for a moment after.
 */
function InstructionsBox({
  setting,
  label,
  placeholder,
}: {
  setting: "commitInstructions" | "prInstructions";
  label: string;
  placeholder: string;
}) {
  const saved = useApp((s) => s.settings[setting]);
  const { state, edit, saveNow, error } = useAutosave(saved, (text) => setSetting(setting, text));
  const now = status(state);
  return (
    <div data-setting={label} className="flex flex-col gap-2 px-4 pt-1 pb-3">
      <textarea
        aria-label={label}
        value={shown(state)}
        placeholder={placeholder}
        rows={6}
        spellCheck
        onChange={(event) => edit(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "s" && (event.metaKey || event.ctrlKey)) {
            event.preventDefault();
            saveNow();
          }
          // Esc here stays in the box rather than leaving Settings.
          if (event.key === "Escape") event.stopPropagation();
        }}
        className="border-input bg-popover rounded-control text-label placeholder:text-muted-foreground focus-visible:ring-ring/50 min-h-28 w-full resize-y border px-3 py-2 outline-none focus-visible:ring-2"
      />
      <div className="flex min-h-7 items-center justify-end gap-2">
        {error && (
          <p role="alert" className="text-destructive me-auto text-xs">
            {error}
          </p>
        )}
        {(now === "dirty" || now === "saving") && (
          <SettingsButton disabled={now === "saving"} onClick={saveNow}>
            Save
          </SettingsButton>
        )}
        {now === "saved" && (
          <span aria-live="polite" className="text-muted-foreground flex items-center gap-1.5 text-xs">
            <Check aria-hidden className="size-icon-xs" />
            Saved
          </span>
        )}
      </div>
    </div>
  );
}

/** Runs an [`Autosave`]: saves an edit `AUTOSAVE_MS` after typing stops, and when leaving. */
function useAutosave(saved: string, save: (text: string) => Promise<unknown>) {
  const [state, setState] = useState<Autosave>(() => autosave(saved));
  const [error, setError] = useState<string | null>(null);
  // Saved elsewhere (another window): shown when there's no edit here.
  const [lastSaved, setLastSaved] = useState(saved);
  if (saved !== lastSaved) {
    setLastSaved(saved);
    setState((s) => external(s, saved));
  }
  const current = useRef(state);
  const saveRef = useRef(save);
  useEffect(() => {
    current.current = state;
    saveRef.current = save;
  });

  // The save on its way, if any: an edit left behind when the box goes is saved after it.
  const inFlight = useRef<Promise<unknown> | null>(null);
  const saveNow = () => {
    const [next, value] = begin(current.current);
    if (value === null) return;
    current.current = next;
    setState(next);
    setError(null);
    const saving = saveRef.current(value);
    inFlight.current = saving;
    saving
      .then(
        () => setState((s) => landed(s)),
        () => {
          setError("Couldn't save your instructions. They'll be tried again.");
          setState((s) => failed(s));
        },
      )
      .finally(() => {
        if (inFlight.current === saving) inFlight.current = null;
      });
  };
  const saveLater = useRef(saveNow);
  useEffect(() => {
    saveLater.current = saveNow;
  });

  // An edit saves itself once typing pauses (again after a failure or a save that landed
  // while typing went on).
  useEffect(() => {
    if (state.draft === null || state.saving !== null) return;
    const timer = window.setTimeout(() => saveLater.current(), AUTOSAVE_MS);
    return () => window.clearTimeout(timer);
  }, [state.draft, state.saving]);
  useEffect(() => {
    if (!state.justSaved) return;
    const timer = window.setTimeout(() => setState((s) => quiet(s)), SAVED_MS);
    return () => window.clearTimeout(timer);
  }, [state.justSaved]);
  // Leaving the page (or picking another style) saves what was typed, after the save on its
  // way when there is one (the box is gone by then, so nothing waits on it).
  useEffect(
    () => () => {
      const { draft } = current.current;
      const saving = inFlight.current;
      if (draft === null) return;
      if (saving === null) {
        saveLater.current();
        return;
      }
      const after = () => void saveRef.current(draft).catch(() => {});
      saving.then(after, after);
    },
    [],
  );

  return {
    state,
    edit: (value: string) => setState((s) => edited(s, value)),
    saveNow,
    error,
  };
}

/** The prefix of new branches: checked as typed by git's rules, saved on Enter or leaving. */
function BranchPrefixRow() {
  const id = useId();
  const prefix = useApp((s) => s.settings.branchPrefix);
  const [typed, setTyped] = useState<string | null>(null);
  const save = useAction();
  const text = typed ?? prefix;
  const problem = branchPrefixProblem(text);

  const commit = () => {
    if (typed === null || problem) return;
    setTyped(null);
    if (typed !== prefix) save.run(() => setSetting("branchPrefix", typed));
  };

  return (
    <SettingsRow
      label={GIT_ROWS.branchPrefix.label}
      description={GIT_ROWS.branchPrefix.description}
      htmlFor={id}
      error={problem ?? save.error}
    >
      <Input
        id={id}
        value={text}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        placeholder="No prefix"
        aria-invalid={problem ? true : undefined}
        className="border-input bg-popover h-control-md rounded-control text-label w-56 px-2.5"
        onChange={(event) => setTyped(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") commit();
          if (event.key === "Escape") {
            // Esc here undoes the typing rather than leaving Settings.
            event.stopPropagation();
            setTyped(null);
          }
        }}
      />
    </SettingsRow>
  );
}
