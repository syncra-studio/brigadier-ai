import { useEffect, useState } from "react";

import { CheckboxRow, ErrorLine, errorText } from "@/app/dialogs/fields";
import { Spinner } from "@/components/icons";
import { Badge, Button } from "@/components/controls";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/overlays";
import type { RemovalBranch, UninstallPlan, UninstallReport } from "@/ipc/generated";
import { formatBytes } from "@/lib/format";
import {
  closeStorageDialogs,
  previewUninstall,
  quitApp,
  uninstall,
  useStorageUi,
} from "@/state/storage";

/**
 * Uninstall Brigadier… (Settings → Storage, or the app menu): a preview of everything that
 * goes, then the teardown, then what happened and one way on: quitting, after which the data
 * folder (unless kept), the per-app folders and the app go to the Trash. Once the teardown has
 * started the dialog can't be dismissed.
 */
export function UninstallDialog() {
  const open = useStorageUi((s) => s.uninstallOpen);
  const [locked, setLocked] = useState(false);
  return (
    <Dialog open={open} onOpenChange={(next) => !next && !locked && closeStorageDialogs()}>
      <DialogContent className="max-w-lg max-h-full overflow-y-auto" showCloseButton={!locked}>
        {open && <UninstallFlow onLocked={setLocked} />}
      </DialogContent>
    </Dialog>
  );
}

type Phase =
  | { type: "loading" }
  | { type: "plan"; plan: UninstallPlan }
  | { type: "running"; plan: UninstallPlan }
  | { type: "done"; report: UninstallReport };

function UninstallFlow({ onLocked }: { onLocked: (locked: boolean) => void }) {
  const [phase, setPhase] = useState<Phase>({ type: "loading" });
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    previewUninstall().then(
      (plan) => current && setPhase({ type: "plan", plan }),
      (cause: unknown) => current && setError(errorText(cause)),
    );
    return () => {
      current = false;
    };
  }, []);

  const run = (plan: UninstallPlan, keepData: boolean, branches: RemovalBranch[]) => {
    setPhase({ type: "running", plan });
    setError(null);
    onLocked(true);
    uninstall(
      plan.planId,
      keepData,
      branches.map((branch) => ({
        repo: branch.repo,
        name: branch.name,
        tip: branch.tip,
      })),
    ).then(
      (report) => setPhase({ type: "done", report }),
      (cause: unknown) => {
        // Nothing started (a stale preview, or another uninstall running): back to the plan.
        onLocked(false);
        setError(errorText(cause));
        setPhase({ type: "plan", plan });
      },
    );
  };

  if (phase.type === "done") return <Done report={phase.report} />;
  if (phase.type === "loading" || phase.type === "running") {
    return (
      <div className="grid gap-4">
        <DialogHeader>
          <DialogTitle>Uninstall Brigadier</DialogTitle>
          <DialogDescription>
            {phase.type === "loading"
              ? "Looking at everything Brigadier created…"
              : "Stopping everything and removing what Brigadier created…"}
          </DialogDescription>
        </DialogHeader>
        {!error && <Spinner className="size-icon-sm animate-spin" />}
        <ErrorLine error={error} />
        {phase.type === "loading" && (
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={closeStorageDialogs}>
              Cancel
            </Button>
          </DialogFooter>
        )}
      </div>
    );
  }
  return <Plan plan={phase.plan} error={error} onUninstall={run} />;
}

