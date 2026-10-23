# Continuity: nothing gets forgotten across hand-offs

Research tab w96, run `20261022-0805-take-over-5-sessions`, 2026-10-23. The question (the user, 10-23): from
Delegator 1 to Delegator 9 a lot was forgotten or left out, and Brigadier can fail the same way. Find a fix that
is better than what the rivals do.

This builds on the run's earlier research and doesn't repeat it: `EXCEL-long-sessions.md`,
`EXCEL-orchestration.md`, `PROPOSAL.md` and `EVIDENCE.md`, all in the run folder's `research/` (not in this
repo). Those cover tokens, compaction and the seam's mechanics. This document covers one thing they don't:
**decisions and instructions that get lost or changed on their way from the user to the work.**

**Evidence.** All of it is in the run folder (local, private): the run ledger (`ledger.md`, 487 lines), the
six Delegator hand-off notes (`msgs/delegator-{1,2,3,5,6,7}-handoff.md`), 30 worker hand-offs, the plan
writer's brief (`msgs/w12-task.md`), and w95's audit of the grill against the plan and the build
(`workers/w95/report.md`, 20 items, read from the three grill transcripts). Short quotes only here: this repo
is public.

## 1. Diagnosis

### 1.1 What was lost, by kind of failure

Each loss is put in the one class that best explains it. Classes (a)–(e) are the ones the task named; (f)–(h)
had to be added, because the evidence didn't fit (a)–(e).

