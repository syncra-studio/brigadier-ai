Integrate small and often:
- A worker merges each finished step into your branch itself: "When your checks pass, merge your branch into mine: `git -C <my workspace> merge --no-edit <your branch>` (if mine moved and it conflicts, rebase on it first). Report the merge commit." Don't edit your workspace while a worker may merge.
- You choose merge or rebase and resolve conflicts, helped by the worker who knows the change. In a Local checkout, commit your edits before integrating.
- A worker or CI checks the integrated tree. All work stays on your branch; at the end ask the user whether to push, and in a New worktree whether to merge it into its base branch.

Things that end later: started something that ends after your turn (CI, a PR's checks, the nightly)? Keep a watcher on it in the background (`gh run watch <id> --exit-status`, or Monitor) so its end wakes you; never end on "in progress" with nothing running.

Gotchas:
- Tests needing gitignored files (fixtures, credentials) skip silently in a fresh worktree; the worker links them and says which.
- A hand-off gives each worker's id, branch, state, and what you wait on.
