# UI replay harness: what the live request block showed, second by second

## Run

    ARM=<arm-dir> [APP=<apps/desktop to replay through>] node tools/ab/replay/run.mjs

Needs `ARM/start.json` and `ARM/rec/events.jsonl`. Writes `ARM/ui-replay.json` and prints the
seconds where the block changed, then a summary. Set `REPLAY_HTML=1` to also dump each
second's rendered markup to `ARM/ui-replay-<t>.html`.

The harness lives in `tools/ab/replay/`:
- `run.mjs`: starts Vite in SSR/middleware mode (no config file, no port) with APP (default: this
  checkout's `apps/desktop`) as its root, so `.tsx` and `@/` imports load in Node. Sets the replay
  clock, browser stubs and the store-snapshot patch, then calls the harness. For the run it copies
  the two files below into `APP/.ab-replay/`, so their imports resolve against APP's packages,
  and removes them after.
- `harness.tsx`: the replay and the measurement. Fields newer app versions added to `BlockMeta`
  (`thinking`, `worked`, `quotaWait`) are passed when the block has them, so one harness replays
  through older and newer app code.
- `auiMock.tsx`: stands in for `@assistant-ui/react` inside `RequestBlock.tsx` only.

Nothing in the app's source is edited. Vite rewrites one import line of `RequestBlock.tsx` in
memory (see "Emulated" below).

## How it works

1. **State.** The harness starts the app's own stores the way a freshly opened, empty
   conversation would have them. `useApp` gets `selection` set to this conversation and
   `threads[conv] = emptyThread`. `useBoard` gets `board = {...emptyBoard(conv), loaded: true}`.
2. **Events.** It reads every recorded envelope in `seq` order. For each second `t` from
   `t0` (in `start.json`), it applies every envelope with `atMs <= t` in one batch through
   `applyEvents` (state/store.ts) and `applyBoardEvents` (state/board.ts). The live app's
   bridge (state/bridge.ts `flush`) calls these same two functions. The two other appliers it
   calls, activity and brain/usage, never feed the thread. The clock never runs backwards: if
   a later `seq` has an earlier `atMs`, it waits for the latest `atMs` seen so far.
3. **Derivation.** It builds the thread the way ConversationView does. The `BoardDigest`
   selection is copied from ConversationView. Then it calls `shownTexts(fullText,
   reportTexts(board.overnight))` and `buildThread(items, texts, hasMore, digest, [],
   session ? "edits" : "all")` from `app/conversation/blocks.ts`. It takes the block node
   whose `requestIds` include the sent message's `requestId`, and builds its `BlockMeta` field
   by field, as `useItems` in ConversationView.tsx does.
4. **Render.** It renders the real `<RequestBlock/>` with `react-dom/server`
   `renderToStaticMarkup`, inside `TooltipProvider` and `ViewContext.Provider` (holding the
   conversation from `useApp`, so `quotaWait` counts). That runs the app's real code:
   - `WorkHeader`, `headerLabel`, `blockSequence`, and the done/answering/fold logic in
     RequestBlock
   - the work groups of `activity/` (`groupActivity`, `ActivityGroup`, `StepRow`, `LeadStep`),
     with the one vocabulary of `activity/words.ts`
   - the team sentences and notices (`activity/TeamSentence.tsx`, `activity/Notice.tsx`)
   - `ThreadStatus`, the single live line (`liveStatus.ts`)

   Every `useBoard`/`useApp` selector reads the replayed stores.
5. **Read-out.** It parses the markup with parse5 and reads the `data-slot`s the components set:
   - `request-work-header`: the header text
   - inside `request-work`:
     - `aui_assistant-message-content`: **orchestrator notes**, the replies shown in the work
     - `orchestrator-step` and `compaction`: **action rows**. These are the grey step lines:
       Created/Landed/Decided/Answered…, and machine rows such as "Waiting for another build to
       finish". A `work-group` counts too: a run of the thread's finished tool steps folds into
       one row ("Searched code, ran a command"), and closed it renders only that summary line.
     - `task-row`: **worker rows** (a team sentence: "Started A and B", "A finished")
     - `task-activity`: a worker's current-activity line. The thread no longer shows one (each
       worker's progress is in the Workers strip on the composer), so it stays empty on newer
       app code.
     - `request-card` and `request-steer`: cards
   - `request-activity`: the live line ("Thinking", "Delegating to a worker", …)
   - when the work has folded: the answer text

The run ends at the first second where the request is on the board and its block is no longer
`working`/`waiting`. That covers the whole request, across every orchestrator turn: the block
stays `working` while workers run and the orchestrator is idle. If the request never ends, the
run stops at the last event plus 1 s, with `finished: false`.

### Per-second frame (`frames[]` in ui-replay.json)

`t` (s after send), `state`, `header`, `orchestratorNotes[]`, `actionRows[]`, `workerRows[]`,
`workerActivity[]`, `cards`, `activity` (the live line), `answer`, and two flags:
- `onlyThinking`: the block is live and its work has no note, action row, worker row or card,
  and the live line is "Thinking" or absent. The header may show ("Working for 1m 3s" over
  nothing still counts).
- `noContent`: the same, but whatever the live line says ("Delegating to a worker" counts too).

### Summary

- `longestOnlyThinking` {start, length}
- `onlyThinkingOver30s`
- `onlyThinkingTotalS`
- `longestNoContent`
- `firstWorkerRowS`, `firstActionRowS`, `firstOrchestratorNoteS`, `firstHeaderS`
- `durationS` (the first second at which the block shows done) and `requestEndedS` (exact,
  from the request's `endedAtMs`)
- `finished`, `finalState`

## Emulated, not exercised

- **getConversation (initial view).** It's emulated as an empty conversation: no messages or
  tasks, run idle. That matches a brand-new conversation opened before the first send, and the
  recording starts before `conversationCreated`. Every later change comes from the recorded
  envelopes, through the same appliers as the live feed.
- **Pending (optimistic) message.** Not emulated. Until the stored user message and request
  arrive (in the a-brig run, the same millisecond), the frame is flagged as the pending block
  (`state: null`, `onlyThinking: true`). The app's pending block shows the same thing: a block
  working with nothing in it.
- **assistant-ui.** `RequestBlock` reads its `BlockMeta` and replies through `useAuiState`.
  The mock returns what `convertMessage` would put on the message: `metadata.custom.block`,
  text parts, and a status. `MessagePrimitive.PartByIndex` renders the reply as plain text, not
  Markdown, and without the streaming word fade. `ActionBarPrimitive` is a passthrough.
- **`useItems`' `BlockMeta`.** Copied field by field because it's a hook with a cache.
  `rework` is fixed to `false`; it only decides "Try again"/Edit.
- **Not read at all, because the request block doesn't use them:**
  - listWorkerEvents: worker transcripts are only for an opened worker panel
  - listOrchestratorLog: the Inspector
  - getWorkerDiff: `board.diffs`. Its only effect in the block is the `+N −M` suffix on a
    worker's activity line, so that suffix never shows here.
  - Worker activity, summaries and `doing` come from `workerEvent` and `orchestratorLogged`
    envelopes, exactly as live.
- **Clock and environment.**
  - `Date.now()` returns the replay time. It's set to `t0` before modules load, so
    module-level clocks (use-activity-clock) start there. Timers and effects never run.
  - zustand's and other hooks' server snapshots are swapped for the current snapshot (a
    `React.useSyncExternalStore` patch), so the server render reads the live state, as a
    client render would.
  - Minimal `window`, `document`, `localStorage` and `getComputedStyle` stubs.

## Known fidelity gaps

- **1-second sampling.** A state that lasts under a second between two samples can be missed.
  Events are batched per second rather than per microtask; a snapshot can't see the
  difference.
- **No CSS or layout.** Text is the DOM text:
  - Long lines (for example "Created Lead with the instructions: …") are truncated on screen
    but complete here.
  - Hover-only things (chevrons, timestamps) carry no text anyway.
  - Scroll position and what is off-screen aren't modelled.
  - Collapsed step details and tooltips (portals) don't render, matching the closed UI.
- **Elapsed times use the event clock (`atMs`), not `recv_ms`.** If the daemon stamped an
  event earlier than the app would have painted it, the replay shows it slightly early.
- **Effects never run.** Any row that only appears after an effect-driven IPC read would be
  missing. None was found in the block's live work. TurnDiff renders only once the request is
  over. A blob-backed long message isn't swapped for its full text (`loadFullText` is an
  effect), so a long reply shows its stored preview; that changes the text, not whether a
  note is visible.
- **The pending block's first fraction of a second** is approximated, as described above.
- **Only the main view is replayed.** A side-chat board, the Workers strip on the composer and
  the side panel aren't.

## Self-test

    [APP=<apps/desktop>] tools/ab/replay/selftest.sh

Replays `fixture/` (the first 10 s of the phase-3 T1 arm `t1-p3b`, its recorded events with the
log, cleanup, settings and provider-check envelopes left out) and checks that the folded row of
the thread's first tool steps ("Searched project memory, searched code") counts as a row from
the second it shows. It fails on a harness that reads only `orchestrator-step` rows.
