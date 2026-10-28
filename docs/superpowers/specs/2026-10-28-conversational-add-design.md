# Adding an intent by conversation

Date: 2026-10-28. Tool: `tools/intents` (herdr-intents). Status: design, not built.

## Why

Today `a` (and the `prefix+a` popup) asks the kind, then a title, saves a raw intent, and opens a polish chat. You have to know the title before you have thought the thing through, and the polish then asks a batch of questions at once.

The owner wants capture to start from whatever they have: an observation, something someone reported, a suggestion, a half-formed idea. An agent should talk it through with them, a question at a time, and write the title and the intent at the end. The type should still be chosen first, so the agent knows what kind of conversation to have, but it can be changed before saving.

## The flow

Decided by the owner, shown in the mockups (`.superpowers/brainstorm/…/add-intent-flow.html`) and confirmed ("A", "looks good"):

1. **Pick the type.** `a` in the tree or list, and the `prefix+a` popup, ask: `b` bug, `r` refactor, `f` feature, `i` idea. The type steers the conversation:
   - bug: what happens, what you expected, where, who sees it;
   - refactor: which code, what hurts, what must not change;
   - feature: who, what they can do, where, what success looks like;
   - idea: what sparked it, who it is for, what would make it worth doing.
2. **Say it in your own words.** A free-text box: typos, fragments and pasted reports are fine.
3. **Polish first.** Before any question, the agent replies with a cleaned-up restatement (a one-line summary plus a short paragraph) and asks whether it heard right. Enter on an empty box accepts; typing a correction gets a new restatement.
4. **Up to 5 questions, one at a time.** Each question builds on the last answer. The status shows `question 2 of up to 5`. The agent may stop early when it has enough. The owner can skip a question or end the questions at any time.
5. **Write-up.** The agent writes the title and the full intent body (Description, Preconditions, Acceptance criteria, Open questions). The acceptance criteria are grouped by area group (owner, 2026-10-28): criteria for the group the intent is filed in come first, ungrouped, then one `### <area path>` sub-heading per other group the work touches. This is the same shape refinement and reconcile already produce. Below it, a `save as:` row shows the four types, with the chosen one selected and the agent's suggestion noted ("agent: this reads as a bug"). Tab changes the type; typing keeps refining; Enter saves; Esc discards.
6. **The home group.** Where the intent is filed is not always known when the conversation starts (owner, 2026-10-28). It is known when `a` is pressed on a group in the tree. It is not known in the card view, where the selected card is incidental, nor from the popup or an empty list. When it is not known, the agent proposes the home group along with the other areas, from the workspace's existing groups. If it cannot tell, it asks, and that counts as one of the 5 questions. The write-up shows `in: <home group>` and `also shows under: <areas>` above the save row. The owner corrects either by typing ("file it under admin/team-invites"). When it is known, the agent keeps it unless the owner says otherwise.
7. **Nothing is written until save.** Esc at any step discards the conversation; the owner accepted losing an unfinished one (no drafts).
8. **When the agent fails** (owner, 2026-10-28: "retry plus save raw with ctrl-s"). A turn that errors, is cancelled or times out, or a write-up with no proposal to save, keeps the conversation on screen with the failure named. The owner can:
   - press Enter to retry the same phase's prompt with the same agent;
   - press `m` to pick another agent and model, then retry;
   - press Ctrl-s to save their own words as a raw intent;
   - press Esc to discard.

   A raw save uses:
   - the first line of their words as the title;
   - the chosen type as the kind;
   - everything they typed as the Description, with their answers so far under Open questions;
   - state `raw`;
   - the home group if known, else where `a` would put it today.

   A later `p` polishes it as usual.

## Goals

- Start from the owner's own words, not a title.
- The agent restates before it questions, so misunderstandings surface at the first turn.
- At most 5 questions, asked one per turn, each about the happy path: who, what, where, what success looks like. The existing rule stands: no edge cases or failure modes before hand-off.
- The agent proposes the title.
- The type can be switched at save.
- Acceptance criteria come out grouped by area group.
- The agent proposes where the intent is filed when that is not known at the start.
- Refactor becomes a fourth type, end to end.

## Non-goals

- `p` on an existing intent keeps today's polish chat (a proposal plus a batch of questions, applied through the diff).
- No drafts and no resuming an abandoned conversation.
- The non-interactive capture (`herdr-intents add "title"` and piped text) stays a raw quick capture.
- No change to how intents are sent, reconciled or tracked once saved.

## Anti-goals (it fails even if it works)

