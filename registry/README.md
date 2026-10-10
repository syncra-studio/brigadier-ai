# registry

The curated model registry. It is published from this repository so installed apps can update
it without a new release. Brigadier's router (`crates/router`) reads it to decide which model
takes each task; see [docs/PLAN.md](../docs/PLAN.md) §6 Phase 5.

Don't confuse it with `crates/registry`, which is the MCP, plugin and skill registry.

## The document

```json
{
  "schemaVersion": 1,
  "revision": 1,
  "updated": "2026-09-29",
  "models": [ { "key": "claude-opus-5-5", "vendor": "anthropic", "cli": "claude", … } ]
}
```

| Field | Meaning |
|---|---|
| `schemaVersion` | The format. An app refuses a version it doesn't know and keeps the registry it has. |
| `revision` | Goes up with every published change. An app only takes a copy with a higher revision than the one it uses, so there is no rollback. The copy bundled in a newer app beats an older downloaded one. |
| `updated` | The date of the revision, `YYYY-MM-DD`. |
| `models[].key` | A stable, unique name for the entry. |
| `models[].vendor` | Who makes the model (`anthropic`, `openai`). |
| `models[].cli` | The CLI that runs it: `claude` or `codex`. |
| `models[].match.ids` | The ids and aliases the CLI lists for this model (`opus`, `claude-opus-5-5`). A listed model whose id, or the concrete model its alias resolves to, is in this list is **curated**. |
| `models[].match.family` | The family word (`opus`, `sol`). Put it only on a family's newest entry. A listed model with no entry of its own inherits that entry if its id contains the word and it is not older than the entry's own ids. So `gpt-6.2-sol` inherits the `sol` entry, and routing marks it **inherited**. |
| `models[].tier` | `frontier`, `strong`, `standard` or `light`; `unrated` is kept for models nobody has placed yet. A task never runs below its quality floor: `strong` for implementation, reviews, merges, orchestration and operating apps; `standard` for research; `light` for scouting, checks and chat. |
| `models[].efforts` | The reasoning efforts it accepts, from `low`, `medium` and `high`. |
| `models[].contextWindow` | The context window in tokens. |
| `models[].knowledgeCutoff` | The vendor's reliable knowledge cutoff, `YYYY-MM`. Leave it out when the vendor hasn't published one. |
| `models[].modalities` | `input` and `output` (`text`, `image`), and `tools` (`webSearch`, `imageGeneration`). |
| `models[].strengths` | 0–10 for each task category: `scout`, `research`, `implement`, `review`, `merge`, `verify`, `operate`, `chat`, `orchestrate`. This is how well suited the model is to that category, weighing quality against cost, so a frontier model scores low for scouting. A missing category counts as 5. |
| `models[].areaStrengths` | −2…+2 per area (`frontend`, `backend`, `infra`, `docs`, `tests`). The router adds it to the category strength for a task in that area. |
| `models[].defaultEffort` | The effort to run the model at for each category. |
| `models[].released` | The release date, `YYYY-MM-DD`. |
| `models[].sources` | The release notes, model pages and benchmarks the entry is based on. |

## What the registry decides, and its bounds

A registry entry is authoritative for a model's tier, strengths, area modifiers, efforts,
context window, knowledge cutoff and modalities. That is what the registry is for. The app
enforces these bounds on every copy, bundled or downloaded, whatever the data says:

- Only CLIs Brigadier drives (`claude`, `codex`). Entries for any other CLI are skipped.
- **No Fable models.** Any entry naming one is dropped.
- Efforts are limited to `low`, `medium` and `high`. A default above `high` becomes `high`,
  and one below `low` becomes `low`.
- Strengths are clamped to 0–10 and area modifiers to ±2.
- A context window outside 8,000–20,000,000 tokens counts as unknown.
- A capability is kept only if the CLI's adapter implements it. Claude gets web search. Codex
  gets web search and image generation, so image output is Codex-only.
- A name the app doesn't know (a modality, a capability, a category or an area) is dropped
  where it appears. The rest of the document still reads, so new names can arrive within a
  schema version.
- The document is at most 1 MB. Keys must be unique, no id may be claimed by two entries, and
  at least one usable entry must be left. Otherwise the copy is refused.

The user's routing rules (never, prefer, only) and the hard rules (no Fable, effort at most
`high`, the sandbox) always beat registry data. The app also checks each entry against what
the CLI itself reports: efforts are the intersection of both lists, and so is image input.

## Updating it

1. Edit `models.json`, raise `revision` by one and set `updated`.
2. Cite a source in `sources` for every fact you add. Don't guess numbers. Leave out a field
   you can't confirm.
3. Check that the app still builds. The bundled copy goes through the same checks when it is
   read, so an invalid document fails at startup in development.

Installed apps fetch
`https://raw.githubusercontent.com/stephen-golban/brigadier-ai/main/registry/models.json`
over HTTPS, at most once a day, with `If-None-Match`. They keep a copy in their data
directory. The copy is **not signed yet**: its integrity rests on HTTPS from GitHub plus the
checks above. Signing arrives in Phase 10 and will use the same minisign key as the app
updater.

## On-demand ranking research

The daemon's `refreshRankings` request returns a job id immediately. It checks the published
registry first, then uses a strong enabled CLI model with read-only web tools to research the
live catalog. `getRankingsRefresh` reports progress, sources, changed fields and errors;
`resetRankings` cancels pending research and removes its overlay. `rankings.changed` tells
clients to reload model summaries and route previews.

Sourced patches may change only tier, category strengths, area modifiers and supported category
default efforts. They identify a provider and concrete model, never a family. They live in
`cache/registry/overlay.json`, independently of the published cache and ETag, and apply only
at their host-captured base revision. A newer published or bundled revision supersedes them.
Invalid or unsourced models keep their previous ratings. Manual rankings, learning, user rules
and trials for uncurated models continue to apply.
