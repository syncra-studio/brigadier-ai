# Merge and Revert

When a session's work is ready, the thread asks before pushing or merging into the base branch.

## What happens

1. The thread checks the work and gets a review when needed. It asks before pushing or merging
   into the base branch. You can direct it to merge locally or use a pull request.
2. A line in the chat records the merge.

## Revert

Under the merge message there is a **Revert** button. It asks the agent to undo that merge the way
it went in: a revert commit on the base branch, pushed if the merge was pushed. The agent reports
when it's done.

You can also just ask in the chat, in your own words.