- The agent asks several questions in one message, or asks a sixth.
- The agent asks about edge cases, error handling or failure modes.
- The restatement or the write-up loses the owner's wording or a decision they gave.
- An abandoned conversation leaves a file behind.
- An agent failure loses what the owner typed.
- A slow agent turn freezes the screen (turns already run on a background thread; keep it that way).

## Refactor, the new type

- Stored as `refactor` in front matter and shown as "refactor". `KINDS` gains it, so `K` cycles bug → refactor → feature → idea.
- The agent's first-line heading gains `refactor: <what changes> in <where>`, parsed like `bug:`, `feature:` and `idea:`.
- New default templates `refactor.md` and `refactor.polish.md`, seeded and upgraded like the others (`old_polish_defaults`).
- Hand-off: a built-in `refactor-spec` hand-off for kind `refactor` that uses the enhancement-spec skill to file a GitHub issue labelled `refactor`, showing the draft before posting. This matches how refactors are filed in skoolscout (enhancement-spec plus the `refactor` label). `grill` and `wayfinder` apply to every kind, refactor included.
- Progress, reconcile and the card view treat refactor like feature.

## What gets saved

On Enter at the write-up:

- A new intent file in the home group from the write-up's `in:` line. The agent's reply names it, and it must be an existing group inside the workspace's scope. If the reply names none, or names one that does not exist, the file goes where `a` would put it today: the group pressed on in the tree, else the top of the workspace's group. The write-up shows that fallback, so it is never a surprise.
- Title from the agent's first line, kind from the `save as:` row, body from the proposal block.
- The `### <area path>` headings that name existing groups become the intent's `areas:` (reuse `reconcile::heading_areas`), so it also shows as a `↳` row under each of them. A heading that names no existing group stays in the body but is not added.
- Skipped questions, and anything the agent still lists under `questions:`, go under `## Open questions`. Answered ones are kept with their `→ answer`, as polish does today.
- State is `refining`, so the `?N` mark and the Ready gate work as they do now (`R` is refused while a question is unanswered).
- The transcript is written to the intent's `<stem>.chat.md`, and the provider's conversation id to `conversations:`, so a later `p` resumes the same agent conversation.
- The new intent is selected. The popup closes after save or discard.

## Constraints

- The agent and model are the ones chosen with `m` (`last-used.toml`); nothing is asked per conversation.
- Every turn goes through the existing provider machinery (spawned CLI, streamed replies, resume by conversation id, re-send of the transcript when a provider cannot resume, cancel on Esc).
- The agent runs in the neutral polish directory under the root (`polish_cwd`), as polish does.
- Plain letters are always typed in the chat box; actions use Enter on an empty box or control keys.
- 80-column terminals and the 85% popup must fit the chat and the `save as:` row.
- Tests are TDD with the fake provider fixtures (`tests/fixtures/events/*.jsonl`) and insta snapshots, run with `</dev/null`.

## Left to the builder

- **How the app knows which phase a reply belongs to.** My call: the app drives the phases. It sends a phase-specific prompt each turn (restate, ask one question or reply `enough`, write up) and parses the reply for that phase. It does not infer the phase from free text.
- **The keys to skip a question and to end the questions.** My call: Enter on an empty box skips (in the restate phase it accepts), and Ctrl-d writes it up now. Ctrl-d is "dismiss" in today's chat, which this flow does not use. Show both in the status line.
- **What the agent sees in each prompt.** Each turn carries the type's interview template, the conversation so far, the remaining question budget, and the happy-path rule. Every turn after the restatement also carries the home group if known ("unknown" otherwise) and the paths of the existing groups in the workspace's tree, so the agent can choose real ones.
- **How the home group and areas come back in the reply.** My call: a `group: <path>` line after the title line names the home group, and the `### <area path>` headings name the rest. The app checks both against the existing groups.
- **How areas are chosen.** My call: the agent picks them from that list, from what the owner said. When it cannot tell where the intent lives or which areas the work touches, one combined question covers both, and it counts toward the 5. The owner can correct the grouping by typing at the write-up, and can still change areas later with `+`.
- **Where the agent runs before the intent has a name.** My call: the interview runs in its own folder under `.polish/`, named once at the start (e.g. `new-<random>`). That name is saved in the intent's front matter (`polish_dir:`), and `polish_cwd` uses it when present, else the stem as today. Claude only resumes a session from the folder it started in, and the stem does not exist until the title does, so this is what lets a later `p` resume the interview's conversation. Discarding deletes the folder. The provider's own session logs (`~/.claude/projects/…`, agy's store) are outside the tool and are left alone.
- **The wording of the new templates** and of the four type-specific conversation guides.
- **Where in the code the interview lives.** Probably a new screen state beside `Screen::Chat`, sharing its turn handling. `app.rs` is already 7,700 lines, so splitting the interview into its own module is welcome.
