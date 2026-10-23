import type { ComposerTarget } from "@/app/conversation/composerTarget";
import { ProjectSettingsButton, ProvidersButton } from "@/app/shell/SetupPickers";

/** One line under the composer: what blocks sending, or what the setup implies. */
export function ComposerHint({ target }: { target: ComposerTarget }) {
  const { conversation, resolved } = target;
  if (conversation) return null;
  const { project, problem, repo, permission } = resolved;
  if (problem) {
    return (
      <p className="text-muted-foreground flex items-center gap-2 px-2 text-xs">
        {problem}
        {resolved.noModel && <ProvidersButton />}
        {project && !resolved.repoPath && <ProjectSettingsButton project={project} />}
        {repo.error && resolved.kind === "session" && (
          <span className="text-destructive">{repo.error}</span>
        )}
      </p>
    );
  }
  // A new worktree starts from its base's last commit: what isn't committed stays out of it.
  const dirty = repo.info?.dirty;
  const local = resolved.environment === "localCheckout";
  if (resolved.kind !== "session" || (permission !== "full" && !dirty)) return null;
  return (
    <p className="text-muted-foreground px-2 text-xs">
      {permission === "full" && (
        <span className="text-full-access">
          Full access: workers run commands without asking.{" "}
        </span>
      )}
      {dirty &&
        (local
          ? "The session works in your checkout, uncommitted changes and all."
          : "The session starts clean on a worktree of its own: your checkout's uncommitted changes stay in your checkout.")}
    </p>
  );
}
