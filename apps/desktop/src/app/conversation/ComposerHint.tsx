import type { ComposerTarget } from "@/app/conversation/composerTarget";
import { ProjectSettingsButton, ProvidersButton } from "@/app/shell/SetupPickers";

/** A setup problem that blocks sending. */
export function ComposerHint({ target }: { target: ComposerTarget }) {
  const { conversation, resolved } = target;
  if (conversation) return null;
  const { project, problem, repo } = resolved;
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
  return null;
}
