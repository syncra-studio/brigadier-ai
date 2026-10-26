# Thin-read regression fixtures (2026-10-26)

Follow-up to OURS-AUDIT.md, “After phase 1” (bfa288096), and worker ledger L14-L17.
This supersedes the thin-tree/cache behavior in lean-client-evidence.md; its client
rendering observations still apply.

The original audit scratch directory had been deleted. These five fixtures are
fresh captures of the same kinds of windows using the installed signed helper in
a scratch home: TextEdit with a scratch text file, Finder with three scratch files,
Safari with a readable local HTML page, Safari with a canvas chart, and Preview
with a chart PNG. Captures requested tree_format:both, include_screenshot:false,
with the normal walk limits. The front app did not change. The focus lock covered
launch, reads and cleanup. Scratch input documents/files and helper were cleaned
up; Finder ownership cleanup found zero remaining windows into the scratch tree.

Labels and string values are redacted, preserving empty/nonempty distinctions;
boolean/numeric state strings, roles, hierarchy, indices and geometry remain.
Titles and their matching title-bar text use the same replacement. Raw pointers
are normalized. The TextEdit body contains only generated scratch text.

| Fixture | Indexed elements | Automatic screenshot before | After |
| --- | ---: | --- | --- |
| TextEdit | 14 | yes | no |
| Finder | 66 | yes | no |
| Safari, readable page | 31 | no | no |
| Safari, canvas | 26 | yes | yes |
| Preview, chart PNG | 21 | yes | yes |

Before uses the exact thin_tree rule from bfa288096 on each captured markdown
view. After is asserted through View::fallback_arguments in the fixture tests.
For no-change since-diffs following *each* fixture, after is always **no**;
before inherited each full read's thin flag and captured on four of the five.
These are classification regressions, not a fresh A/B/C task benchmark.

The new classifier normalizes AX, UIA and AT-SPI role names, not substrings in
labels, values or identifiers. It includes unindexed readable markdown rows.
Media must cover at least 10% of the window, with both clipped dimensions at
least 64 units in the backend's window coordinates, and lack readable descendants. Small/out-of-window
icons do not qualify. Menu/toolbar labels and the title-bar filename do not
stand in for document contents: Preview exposes no image content at all in its
tree, so its toolbar-only body still gets a screenshot. Empty/inaccessible
windows remain automatic-image cases.

The shim requests the already-built structured elements alongside markdown
internally, avoiding another AX walk. Geometry is removed again for a default
or explicit markdown reply; explicit elements/both/full_output choices remain.
The same projection covers run_steps final observations without repeating steps.
Truncated reads and unknown changed-diff baselines retain the reviewer's cautious
image fallback; no-change diffs always skip automatic capture. Screenshots, errors,
snapshot ids and the original tree stay intact.

The captured Finder view controls have descriptions. A separate fixture variant
removes their four descriptions and structured labels to represent a toolkit
with an icon-only toolbar. It produces exactly one visual_hint:

> 4 controls are unlabelled; a screenshot would show them (include_screenshot:true).

It does **not** request an image. Hints count groups of at least three visible
unlabelled controls (at least 12×12 in the backend's window coordinates), excluding window chrome,
invisible accessibility buttons and file-name fields with readable values.
Explicit screenshot requests remain honoured; explicit false disables fallback.
