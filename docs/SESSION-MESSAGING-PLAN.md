# Session messaging plan: sessions that talk to each other

> Status: plan, 2026-10-13. Base: main `3c9e9ec5`. Implementation is a later run.
> Paths: `M/` = `crates/core/src/manager/`, `P/` = `crates/providers/src/`, `D/` = `apps/desktop/src/`.
> File:line references are to `3c9e9ec5`.

## 1. What it is

A session's thread can send a short text message to another session, in the same project or in another project. The
receiving session hears it as input from a peer (not from its user), does the work, and can answer with a reply. A reply
comes back to the session that asked, on its own, with no polling.

Both conversations show every message:
- the sender shows a quiet row saying who it messaged;
- the receiver shows an attributed message block saying who sent it.

Each side opens the other at the exact place.

**In scope:** project sessions, between sessions of one project or of two projects. Claude and Codex threads, in any
mix.

**Not in scope:**
- Plain chats (outside projects). They have no thread tools today. They are a possible later step (§9).
- Workers. A worker still talks only to its own thread (`ask_orchestrator`, `submit_report`). The thread relays.

The user sends a message the same way they ask for anything: in words, to a thread ("tell the API session the
schema changed"). There is no new button, menu or setting.

## 2. What exists to build on

| Need | Today | Where |
|---|---|---|
| Wake an idle or sleeping thread | `deliver` / `queue_envelope` push an `Envelope` into the in-memory inbox and `kick` the thread. A hibernated session is made Active first. | `M/conversation.rs:1409-1477`, `:706-711`, `:1539` |
| Steer a busy thread | `join_working`, then the provider's `steer`. Claude writes to stdin; Codex uses `turn/steer`, else `turn/start`. | `M/conversation.rs:924`, `P/lib.rs:156-188`, `P/codex/mod.rs:1821-1860` |
| Resume a stopped CLI | `ensure_cli` with `Origin::Resume { native_id }` | `M/conversation.rs:1884`, `:2065` |
| Tools with a known caller | MCP grant → `Role::Orchestrator { conversation_id }`, `tools_for(role)`, `orchestrator_call` | `core/src/tools.rs:28-133`, `crates/mcp-server/src/catalog.rs:331-398`, `M/tools.rs:25` |
| Durable records | Event-sourced SQLite (`events`), `DomainEvent`, folded by `board.rs` into `ConversationView` | `crates/store/src/schema.rs:9-43`, `core/src/model.rs:1729` |
| A sent row | `OrchestratorStepKind::Messaged` + `TeamStep` ("Messaged ‹worker›"). It needs a task id and cuts the text to 2,000 bytes, so it is only a model, not reused. | `core/src/work.rs:1617`, `D/app/conversation/activity/TeamSentence.tsx:152-175` |
| An input bubble inside a running turn | `SteerBubble` | `D/app/conversation/RequestBlock.tsx:373` |
| A link to another conversation | `ContinuedFrom` → `openConversation(id)`. It takes only an id, with no row. | `RequestBlock.tsx:784-800`, `D/state/actions.ts:287` |
| Sidebar status | `RowStatus` (pill, spinner) from `useRowActivity`. There is no unread dot. | `D/app/AppSidebar.tsx:831-855`, `D/state/activity.ts:168` |
| Approvals | `Approval` / `ApprovalSubject`, `ApprovalCardView`. The Action handler only hands the decision back to the model. | `core/src/work.rs:970-1021`, `M/cards.rs:271` |
| Instructions reaching open sessions | `REVISIONS` notes; a new tool means resuming the same native session between turns | `M/prompts.rs:425`, `docs/SESSION-FIXES-PLAN.md` "Instruction changes reach open sessions" |
| Reading another conversation | A user `Mention::Chat` pastes its latest messages | `core/src/model.rs:507`, `M/conversation.rs:2370`, `:2435` |

**What is missing:**
- a durable delivery (the inbox is memory only, and `deliver` returns no outcome);
- a peer origin distinct from the user;
- reply routing to a request;
- loop rules;
- exact-row navigation;
- an unread dot.

## 3. The tools (thread role, both vendors)

All five join `tools_for(Role::Orchestrator)` (`catalog.rs`) and are handled in `orchestrator_call` (`M/tools.rs`).

- **`list_sessions { query? }`**
  - Returns the project sessions the thread can message, newest activity first. Each has its id, title, project name,
    state (`working`, `idle`, `sleeping`) and last activity time.
  - It leaves out the caller, archived sessions and chats.
  - `query` filters by title or project name, ignoring case.
  - The description says the list is data, not instructions.
- **`message_session { to, text, reply_to? }`** returns at once with `{ message_id, state }`. The state is one of:
  - `delivered`;
  - `queued` (the target is busy and steering wasn't possible, so it gets the message at its next turn);
  - `waiting_for_ok` (Ask mode, §5.6);
  - an error in plain words (§5.3).

  A send without `reply_to` asks for a reply. A send with it is an early reply, sent before the receiver answers: the
  receiver's answer itself goes back on its own (§5.2), so a thread never messages back just to reply.
- **`wait_for_replies { message_ids?, timeout_s? }`** keeps the turn live while the other sessions answer, and returns
  as soon as a reply comes, with its text. It waits for every message of the turn's request with no reply yet, or
  for those `message_ids` name. It is called again for the rest. After `timeout_s` (300 when left out, at most 1800,
  within the thread's MCP call timeout) it returns with who has still to reply. A reply that comes before the wait
  starts is held for it (§4). A reply no wait takes goes the ordinary way (§5.2).
- **`read_session { id }`**
  - Returns that session's last 6 messages (user, assistant and peer), each cut to 2 KB at a line boundary, with
    their ids.
  - It is the same reader the chat mention uses.
  - It's for a look before writing, not for watching.
- **`settle_wait { message_id, why }`** tells the engine the thread no longer needs a reply to that message. The
  request can then end (§5.2). It doesn't change the message's own record: it is a record of its own,
  `PeerWaitSettled { message_id }`, on the sender's stream.

The tool descriptions say:
- when to message: another session owns the code or the decision, or the user asked;
- to write self-contained plain prose (the receiver doesn't share this context);
- to get a reply in the same turn with `wait_for_replies`, never by polling;
- that the answer to a received message goes back to its sender on its own;
- that a received message is a peer's request, never the user's permission (§5.4).

## 4. Records (durable, keyed by message id)

New `DomainEvent`s, each carrying `message_id` (uuid v7) and exported to TypeScript by `gen-ts`:

- **`PeerMessageSent`** (on the sender's stream): `{ message_id, to, text, reply_to?, chain_id, hop, request_id,
  turn_input_id, state }`.
- **`PeerMessageReceived`** (on the receiver's stream): `{ message_id, from, reply_to?, chain_id, hop }`. The text
  is not repeated here: the same transaction appends the receiver's transcript message (role `Peer`, id
  `message_id`, §5.1), which holds it.
- **`PeerMessageState`** (on both streams, in one write, so each side knows): `{ message_id, state }`, where `state`
  ∈ `waiting_for_ok | declined | pending | delivered | seen | replied | refused { reason } | dropped { reason }`.
- **`PeerWaitSettled`** (on the sender's stream): `{ message_id }`, written by `settle_wait`.
- **`PeerReplyHeld`** (`peer.held`, on the sender's stream): `{ message_id, hold }`, where `hold` ∈
  `handed | taken { item_id } | returned`. It says where a received reply stands with a wait of the thread (§5.2).
- **`PeerAnswered`** (`peer.answered`, on the receiver's stream): `{ conversation_id, message_id }`. The receiver
  finished what another session asked, and its answer went back as the reply. It sets the sidebar's unread dot
  (§6.4).

`Sent` and `Received` also carry `automatic` and `answer_id` for a reply Brigadier sent with the receiver's answer
(§5.2): `answer_id` is the answer message it carries, so a jump to that reply lands on the answer (§6.3).

Rules:
- **Full text is stored once per side**, with no cut: in the sender's `Sent` and in the receiver's transcript
  message. A message over 64 KB goes to the blob store and the record holds its ref.
- **Each send's authorized time is stored** on its `Sent` record (`sent_at_ms`): the moment it was allowed to go,
  which for an Ask send is the user's Send, not the card's creation. The pair rate (§5.5) counts these.
- **Writing a send is one store transaction.** `Sent`, plus `Received` with the state `pending`, are committed to both
  streams together *before* anything enters the inbox. Nothing is ever shown without also being durably due.
- **`delivered`** is written when the envelope is in the receiver's inbox. **`seen`** is written when the receiver's CLI
  accepted it: a turn started with it, or a steer was acknowledged. **`replied`** is written on the original message
  when a reply that names it is committed.
- **Recovery at startup.**
  - Every `Received` whose last state is `pending` or `delivered` is put back in its session's inbox by message id, and
    the session is kicked.
  - Before redelivering a message whose CLI may already have taken it, the daemon looks for the message id in the
    receiver's native transcript. The id is in the envelope header, so this is a cheap text search of the Claude
    transcript or the Codex rollout, over its **input records only** (user input and steers). Tool results and
    assistant output never count: a receiver can read its own queued message with `read_session` before it is
    delivered. If it is found, the daemon records `seen` and does not send it again.
  - The inbox drops a second envelope with a message id it already holds.
- **Replies and waits survive a crash.**
  - A reply to a message the running turn sent is held for a wait. A wait hands it over as its result (`handed`).
    It becomes `seen` and `taken` only once the CLI shows that call finished, with `seen` recorded first.
  - What a turn ends without taking (no wait came, a Stop, a limit) is `returned`: it goes back to ordinary delivery.
  - At startup, a reply that is `taken` but still due is marked `seen`, and a `handed` one is `returned`. So a reply is
    never lost between a wait's result and the push, and never shown twice.
- **Each message is owed one reply.** The automatic reply is written in the same transaction that marks its message
  `replied`, so it goes once, across restarts. A message that already has a reply (the thread's own early one) gets
  none.
- **Archive and delete.** Archiving a session with pending messages for it marks them `dropped { archived }`. Deleting a
  session removes its streams as today. The other side keeps its own record and shows "Couldn't deliver: the session
  was deleted".

## 5. The engine rules

### 5.1 Peer provenance (an engine contract, not styling)
- A received message has the new role `MessageRole::Peer`, with `Message.peer` (`{ message_id, from, reply_to? }`)
  saying who sent it. The role stays a plain value, so the app's role checks keep working. It is never `User`.
  That holds everywhere:
  - the transcript;
  - request creation (`M/requests.rs`);
  - steering (`join_working`);
  - the queue (`QueuedMessage`);
  - recovery;
  - rebirth briefings.
- Every place that looks for "the user's words" matches `User` only, and does so explicitly. Today that includes:
  - merge consent (`M/merge_consent.rs`, `M/landing.rs:1930-2028`);
  - branch and edit lookups (`M/branches.rs:53,105,169`);
  - CI consent (`M/ci.rs:1352`);
  - the follow-up router.

  Phase 1 greps every `MessageRole::User` match and records each one's decision in the commit.
- **A peer message can't:**
  - give merge or CI consent;
  - answer a question card;
  - approve an outline;
  - count as the "human message" that resets the hop rule (§5.5).
- **The envelope** the receiving CLI sees:
  - one header line: `[message from session "‹title›" in ‹project› · session ‹session_id› · id ‹message_id›]`, plus
    `· reply to ‹id›` for a reply. The session id is what a reply's `to` names, so the receiver needn't look it up
    (as built: a live receiver without it guessed the message id as `to` and was refused);
  - then the text.

  The thread's instructions say what that header means (§3).

### 5.2 Requests and replies
- **Each send records the request it belongs to.** `request_id` is the sender's request whose turn made the call, from
  the turn's causing input (§5.5). It never falls back to "running or newest" (`M/requests.rs:46`).
- **A received message that is not a reply** opens a new request in the receiver, with `origin: Peer`, or joins the
  running one when it is steered in.
- **The sender waits for the reply inside its turn.** It calls `wait_for_replies` (§3), the turn stays live, and the
  reply comes back as that call's result. The thread then answers in the same turn. A reply never enters the inbox
  or starts a turn when a wait takes it.
- **The receiver's answer is the reply.** Each message another session sent that is not itself a reply is owed one
  reply. When every request that answers it is over (the one it opened, the one it was steered into, and those
  steered into them), Brigadier sends that turn's finished answer back as the reply (`automatic`, with `answer_id`).
  - An interim turn end, a `[quiet]` turn, or a worker, review or card still going keeps the request working, so the
    reply carries the finished answer.
  - Several messages steered into one turn each get their own reply.
  - A reply never triggers an automatic reply, so a chain of replies ends. The automatic reply is exempt from the hop
    refusal; at one per message, loops stay bounded.
  - A thread may still send an early reply with `reply_to`. Its message then gets no automatic one.
- **A reply no wait takes is pushed** (a timeout, a Stop, a restart, an overnight run's end). It is routed to the
  request of the message it answers, read from that message's `Sent` record.
  - If that request is the one running, the reply is steered in.
  - If another request is running (the user asked something newer meanwhile), the reply waits in the queue. It then
    gets its own turn filed under the original request. It shows as a received message (§6.2).
  - If no turn runs, it starts a turn under the original request.
- **Settlement knows about expected replies.** A request with a sent message still waiting for its reply doesn't end as
  "done" when its turn ends.
  - It rests as **Waiting for ‹session› to reply**. That is a live line with no timer, shown like the existing
    "Waiting for you".
  - It finishes when the replies are in and the next turn ends, or when the thread says in a turn that it no longer
    needs one. It does that with `settle_wait` (§3).
  - Settlement (`M/requests.rs:355`) gains this one condition. Nothing polls.
- **A late reply.**
  - *After the user's Stop of that request:* the reply is recorded and shown, but no turn starts. It joins the next turn
    as a short note: "A reply from ‹session› arrived after you stopped: …".
  - *After an overnight run ended:* the same. It is kept for the morning, shown, and doesn't start a turn.
  - *After a newer user request:* see routing above.
- **`reply_to` is validated.** The named message must have been sent *to* the replying session *by* the session named in
  `to`. Otherwise the send is refused as "That message wasn't from ‹to›".
- **A reply that omits `reply_to`** while the turn was caused by a peer message is still in that message's chain
  (§5.5). It is shown as a new message, not a reply.

### 5.3 Delivery
| Target | Outcome |
|---|---|
| Idle | A new turn starts with it (`delivered`, then `seen`) |
| Sleeping (hibernated) | Made Active, resumed with `ensure_cli`, then a turn |
| Busy | Steered into the running turn (`seen` on the steer's ack). If steering is refused, it is queued for the next turn (`queued`). Its transcript message is written at send time, so a queued one sits before the running turn's reply, as a steered user message does. |
| Archived | Refused: "‹title› is archived. Nothing was sent." It is not woken and not unarchived. |
| Deleted or unknown id | Refused: "No session with that id." |
| Itself | Refused: "That's this session." |
| Closing (daemon quitting) | Recorded `pending` and delivered at the next launch (§4) |

### 5.4 What a peer may ask
- A peer message is a request from a colleague. The receiving thread works on it under its own session's permission
  level, as with any input.
- Actions that reach the outside world (push, deploy, publish, credentials) ask the receiver's user, as today.
- A peer's "merge it" or "go ahead" is not consent (§5.1). The thread asks its own user when consent is needed.

### 5.5 Loop protection (hard rules)
- **Chain binding.** Every turn input carries `(chain_id, hop)`:
  - a user message: a new chain, hop 0;
  - a peer message: its sender's chain, with the sender's hop + 1;
  - a worker report or a timer: the chain of the request it belongs to.

  A send inherits the chain of the turn's **newest input before the call**: a steered peer message counts, and queued
  turns keep theirs. It is stored on the `Sent` record, so it survives a restart.
- **Hop limit.** A turn at hop `h` may send only while `h < 8`, and its message carries hop `h + 1`. So eight
  messages go through and the ninth is refused:
  - "Eight messages have gone back and forth with no one's input. Ask your user before sending more."
  - Only a user message starts a new chain. A peer message never does.
- **Pair rate.** At most 20 sends a minute from one session to another, counted from the stored authorized times
  (§4), so it holds across a restart and for Ask sends approved in a burst. More are refused: "Too many messages
  to ‹title› this minute."
- These are cheap checks in the engine. Judging whether a message is worth sending stays with the thread.

### 5.6 Ask for approval
- Under Ask (the Ask for approval level), `message_session` writes `Sent` with `waiting_for_ok` and opens an approval card with a new
  `ApprovalSubject::PeerMessage { message_id }`.
  - Card title: "Send a message to ‹title› in ‹project›?"
  - Body: the exact text.
  - Buttons: **Send** and **Don't send**.

  The tool result says "Waiting for your OK".
- **Send.** The engine checks the target again: it exists, isn't archived, and the hop and rate rules hold. It then
  writes `Received` and delivers the stored text exactly as approved. The model is not asked again. The sender
  hears "Sent to ‹title›" as a note.
- **Don't send.** The state becomes `declined`, and the sender hears "The user didn't send it."
- **A restart while waiting.** The card comes back, because it is a stored approval. Nothing has been delivered.
- **The target is archived or deleted before the OK.** The card's Send fails with that reason and the state becomes
  `refused { reason }`.
- Approve for me and Full access send without a card.

### 5.7 Instructions reach open sessions
- One `REVISIONS` entry (`M/prompts.rs:425`) carries the note: "You can now message other sessions: list_sessions,
  message_session, wait_for_replies, read_session, settle_wait. …"
- An open session's next turn closes its idle CLI and resumes the same native session, so the new tool list is read.
  Its context stays.
- `CONTRACT` stays unchanged.

## 6. Display

### 6.1 In the sender
The sender's request is one turn: it sends, waits, then answers.
- **One row in the work for each exchange**, built like the other action rows. Adjacent sends and waits share it.
  - It comes from the sent records (§4), placed at their position in the request's work. (As built: no
    `MessagedSession` step. A send refused before any record exists leaves an
    `OrchestratorStepKind::MessageRefused { to?, title?, reason }` step instead, its only trace.) A
    `wait_for_replies` call joins the row of the messages next to it.
  - Each session it names is a link that opens that session at the message. A deleted session is named but not
    linked. The row names only the title, never the project.
  - The row has no chevron and opens nothing. A failure's reason is its tooltip.
- **Live words**, the same row changing as the turn goes on:
  - "Sending message to ‹title›", with a speech-bubble icon and a shimmer on the words;
  - "Waiting for ‹title› to reply", with a clock icon and the shimmer, while the wait runs;
  - "Waiting for your OK to message ‹title›" under Ask, until Send.
- **Done:**
  - "Sent message to ‹title›, waited for its reply";
  - several: "Sent messages to A and B, waited for their replies", or "…, waited for a reply from A" when only one came;
  - a wait that took no reply: "…, waited; no reply yet". A wait with no message next to it: "Waited for a reply from
    ‹title›";
  - "Didn't send message to ‹title›" (the user's Don't send) and "Couldn't send message to ‹title›".
- **The reply a wait took isn't shown in the sender:** the turn's own answer says it. A turn whose only work is the
  exchange folds like any other: "Worked for Ns", with the answer below.
- **A request that ended with replies still due** rests "Waiting for ‹title› to reply · ‹time›" in its header and live
  line. A pushed reply then shows as a received message (§6.2).
- **A group's summary** uses "messaged a session" / "messaged sessions" among its kinds, e.g. "Read files and
  messaged a session".

### 6.2 In the receiver
- **The message stands in the input position of its turn, in the user's own bubble:** right-aligned, the same fill,
  corner radius and padding.
  - Above it is a caption: a speech-bubble icon and "Sent by ‹title› from another session" (13 px on a 19.5 px line,
    muted). It opens the sender at its sent row.
  - The text is rendered markdown, folded past 6 lines with Show more / Show less.
  - The time and Copy show on hover.
  - Screen readers hear "Message from ‹title›:".
- **A message steered into a busy turn** shows inside the running turn, like a steer bubble, with the same caption.
- **Its turn is ordinary.** The reply Brigadier sends with the answer adds no row: the answer is the reply.

### 6.3 Jumping to the exact row
- `openConversation(id, { at?: { message: message_id } })` (an automatic reply's id lands on the answer it carries) (`D/state/actions.ts:287`):
  1. selects the session and shows its chat (it may have been left on another tab);
  2. loads the history page that holds the message, if it isn't loaded;
  3. unfolds the work block around the row;
  4. scrolls it to the centre;
  5. flashes it once (a tint held 200 ms, then faded over 400 ms), once its folds have finished opening.
- Without `at`, it behaves as today.

### 6.4 Sidebar and notifications
- **An unread dot** shows on a session row that finished what another session asked (`PeerAnswered`) while it wasn't
  the open conversation. It clears when the session is opened.
  - This is the sidebar's first unread mark.
  - The "last opened" mark lives with the app's other remembered view state.
- The existing spinner and pills are unchanged.
- **The finish notification** (when the app is in the background) names the cause: "‹title› finished · after a message
  from ‹sender›".

## 7. Phases

Every phase's "done when" includes:
- `tools/full-checks.sh` passing on the integrated tree;
- one review by the other vendor;
- a fresh verifier;
- a `report.md` for that phase.

Instruction changes ship their `REVISIONS` entry in the same commit.

### Phase 1: the engine
Scope:
- the records and transaction (§4);
- `MessageRole::Peer` and the audit of every `User` match (§5.1);
- the three tools, for Claude and Codex;
- delivery and recovery;
- request routing and settlement, `settle_wait`, late replies;
- the chain, hop and rate rules;
- the Ask pending send;
- the revision note.

**Done when** flow tests in `M/flow/` pass. They run the real manager, store and git, with scripted CLIs calling the
tools, and cover:
- **Delivery:**
  - idle, sleeping, busy (steered) and steer-refused (queued) targets;
  - archived, unknown and self targets refused, with nothing woken.
- **Projects:** same project and across two projects.
- **Restart:**
  - a crash before the inbox enqueue;
  - a crash after enqueue but before the CLI took it;
  - a crash after the CLI took it.

  Each ends delivered exactly once, and the third is found in the transcript and not resent.
- **Replies:**
  - a reply steered into the waiting request;
  - a reply arriving after a newer user request (queued, then filed under the original);
  - a late reply after Stop and after an overnight end (shown, no turn);
  - a `reply_to` naming a message that wasn't from `to` (refused).
- **Chains:**
  - forwarding A → B → C keeps one chain and counts hops;
  - a reply that omits `reply_to` stays in the chain, across a restart;
  - the 8-hop refusal;
  - a user message in one session starts a new chain;
  - a peer message doesn't.
- **Rate:** the 21st send in a minute is refused.
- **Provenance:** a peer's "merge it" doesn't give merge consent, and the merge asks the receiver's user.
- **Ask:** Send, Don't send, a restart while waiting, and a target archived or deleted before the OK.
- **Settlement:** a request with an expected reply rests "Waiting for ‹title›" and ends after the reply's turn.
- **Revisions:** an open session at revision N−1 resumes at N with exactly the note and the new tools, for Claude and
  for Codex. A session at N hears nothing.

### Phase 2: the conversation and the sidebar
Scope:
- the sent row and its states;
- the received block and the reply caption;
- the steered variant;
- `openConversation(…, { at })` with page load, unfold, scroll and flash;
- the unread dot;
- the notification text.

**Done when:**
- Render tests (`*Render.test.ts`) and `blocks.test.ts` cover every state: waiting for OK, sending, delivered, seen,
  replied, failed, received, a reply to an earlier request, and steered.
- Fixture-gallery screenshots (1728 × 1024, DPR 2, dark) of each state, in Compact and Normal density and a narrow
  window, are in the report.
- A jump from the receiver to a sent row that sits in folded, not-yet-loaded old history lands on the row, unfolded
  and flashed. The same holds from the sender to a received message.
- The dot appears only for a session that is not open, and clears on open.

### Phase 3: live, end to end
**Done when** a dev build, under its own identity and scratch data dir and never the installed app, runs two real
sessions and shows each case working:
1. Claude → Codex in one project: the receiver is idle, the reply comes back, and both conversations show the row,
   block and reply.
2. Codex → Claude across two projects, with the receiver busy (steered) and then hibernated (resumed).
3. Ask mode: the card, then Send.
4. A jump each way.
5. The unread dot.
6. A daemon restart with a message pending.

Screenshots of each go in the report. Every throwaway session, project and data dir is removed afterwards.

## 8. Decisions

- **Sessions only.** Plain chats are out for now. The Delegator decided this on the user's behalf on 2026-10-13;
  the user didn't decide it.
- **Replies are waited for inside the turn, with push as the fallback.** The sender blocks in `wait_for_replies` and
  answers in the same turn; the receiver's finished answer goes back as the reply. A reply no wait takes is pushed,
  and the request rests "Waiting for ‹title› to reply" until it comes. Nothing polls. (Decided 2026-10-14, replacing
  the push-only design, so the sender reads as one turn.)
- **No new user control.** The user asks a thread in words. There is no Reply button on a received block, because the
  receiving thread answers on its own.
- **No settings.** The hop limit (8) and the pair rate (20 a minute) are fixed engine rules.

## 9. Later

- Plain chats as senders and receivers.
- A worker messaging another session directly, if relaying through the thread proves slow.