| Class | What it means | Count | The items |
|---|---|---:|---|
| (a) never written down | the fact never reached any file | 1 | w95-13: an old engine sweep empties `tmp/` every turn; nobody listed it against the G13 design |
| (b) written, never read | the reader that mattered never had the source | 1 | H12: the Codex plan review got the plan only, not the grill, so it couldn't see drift |
| (c) read, not applied | the source was in front of the reader | 4 | w95-2 (a worker can't start workers, against plan §8.13), w95-15 (`disableAllHooks` override, against G17), H3 (the 02:45 question carried by three hand-offs, never asked again when the user came back), H4 (the Stage 3 verifier's three "Waiting on you" items never shown while the user was live) |
| (d) written where nobody looks | in a file, in the wrong place | 6 | w95-4 (no context ring: in §1, in no step), w95-8 (Q9 idle-parent notice: in §9, not in S13), H2 (terms check: the ledger has three "Waiting on you" sections; D6 and D7 dropped it), H5 (12 of 17 decisions taken for the user sit in the Timeline, not in "Decided on your behalf"), H6 ("Waiting on you: G12" never closed after G12 closed), H7 (w88's Codex 1M and failure-hook probe results: in a hand-off and the ledger, not in the plan, until w92 wrote them into S22 in `b5c3a43bc` on 10-23) |
| (e) summarised into vagueness | kept, but the part that mattered was dropped | 4 | S-1 (Sources: the memory note kept the list and dropped the rule, "what this thread looked at from outside the repo", and "searches"), w95-6 (the agreed PR words and colours became "state word + colour"), w95-14 ("you can read and edit the same file in Settings" was dropped), H1 (D3's hand-off named run-folder research as `research/…`, so this very task pointed at the repo) |
| (f) added by a writer, with no source | a plan or research line nobody decided, read later as decided | 7 | S-2 (the ledger shows in Sources), w95-1 (Chats can't run workers), w95-9 (the successor waits for the next user message), w95-10 (a successor on account switch or model change), w95-11 (the cold-cache rule), w95-12 (sidebar sessions can't message each other), w95-16 (the shipped rules file forbids Fable for everyone) |
| (g) decided for the user, never shown | a Delegator, verifier or memory note settled what the user owned | 7 | w95-3 ("pushback accepted" written to memory; the user never saw the pushback), w95-5 (computer use reaches no session), w95-7 (Codex network on at Ask/Accept edits), w95-17 (one session per Local checkout), w95-18 (AskUserQuestion denied), w95-19 (folder trust forces Ask), w95-20 (a Chat's shell runs only `brigadier`) |
| (h) the hand-off itself misfired | two owners, typed clock, lost text | 3 | H8 (Delegator 5 kept acting for 37 min after it handed over: two culprits named for one CI failure, 18:39 and 19:05), H9 (ledger times ran 4h40 ahead for ~12 hours; times typed out of order, e.g. 11:15 then 11:11), H10 (`dlg send` let the shell eat backticks; one message lost a word) |
| **Total** | | **33** | |

`w95-N` = item N of `workers/w95/report.md`. `H` = found in the Delegator hand-offs and ledger for this
document. `S-1`, `S-2` = the Sources case the user found.

### 1.2 What survived, and why

- **30 worker hand-offs, no proven loss.** The worker hand-off has fixed sections (done with hashes, in
  progress with exact files, approvals already given). Corrections travel as files named by path ("go-ahead
  and corrections in `msgs/w63-reply-1.md`; w69 inherits them"). Items addressed to a role with a must
  survived and were acted on: w75's "VERIFIER MUST CHECK" went through w77 to w78, which judged each
  (`w78/report.md:153–155`); w84's deviations were judged by w85/w86.
- **Rules kept with their date and the user's words survived every Delegator hand-off.** "The user asked
  (10:37) that the Delegator runs on Fable 5.1" opens hand-offs 2, 3, 5, 6 and 7, word for word. So does the
  02:55 accounts rule.
- So the hand-off note is not where most things were lost. **Only 7 of the 33 happened at a hand-off** (H1–H4,
  H7, H8, H10). The other 26 happened upstream: when a decision was written down, when a document was derived
  from it, or when someone decided in the user's place.

### 1.3 The three findings that decide the fix

1. **Provenance is the main failure, not memory.** Classes (f) and (g) are 14 of 33. In both, the text
   survived perfectly; what was lost is *who decided it*. A plan line the writer invented reads exactly like
   a line the user dictated. After that, every successor and verifier faithfully carries the error. Better
   recall makes this worse, not better. The fix must make the source of every line visible.
2. **Summaries keep examples and drop the rule.** The memory note's Sources line kept "attachments, fetched
   URLs, MCP servers" and dropped "from outside the repo". With the rule gone, a writer could not tell that
   the ledger doesn't belong there. The same happened to the PR list and the Settings promise. The fix must
   keep the user's defining sentence verbatim, not a list of instances.
3. **Open items have no owner and no moment.** Questions and "waiting on you" items were carried faithfully
   and never asked again, because nothing says *when* to raise them. The user was live from 11:10 on 10-23;
   four open items waited through it. The fix must give each open item one list and one trigger.

### 1.4 What today's continuity battery measures, and what it misses

The S21 battery (`tools/continuity-battery/`, baseline 11/12) plants twelve items and, at the end, *asks* for
them. Every pass came from the successor searching the record when asked (report `docs/evidence/2026-10-22-claude-continuity.md`,
"Successor searched? yes" on every row). None of this run's 33 losses was a fact someone was asked for and
couldn't find. They were rules nobody re-applied, documents that added or dropped things, and questions
nobody raised again. **The battery needs items that test acting on a decision without being asked, deriving
a document from decisions, and raising open items.**

## 2. What others do

Sources were read on 10-23: shallow clones at pinned commits, read and then deleted, plus vendor docs and
papers. Full notes with `file:line` and URLs are in the run folder, at
`workers/w96/continuity-sources.md`. Licences: T3 Code `d7a8d6f3`, bb `1e80ca7` and Orca `b4ee652b` are MIT.
Superset `853e3e8d` is Elastic 2.0, so we take ideas only. OpenHands is MIT. We take designs, never code.

### 2.1 Products

| Product | What it carries across a restart or hand-off | Verbatim or summary | Successor made to read it? | Checked against it? | Numbers |
|---|---|---|---|---|---|
| Claude Code | CLAUDE.md, auto memory, the plan-mode plan and the task list are re-read from disk after compaction. Chat-only instructions are summarised ("detailed instructions from early in the conversation may be lost"; the docs say to move them into CLAUDE.md) | files verbatim, chat summarised | the files are re-injected; nothing asks for an acknowledgement | no; a `PostCompact` hook sees the summary but can't act on it | none |
| Codex | AGENTS.md every turn; memories made in the background from idle chats ("a recall layer, not a guaranteed source of rules"); the cookbook's ExecPlan has a Decision Log | AGENTS.md verbatim, compaction summary unreadable | by convention | no | a 7-hour run, an anecdote |
| T3 Code | a plan is a stored record with a status (draft, active, completed, superseded). "Implement plan" starts a thread whose first message *is* the plan. To-do lists are dropped from cross-provider hand-offs. User constraints are not pinned | verbatim plan | yes, it is the first message | only that the plan is active, not the output | none |
| bb | memory records typed fact, preference, decision and more, with pinning, importance and versions; a ≤3,900-char catalog injected every turn; a hand-off is just "Continue from @thread" | summaries in the catalog | sees the catalog, opens nothing | no; memory ranks below the user's requests | none |
| Orca | task specs must name Target, Change, Constraints, Ownership and Observable acceptance; decisions go through durable "gates"; the continuation prompt ends "Briefly state where the previous session stopped" | verbatim spec and transcript path | the only restate step found in these products | the dispatch, not the acceptance | none |
| Superset (ideas only) | hand-off reads every user and assistant turn from the CLI's session file (36k chars); its plan template has a Decision Log, and the rule is "restart from only the plan" | verbatim turns | by convention | no | none |
| Cline / Roo | memory-bank files (`activeContext.md`, `progress.md`, Roo's `decisionLog.md`), "I MUST read ALL memory bank files at the start of EVERY task"; `/newtask` writes a context block the user can edit | model-written files | by prompt only | no | none |
| Cursor, Windsurf, Devin | Cursor removed memories in 2.1 and points to rules; Windsurf memories are retrieved "when relevant", with no guarantee; Devin knowledge is pinned per repo or by trigger, and playbooks list Forbidden Actions | rules verbatim, memories by relevance | rules always; memories not | no | none |
| Amp | replaced compaction with a hand-off: a goal-focused first prompt and file list that the user reviews and edits | drafted, user-edited | the user does | no | none |
| Kiro | requirements as EARS acceptance criteria, each turned into a property test tagged "Validates Requirement N" | verbatim requirements | yes | **yes, by tests** | none on retention |
| GitHub Copilot memory | each fact is stored with `path:line` citations and checked against the current code before use | facts with citations | checked just in time | **yes, against code** | PR merge rate 83% → 90% in an A/B (vendor) |

### 2.2 Measured results

- **Lost in Context Compaction** (Wang et al., arXiv 2608.11242, v2 2026-10-11; checked against the abstract):
  only **17%** of user side-constraints survive compaction on average, and most compactors did worse than no
  compression. An inbox agent deleted mail without approval in **30 of 100** sessions after compaction, against
  0 before. A separate extractor that reads only the user's turns and keeps constraints apart from the summary
  kept **over 90%** in all three scenarios. The agent's notes also report restating the constraint cut the
  violations from 30% to 13%; that figure is from the paper's body, not re-checked.
- **Governance Decay** (Chen, arXiv 2606.22528, 2026-06; checked against the abstract): 0% policy violations
  with the policy in context, **30%** after compaction (up to 59%), and **0% with the constraints pinned**
  outside the compaction.
- **LLMs get lost in multi-turn conversation** (Laban et al., arXiv 2505.06120): a 39% drop when instructions
  arrive across turns. Restating all the user's turns together at the end recovers about half of it.
- **MAST, why multi-agent systems fail** (Cemri et al., arXiv 2503.13657): disobeying the task spec is 11.8% of
  failures, task derailment 7.4%, missing or incomplete verification 8.2%. Adding a task-objective verification
  step gave +15.6% on ChatDev.
- **The Complexity Trap** (JetBrains, arXiv 2508.21433): masking old tool output matches LLM summaries at about
  half the cost, and summaries made runs 13–15% longer because they hide failure signals.
- **Anthropic's long-running harness post** (2025-11): a JSON feature list whose items may only flip
  `passes`, because the model is "less likely to inappropriately change or overwrite JSON"; every session
  starts by reading the progress file and git log. Qualitative.

### 2.3 What this tells us

- **The measured fix is a registry outside the summary.** Pinned constraints, kept apart and re-attached
  verbatim, took violations from 30% back to 0%, and retention from 17% to over 90%. That is exactly what the
  ledger's rules and decisions become at the seam (§3.3), and why they are never cut for budget.
- **Read the user's turns, not the model's notes.** The best measured extractor read only the user's
  messages. Our ledger relies on the thread writing it, so the bootstrap also carries the user's own messages
  verbatim (§3.3).
- **Nobody checks provenance.** No product marks which plan line came from the user and which was added by a
  writer, and none lists decisions a derived document leaves out. Only Kiro (requirements to tests) and Copilot
  (citations to code) check work against a record at all. That gap is our largest class, (f) and (g), and the
  part of the design nobody else has (§3.5, §3.6).
- **Nobody gives open items a moment.** Orca's question gates and bb's "ask the user for the missing decision"
  come closest. Neither re-raises a question when the user comes back.
- **Restating helps, but rote acknowledgement isn't measured.** Orca's "state where it stopped" and Cline's
  "MUST read ALL" have no numbers. Laban's recap does. So the take-over line (§3.4) is small, and it stays only
  if the battery shows it helps.

## 3. The design

It keeps the plan's frame: the ledger is the record (Q3), Brigadier enforces nothing (G18), the seam nudge is
the only push (G12), the engine plumbs and the model judges. Every part is either an instruction, a cheap
`brigadier` command, or a battery item. Each part names the classes it answers.

### 3.1 Every entry has an id, a stamp and an owner (answers e, g, h)

- `brigadier ledger add` prefixes each entry with an id and a stamp it writes itself:
  `L14 · 10-23 11:40 · gen 2 ·`. Ids count up per ledger (a worker has its own) and never repeat. The model never types a time.
- **Rules and decisions quote the user.** A rule from the user is their exact words. A decision the user made
  is their words *and* the words they answered: for "A" or "sure", the option text itself. The entry keeps the
  sentence that defines the thing ("Sources = what this thread looked at from outside the repo"), not only
  the examples.
- **Each decision names its owner:** "user decided", "I decided" or "worker claims". "I decided" on something
  the user asked for is a change to it, and the next reply says so (§3.6).
- Entries written with file tools still count; they just have no id. `brigadier ledger check` (§3.5) lists
  them as "no id".

### 3.2 One list of open items, with a moment to raise them (answers c, d)

- One kind, **Open**, covers a promise, a question to the user and a "waiting on you". It is closed by
  appending `brigadier ledger close L12 "<how>"`; nothing is deleted.
- `brigadier ledger show --open` lists the open ones; `--rules` lists rules and decisions. Both read the one
  ledger file: no new files.
- **The moments:** the open list is the first thing in a successor's bootstrap (§3.3); and when the user
  writes after a gap, the thread raises the oldest open question first, one at a time, with how many more are
  waiting. That is guidance in the seam text and the `/brigade` skill, not an engine push.

### 3.3 The seam carries decisions verbatim; the note carries only what changed (answers b, e)

- The bootstrap order becomes: the map; **open items, rules and decisions, verbatim and complete, never cut
  for budget**; then the rest of the ledger newest-first within the budget, with the exact read command for
  what was left out; then the note; then **every user message of the lineage, verbatim**, within about 8k
  tokens (the first and latest always, a read command for the rest); then the latest exchange and the tail
  (L2).
- **Why the user's own messages.** The ledger depends on the thread writing it. The user's turns are the
  source of every rule, they are short, and copying them is plumbing, not judgement. Both 2026 compaction
  studies got their constraints back by keeping them outside the summary, and one did it by reading only the
  user's turns (§2.2). This covers class (a): a rule the thread never wrote down.
- **The note never restates decisions.** They travel by machine, verbatim. The note says only what the ledger
  can't: doing now, waiting for, next, "read A, B, C; next D, E", and which entries went stale. A summary of a
  summary is where Sources lost its rule.
- **Every path in a note or an entry is absolute.** A relative `research/…` is how this task got the wrong
  folder.

### 3.4 Check before and after the seam (answers c)

- **Before the note (in the seam nudge):** run `brigadier ledger show --open --rules`. Close or carry each open
  item by id. For each rule, check that your last steps kept it; write any slip as a Failed check.
- **After the seam (in the bootstrap):** the successor's first ledger entry names what it carries and what
  bears on its next step: `Took over gen 3 · open L12 L20 · applies next: L5 (tests stay on Linux), L9 (no
  new setting)`. This is not a line-by-line acknowledgement: forty rote "ok" lines cost tokens and prove
  nothing. Picking the few that bear on the next step, with how, is the check.
- The battery measures whether this line helps (§3.7). If it doesn't, it goes.

### 3.5 Derived documents cite their source (answers f, b)

This is the main fix for the largest class. It applies whenever a session turns decisions into a document:
a plan, a worker brief, a report, a commit message that claims "as decided".

- **Every line that comes from a decision cites its id.** A line with no id is the writer's own and says
  "proposed". A gap the decisions don't cover becomes a question to the user (`brigadier ask`), never a silent
  fill. This is the line that would have stopped "the ledger shows in Sources".
- **`brigadier ledger check <file>`** is plumbing, not judgement. It lists the rules and decisions the file
  never cites, ids it cites that are closed or superseded, and entries with no id. The thread reads the list
  and judges; nothing is refused.
- **Worker briefs carry the rules that apply, verbatim, with ids.** A worker is a separate CLI; it sees only
  its brief. The brief's own line count stays small: only the rules that bear on the job.
- **A reviewer of a derived document gets the source too.** A plan review is given the ledger (or the
  decision list) and asked for three things: missing, changed, added. Without the source, a reviewer can only
  check the document against itself (H12).

### 3.6 Decisions taken for the user are shown, once (answers g)

- In a Brigadier session the user is in the tab, so the thread says it in its reply: "I decided L18: … (you
  asked for …)". The entry gets "told". No card, no engine push.
- For the Delegator, where the user is away, this is a digest when the user comes back (§5).

### 3.7 Measured: decisions survive hand-offs (battery items, S21/S25)

A **decision-survival** block is added to the continuity battery. It reuses the battery's seams and adds what
§1.4 found missing.

- **Plant ten items** before the seams: three user rules (one later superseded by a correction), one
  principle with its defining sentence and two examples, two user decisions with reasons, one "I decided" by
  the thread, one open question to the user, one open promise, one false worker claim.
- **Three forced seams:** two by size, one death with no note (as today).
- **Then, without restating anything:**
  - **T1 applied unprompted:** a coding request where two rules apply (one in its superseded form). Scored
    from the diff.
  - **T2 the principle:** a new case only the principle decides (as the ledger-in-Sources case). Pass = the
    right answer, or a question to the user; fail = a silent wrong fill.
  - **T3 derived brief:** "write the brief for a worker to build X". Scored for decisions cited, missing,
    contradicted, and additions not marked "proposed". A fixed rubric, read by a model judge; it's a report.
  - **T4 open items:** a "hi, where are we?" message. Pass = the open question and the promise are raised.
  - **T5 told:** the thread's own decision was told to the user and marked.
  - **T6 recall when asked:** today's items, unchanged.
- **Bars** (report only, never a gate; plan §5):
  - after three seams, ≥ 9 of 10 planted items recalled when asked (today's B3) **and** ≥ 9 of 10 applied
    unprompted where they apply (T1, T2);
  - the derived brief: 0 contradictions, 0 unmarked additions, ≤ 1 decision missing (T3);
  - both open items raised (T4); the own decision told (T5).
- **Comparison arm:** the same script in the plain CLI with its own compaction. That is the bar every rival
  inherits.
- The first run is the baseline. With a 60k debug seam this costs about one battery run (≈ $3 today).

### 3.8 What this deliberately does not do

- **No acknowledgement of every line.** It costs a line per decision per seam and turns into rote. The "applies
  next" line (§3.4) does the job, if the battery says so.
- **No second file.** One ledger, filtered by kind. A session's record stays `record/ledger.md`, already
  indexed (S18) and kept by Archive (S20).
- **No engine contradiction check.** Judging whether a plan line contradicts a decision is the model's job;
  `ledger check` only finds what isn't cited.
- **No push of the decisions every turn.** The rivals' memory plugins inject a catalog each turn; G1 and G17
  rule that out, and the standing file stays under 2,000 bytes.

## 4. Amendments to docs/STRIP-DOWN-PLAN.md

Applied in the same commit as this document. Plain words, the plan's style. Exact text:

**Status header (new paragraph, after the G23 line)**

````text
> Amended again on 10-23 with `research/CONTINUITY.md`: decisions survive hand-offs (S22b, §6, §9.5 L8–L9).
> From this amendment on, a plan line that comes from a user decision cites it (a G/Q id, or a ledger id), and a
> line the writer adds says "proposed". Older text is not re-tagged. The known drifts are in the run's audit
> (`workers/w95/report.md`, run `20261022-0805-take-over-5-sessions`) and wait on the user (§9.6).
````

**§1, Long sessions, under the Ledger bullet (added)**

````text
  - `brigadier ledger add` gives each entry an id and the time. Rules and decisions quote the user. Open items
    (promises, questions waiting on the user) are one kind, closed by id. At every seam they travel first and
    whole (S22b, `research/CONTINUITY.md`).
````

**S22, Successor (one line added after the bootstrap list)**

````text
  - S22b then changes this order and the seam text (`research/CONTINUITY.md`); S22 lands with the text above.
````

**S22b (new step, before S23)**

````text
**S22b. Decisions survive the seam (`research/CONTINUITY.md`).** Opus. After S22; it may land with S23.
Proposed (10-23): the user asked for a fix to forgotten decisions; the details below are the research's design,
not the user's decisions. Why: on 10-22/23, 33 decisions and instructions were lost or changed between the
user and the work. Most were not forgotten at a hand-off. They lost who decided them, lost their defining
sentence in a summary, or waited with no moment to raise them (CONTINUITY §1).
- **Scope:**
  - `brigadier ledger add` writes `L<n> · <MM-DD HH:MM> · gen <g> ·` before the text. Ids count up per ledger
    and are never reused. The model never types the time.
  - `brigadier ledger close <id> "<how>"` appends a closing entry. Nothing is rewritten.
  - `brigadier ledger show --open` lists open items that aren't closed; `--rules` lists rules and decisions.
    With no flag it shows everything, as today.
  - `brigadier ledger check <file>` lists the rule and decision ids the file never cites, the ids it cites that
    are closed or superseded, and the ledger's entries with no id. Text out, `--json` too. It judges nothing
    and refuses nothing; the thread reads the list and decides.
  - **Bootstrap order** (replaces S22's list): the map; open items, rules and decisions, verbatim and whole,
    never cut for budget; the rest of the ledger, newest first, within the budget, with the read command for
    what it left out; the note; **every user message of the lineage, verbatim**, within about 8k tokens (the
    first and the latest always; the read command for the rest); then L2's latest exchange and tail. The user's
    own messages catch a rule the thread never wrote down: a 2026 compaction study kept over 90% of user
    constraints by reading only the user's turns (CONTINUITY §2).
  - The §6 standing text and seam text as amended on 10-23.
  - **The `/brigade` skill** gains four lines:
    - a worker's brief carries the rules that bear on its job, verbatim, with their ids;
    - a plan or brief built from decisions cites their ids, lines of the writer's own say "proposed", and a gap
      goes to `brigadier ask`, never a silent fill;
    - before handing a derived document on, run `brigadier ledger check` on it, and give its reviewer the
      ledger with the question "what is missing, changed or added?";
    - when the user writes after a gap, raise the oldest open question first and say how many more wait.
  - **The battery's decision-survival block** (`tools/continuity-battery/`, CONTINUITY §3.7): ten planted items
    (rules, one later corrected, a principle with its defining sentence, decisions, one "I decided", open
    items, a false worker claim), three forced seams, then six checks: applied without being asked, the
    principle on a new case, a derived brief, open items raised, own decision told, recall when asked.
- **Files:** `M/cli_ledger.rs`, `crates/cli/src/args.rs` and the ledger schema, S22's bootstrap module,
  `M/standing_instructions.md`, the `/brigade` skill file, `tools/continuity-battery/`.
- **Done when:**
  - Unit tests: ids count up and survive a move to `ledger-archive.md`; `close` appends; `show --open` leaves
    out closed items; `check` on a fixture plan lists exactly its uncited, closed and no-id entries.
  - A bootstrap test: with a ledger over the budget, every open item, rule and decision is in the bootstrap
    whole, and the cut part has a read command that works.
  - S17's byte test still passes (≤ 2,000 bytes, ≤ 15 lines); the provider test asserts the new text.
  - One battery run with the decision-survival block (Claude, 60k debug seam) exists as the baseline. It is a
    report, never a gate.
````

**S23, after its done-when (added)**

````text
- If S22b has landed, the seam-versus-rebirth comparison includes its decision-survival block, with the same
  planted items on both arms.
````

**S25, first scope line (extended)**

````text
  - The battery runs nightly on both providers, as a CI artifact and a Reports page. The decision-survival block
    (S22b) runs with it, next to a plain-CLI arm that compacts on its own (L9).
````

**Order view (changed lines)**

````text
after S18 + S21:  S22 → S22b → S23 → S24 → S25 → S26 → S27
…  L1–L9 with S18–S27 · …
````

**§6, intro paragraph (sentence added)**

````text
The entry ids, the Decision and Open kinds, absolute paths and the
"cites entry ids" line were added on 10-23, proposed (S22b, `research/CONTINUITY.md` §3.1–§3.6).
````

**§6, standing instructions (the whole block, replacing the old one; 13 lines, 1,826 bytes)**

````text
This is a plain Claude Code / Codex session inside Brigadier. Work as you would anywhere: run checks, reviews, git and gh yourself.
Your session folder is $BRIGADIER_SESSION_DIR. Scratch files go in $TMPDIR (inside it), never in the repo.
Keep a ledger at $BRIGADIER_LEDGER as you work. `brigadier ledger add` works in every role except chats beside a tab and stamps each entry with an id and the time; `brigadier ledger show` reads it back. Write an entry the moment something is settled, in the same step as your other tool calls.
Each entry says its kind and its source (which message, command or file). Kinds:
- Rule from the user: their exact words, which message, scope (this request / session / project); add "supersedes …", never delete.
- Decision: the user's words and what they answered (the option's text, not just "A"), or yours marked "I decided"; keep the sentence that defines it, and the reason.
- Exact identifier (hash, absolute path:line, number with units, id) · Failed check (command, exact error line, fixed or not) · Ruled out (what, why).
- Open: a promise, or a question waiting on the user; `brigadier ledger close <id>` when it is done.
- Learned from reading: file:line and a one-line finding, never a copy.
Never delete entries; move finished ones to ledger-archive.md beside it. A worker's claim is not a fact until you check it; say whose it is.
A plan or brief built from the ledger cites entry ids; mark lines of your own "proposed". Tell the user each "I decided" that changes what they asked.
For big reading, use your own subagents and keep only conclusions with file:line. Read exact ranges. Brigadier's commands: `brigadier --help`; ask the user with `brigadier ask`.
You may run workers of your own through `brigadier start` when the job calls for them; the brigade skill says how.
````

**§6, seam text (its note and the whole block, replacing the old one)**

````text
**Seam text (not standing).** S22 sends the first part as the seam nudge; the successor's bootstrap carries the
part after `---`. The check, take-over and "raise the oldest question" lines are S22b's (`research/CONTINUITY.md`
§3.3–§3.4).

```
A fresh session will continue this work. Finish the step you are on, at a boundary you choose; don't start new work.
Update the ledger. Run `brigadier ledger show --open --rules`: close or carry each open item by id, and check your last steps kept each rule; write any slip as a Failed check.
Then write the hand-off note at <path>: doing now, waiting for, next, "read A,B,C; next D,E", stale ledger entries. Don't restate rules or decisions; they travel verbatim. Absolute paths only. Then stop.
If this is your second hand-off in a row that is still reading, say so in the note and propose splitting the work.
---
You continue an earlier session. The open items, rules and decisions below are verbatim and authoritative; so are the note and last exchanges.
First add one ledger entry: "Took over gen <n> · open <ids> · applies next: <id> (how), …". If a question waits on the user, raise the oldest one in this reply.
For anything else from before, search the record (`brigadier brain search`, `brigadier brain read`) and quote what you find; if it isn't there, say so rather than guess.
```
````

**§8, item 16 (new)**

````text
16. **Decisions survive only as well as the model keeps them (S22b).** Ids come from `brigadier ledger add`;
    entries written with file tools have none, and `ledger check` lists them. The take-over line costs about
    100 tokens per seam: if the battery shows no gain in "applied without being asked", it goes. The derived-brief
    check is a model reading a fixed rubric, so it is a report, not proof.
````

**§9.5, Our bar (bullet added)**

````text
  - **Decisions survive** (proposed, CONTINUITY §3.7): after three seams, ≥ 9 of 10 planted items applied without being
    asked where they apply; a brief derived from them has 0 contradictions and 0 additions not marked
    "proposed"; every open item is raised again. The plain CLI with its own compaction is the comparison.
````

**§9.5, Steps (two added)**

````text
  8. **L8** (= S22b, proposed): entry ids and the time from `brigadier ledger add`, `ledger close|show --open|check`, the
     bootstrap that carries open items, rules and decisions whole, the seam's check and take-over lines, four
     `/brigade` lines, and the battery's decision-survival block. Done: S22b's done-when. Opus, S–M.
  9. **L9** (into S25, proposed): the decision-survival block nightly on both providers, next to a plain-CLI arm that
     compacts. Done when the nightly artifact shows the "decisions survive" bar per provider. Codex, S.
````

**§9.6, Waiting on you (two bullets added at the top)**

````text
- The 20 drifts between the 10-22 grill and the plan or the build (run `20261022-0805-take-over-5-sessions`,
  `workers/w95/report.md`). Each needs your call. Until then the plan's text stands; the 10-23 continuity
  amendment changes none of them.
- After a seam, should the successor carry on by itself, as a Delegator does, or wait for your next message,
  as S22 does by default (w95, item 9)? `BRIGADIER_SEAM_CARRY_ON=1` already does the first. S22b works either
  way.
````

## 5. Delegator skill changes (proposed only)

The user owns `~/.claude/skills/delegator`. Nothing there is changed by this run. Each change names the
classes it answers.

Ordered by what they fix for the least work. 6 and 7 are mechanical (`dlg` only); 1–5 change how the Delegator
writes; 8–10 change when it speaks.

1. **Three files, not one ledger (d).** `<run>/decisions.md` holds decisions only, append-only:
   `D12 · 10-22 08:53 · user · "<their words>" · answers: "<the option text>" · scope`. The Delegator's own are
   `by delegator · not yet shown`, until the user sees them. `<run>/open.md` holds questions to the user,
   "waiting on you" items and promises, each with "asked <time>" and a closing line. `ledger.md` keeps the
   goal, start commits, workers and the Timeline. Today the ledger has three "Waiting on you" sections and
   12 of 17 decisions sit in the Timeline. SKILL.md §1 step 4 changes.
2. **A live answer is written in the same turn, quoted (a, e, g).** For "A", "sure" or "as u recommend", copy
   the option's text verbatim next to the answer. Keep the sentence that defines the thing, not only its
   examples. A memory note gets the gist and a pointer, "quotes: `<run>/decisions.md` D1–D40", never the gist
   alone. A pushback the user hasn't seen is never written as accepted (w95-3).
3. **Briefs that turn decisions into documents (f, b).** Their input is `decisions.md`, not the memory note.
   Every line cites a `D` id or says "proposed"; a gap is listed as a question. The Codex plan review gets
   `decisions.md` too and three questions: what is missing, changed, added? SKILL.md §5 "Outlines and plans".
4. **Coverage before a build starts (d).** Every `D` id appears in some step's scope or done-when, or is
   marked "no step needed". That would have caught w95-4, w95-8 and w95-14. A `dlg check-doc <file>` that lists
   the ids a file never cites is the plumbing, the same as `brigadier ledger check`.
5. **Rebirth reads the files; the note holds only what's in flight (b, e).** §6 Recovery reads `decisions.md`
   and `open.md` whole. `delegator-N-handoff.md` carries no rules and no decisions, only in-flight state, and
   absolute paths only (H1). `dlg rebirth` passes the three paths to `/delegator resume`.
6. **One owner (h).** `dlg rebirth` writes `<run>/owner` with the new session's id. A `dlg` command from any other
   session refuses: "this run was handed to <id> at <time>; stop". Delegator 5 acted 37 minutes after handing
   over (H8).
7. **The tool writes times and carries text (h).** `dlg log "<text>"` adds the Timeline line with the real local
   time (H9). `dlg send` takes the message from a file or stdin, so the shell can't eat backticks (H10).
8. **When the user is back (c, g).** When the user writes after 30 minutes or more away, the first reply opens
   with what was decided on their behalf since then (one line each, with ids) and the oldest open question,
   one at a time, with "N more wait". Each item gets "shown <time>". On 10-23 the user was live from 11:10 and
   four open items waited through it (H3, H4).
9. **A "needs the user" that touches a settled decision waits (g).** This makes the existing rule concrete
   (never approve "changes to settled plan decisions"): before deciding a worker's "needs the user" item,
   search `decisions.md` for its area. If a user decision covers it, it goes to `open.md`, not into a
   decision. w95-7, w95-18 and w95-20 would have waited for the user.
10. **A replay test for the skill (all).** A synthetic grill (ten decisions, one principle with examples), a
    plan-writer brief and a Codex review, run once after the change. The derived plan is scored for missing,
    changed and added lines, as the battery's T3.

## 6. What the user must decide

1. **w95's 20 drifts.** Each one needs your call (`workers/w95/report.md`). Items 1, 2 and 8 are live in shipped
   code; 9–11 are in steps not built yet (S22, S24). Nothing here changes them.
2. **After a seam: carry on, or wait for you?** By default S22's successor starts on your next message;
   `BRIGADIER_SEAM_CARRY_ON=1` (w92, 10-23) makes it carry on by itself, as a Delegator does. The default was
   never asked (w95-9). S22b works either way. My recommendation: carry on by
   itself when the predecessor was mid-task, and wait when it had finished and was waiting for you.
3. **The Delegator skill changes (§5).** Take all, some or none. My recommendation: 6 and 7 now (mechanical,
   `dlg` only), 1–5 and 8–9 before the next long run, 10 after them.
4. **Re-check the unbuilt steps against the grill?** Old plan text is not re-tagged. The steps not yet built
   (S22–S27 and §9) hold three of the "added by a writer" items. My recommendation: one docs worker tags each of
   their lines with its source or "proposed", from w95's transcript extracts, before S24 starts.

S22b itself is proposed, not decided: the plan marks it so. If you don't want it, it is one step to drop.
S23–S27 work without it; S25 then runs the battery without the decision-survival block.