function Plan({
  plan,
  error,
  onUninstall,
}: {
  plan: UninstallPlan;
  error: string | null;
  onUninstall: (plan: UninstallPlan, keepData: boolean, branches: RemovalBranch[]) => void;
}) {
  const [keepData, setKeepData] = useState(false);
  const [picked, setPicked] = useState(
    () => new Set(plan.branches.filter((b) => b.merged && !b.checkedOut).map((b) => key(b))),
  );
  const pick = (branch: RemovalBranch, on: boolean) =>
    setPicked((set) => {
      const next = new Set(set);
      if (on) next.add(key(branch));
      else next.delete(key(branch));
      return next;
    });

  return (
    <div className="grid gap-5">
      <DialogHeader>
        <DialogTitle>Uninstall Brigadier?</DialogTitle>
        <DialogDescription>
          Brigadier removes what it created on this computer, quits, and then moves itself to the
          Trash. Your repositories, your own branches and your files are not touched.
        </DialogDescription>
      </DialogHeader>

      {plan.running.length > 0 && (
        <p className="text-sm">
          These stop: <span className="text-muted-foreground">{plan.running.join(", ")}</span>.
        </p>
      )}

      <section aria-labelledby="uninstall-items" className="grid gap-1.5">
        <h3 id="uninstall-items" className="text-muted-foreground text-xs">
          What goes
        </h3>
        <ul className="grid gap-1 text-sm">
          {plan.items.map((item) => (
            <li key={item.label} className="flex items-start gap-2">
              <span className="min-w-0 flex-1 break-all">{item.label}</span>
              {item.toTrash && <Badge variant="secondary">Trash</Badge>}
              <span className="text-muted-foreground shrink-0 text-xs tabular-nums">
                {formatBytes(item.bytes)}
              </span>
            </li>
          ))}
          {plan.sudoersRule && (
            <li>
              The rule that keeps the computer awake with the lid closed (asks for your
              administrator password once).
            </li>
          )}
          {plan.microphone && <li>The microphone permission.</li>}
          <li>
            {plan.app.type === "bundle"
              ? `The app ${plan.app.path} (${formatBytes(plan.app.bytes)}), to the Trash once it quits.`
              : plan.app.type === "noBundle"
                ? "This build doesn't run from an app bundle, so there is no app to move."
                : plan.app.how}
          </li>
        </ul>
      </section>

      <CheckboxRow
        label="Keep my data"
        note={
          <>
            Sessions, chats, Brains and settings stay in{" "}
            <span className="break-all">{plan.dataDir}</span> ({formatBytes(plan.dataBytes)}).
            Unchecked, that folder goes to the Trash.
          </>
        }
        checked={keepData}
        onCheckedChange={setKeepData}
      />

      {plan.branches.length > 0 && (
        <section aria-labelledby="uninstall-branches" className="grid gap-2">
          <h3 id="uninstall-branches" className="text-muted-foreground text-xs">
            Branches Brigadier made
          </h3>
          <p className="text-muted-foreground text-xs">
            Checked ones are deleted. Unchecked ones stay, and the end lists the command that
            deletes each.
          </p>
          <div className="grid gap-2">
            {plan.branches.map((branch) => (
              <CheckboxRow
                key={key(branch)}
                label={`${branch.name} · ${branch.repo}`}
                checked={picked.has(key(branch))}
                disabled={branch.checkedOut}
                onCheckedChange={(on) => pick(branch, on)}
                note={
                  branch.checkedOut
                    ? "Checked out in a worktree Brigadier doesn't own, so it stays."
                    : branch.merged
                      ? `Merged into ${branch.target}.`
                      : `Not merged: ${branch.ahead} ${branch.ahead === 1 ? "commit" : "commits"} ${branch.target} doesn't have. Deleting it loses them.`
                }
              />
            ))}
          </div>
        </section>
      )}

      <ErrorLine error={error} />
      <DialogFooter>
        <Button type="button" variant="ghost" onClick={closeStorageDialogs}>
          Cancel
        </Button>
        <Button
          type="button"
          variant="destructive"
          onClick={() =>
            onUninstall(
              plan,
              keepData,
              plan.branches.filter((branch) => picked.has(key(branch))),
            )
          }
        >
          Uninstall
        </Button>
      </DialogFooter>
    </div>
  );
}

function Done({ report }: { report: UninstallReport }) {
  const [quitting, setQuitting] = useState(false);
  return (
    <div className="grid gap-4">
      <DialogHeader>
        <DialogTitle>Almost done</DialogTitle>
        <DialogDescription>
          Quit Brigadier to finish: once it has quit, what it still used goes to the Trash.
        </DialogDescription>
      </DialogHeader>
      <ul className="grid gap-2 text-sm">
        {report.steps.map((step) => (
          <li key={step.label} className="grid gap-0.5">
            <span className="flex items-start gap-2">
              <Badge variant={step.ok ? "success" : "destructive"}>
                {step.ok ? "Done" : "Not done"}
              </Badge>
              <span>{step.label}</span>
            </span>
            {step.detail && (
              <span className="text-muted-foreground text-xs break-all whitespace-pre-line">
                {step.detail}
              </span>
            )}
          </li>
        ))}
      </ul>
      {report.keptBranches.length > 0 && (
        <section aria-labelledby="uninstall-kept" className="grid gap-1.5">
          <h3 id="uninstall-kept" className="text-muted-foreground text-xs">
            Branches kept
          </h3>
          <ul className="grid gap-2 text-sm">
            {report.keptBranches.map((branch) => (
              <li key={key(branch)} className="grid gap-0.5">
                <span>{branch.name}</span>
                <code className="text-muted-foreground text-xs break-all select-text">
                  {branch.command}
                </code>
              </li>
            ))}
          </ul>
        </section>
      )}
      {report.afterQuit.length > 0 && (
        <section aria-labelledby="uninstall-after" className="grid gap-1.5">
          <h3 id="uninstall-after" className="text-muted-foreground text-xs">
            After Brigadier quits
          </h3>
          <ul className="text-muted-foreground grid gap-1 text-xs">
            {report.afterQuit.map((path) => (
              <li key={path} className="break-all">
                {path}
              </li>
            ))}
          </ul>
        </section>
      )}
      <DialogFooter>
        <Button
          type="button"
          disabled={quitting}
          onClick={() => {
            setQuitting(true);
            void quitApp();
          }}
        >
          Quit Brigadier
        </Button>
      </DialogFooter>
    </div>
  );
}

function key(branch: RemovalBranch): string {
  return `${branch.repo}\n${branch.name}`;
}
