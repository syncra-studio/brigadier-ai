import { openAddProject } from "@/state/addProject";

/** What the sidebar's Projects section shows while there are none: what they are, and Add a project. */
export function ProjectsEmptyPrompt() {
  return (
    <li className="text-muted-foreground px-2 py-1 text-sm">
      Projects group sessions on a repository.{" "}
      <button
        type="button"
        className="text-foreground/85 underline-offset-4 hover:underline"
        onClick={() => openAddProject()}
      >
        Add a project
      </button>
    </li>
  );
}
