# capabilities: list view, readiness and delivery progress

**Date:** 2026-10-25
**Status:** approved by the owner 2026-10-25.
**Owner:** Edwin Cruz (jefelabs)
**Home:** `jefelabs/herdr-plugins`, `tools/capabilities/` (crate and binary `herdr-capabilities`).
Builds on `2026-10-24-intents-design.md` (the tool's original design; the tool
was renamed from *intents* to *capabilities* on 2026-10-25).

## Intent and the why

The tree view is good for arranging capabilities and working on one at a time.
It is poor at answering the questions the owner asks when moving through all
of them: which still need answers from me, which are ready to hand off, and
how far along is each one that has already been handed off.

Today the answer to the last one lives outside the tool: in Herdr's sidebar,
in the repo's `docs/superpowers/` folders, in GitHub. This change brings those
answers into one full-screen list, makes "every question answered" the gate
for sending, and narrows what the refinement conversation asks about so that
the gate is reachable.

## Decisions made in the brainstorm

The owner's decisions, as decided.

1. **A second view, toggled with the tree.** A flat list of every capability,
   across all groups.
2. **The list replaces both columns.** While it shows, the tree and the detail
   panes are gone; the list is full width.
3. **Each row shows the capability's status, a timestamp, the title and the
   description.** Tabbing through rows must show at a glance which need
   questions answered, which are ready, and which are complete.
4. **Status before sending is its own thing.** A capability that has not been
   sent still needs a readable status (needs answers, ready).
5. **Sending requires every open question answered.** `s` works from the list
   and refuses while any question is unanswered.
6. **Open questions are happy-path details.** They are the answers an agent
   needs to steer implementation effort. Edge cases are not open questions:
   they surface after the capability is submitted to an agent.
7. **After sending, the row shows the agent's progress on delivering the
   feature**, not whether its terminal is busy.
8. **Progress is a bar until complete.** It fills through the delivery stages.
9. **Complete means every acceptance criterion is ticked.**
10. **Sort orders are selectable** ("various sort orders"; the specific set
    is a call below, marked as mine).

## Calls made in the brainstorm (Claude's, for the owner to check)

The owner did not decide these; each is marked so it can be overruled.

- **Sort orders:** Updated (default, newest first), Progress (least advanced
  first), Captured (newest created first), Title (A–Z). The last-used order
  is remembered. Progress order: `raw`, then `?N` (most open questions first),
  then `ready`, then sent rows by bar fill ascending, `✓ complete` last; ties
  by Updated. "Updated" means the front matter's `updated`, else the file's
  modification time (`store::stamp`).
- **Keys:** `v` toggles tree ↔ list; `S` cycles the sort order; `Enter` on a
  row returns to the tree view with that capability selected; `g` focuses the
  agent a capability was sent to (`herdr agent focus <name>`).
- **Description:** one line under the title, cut at the pane edge.
- **Timestamp follows the sort:** the created time under Captured, the updated
  time otherwise.
- **Group label:** shown dim at the right of the title line when the pane is at
  least 100 columns wide, omitted otherwise.
- **"Needs you" flag:** the live Herdr state is not the progress, but when the
  sent agent is `blocked` (waiting for the owner) the row says so, highlighted.
- **A raw capability cannot be sent** even though it has no open questions:
  at least one refinement pass, or an explicit `R`, comes first. `R` stays the
  manual way to declare a hand-written capability ready.
- **Progress weights and stage labels** as given under *Delivery progress*.
- **The view in use (tree or list) is remembered** between sessions.

## Goals

- One screen that shows every capability's readiness and delivery progress.
- A send gate that matches decision 5 everywhere `s` works.
- Refinement that asks happy-path questions, so the gate is reachable.
- Progress derived from evidence that already exists (files, the plan ledger,
  GitHub, Herdr), with no new manual bookkeeping for the owner.

## Non-goals

- Filtering or searching the list. Sorting only.
- Editing from the list. Editing, answering and reading artifacts stay in the
  tree view (`Enter` goes there).
- Watching the disk. The list refreshes on `r`, on returning from another
  screen, and on the timers below; it does not use file-system events.
- Tracking more than one agent per capability. `sent_to` holds one agent; a
  second send replaces it, as today.

## Anti-goals

- **The list must never block on a slow command.** `herdr`, `gh` and git run
  off the UI thread; a missing or slow tool degrades a column, never the view.
- **No progress claimed without evidence.** A stage shows as reached only when
  its signal exists. A hung agent must not look like progress.
- **No silent template rewrites.** The owner's seeded templates in the config
  directory are not edited by the tool.

## The list view

```
 Capabilities · 7 · sorted by Updated                          v tree · S sort · s send · g agent
 ──────────────────────────────────────────────────────────────────────────────────────────────
  ?2     Families can skip a folder without seeing Pending                         Oct 24 18:02
         When a parent uses Skip for now on the report-card folder, the folder still shows up…
  ready  Org admins can export candidate lists as CSV                              Oct 23 09:41
         Admins need a one-click export of the filtered candidates table for board meetings…
  sent   ██░░░░░░░░ speccing          ! needs you   Notify primary contact to schedule…  Oct 25 08:10
         The primary contact gets no prompt to book the intro meeting after acceptance…
  sent   █████▌░░░░ building 4/9                    Counselors can reassign a student…   Oct 22 16:15
  sent   ███████░░░ PR #5830 open                   Applicants see status summaries      Oct 21 14:02
  sent   ✓ complete                                 Invitees skip the join request       Oct 20 10:30
  raw    a                                                                         Oct 25 07:30
         (no description yet)
```

- **Rows:** status, then (for sent capabilities) the bar and stage label, then
  the title, then the timestamp right-aligned; the description's first line on
  the line below, dim.
- **Selection** carries across the toggle in both directions: the same
  capability is selected in either view. A group selected in the tree maps to
  the first capability under it, or the first row.
- **Keys in the list:** `j`/`k`/arrows move, `PgUp`/`PgDn` page, `v` back to
  the tree, `S` next sort order, `Enter` open in the tree, `s` send (gated),
  `g` go to the sent agent, `r` rescan, `q` quit. Other tree keys do nothing.
- **Empty states:** no capabilities at all → one dim line saying how to add one
  (`a` in the tree). No repo on a sent capability → the bar still shows what
  the file itself proves (sent, verified).

## Status before sending

Derived, not stored:

| Shows | When |
|---|---|
| `raw` | `state: raw` |
| `?N` | not sent, N > 0 unanswered open questions |
| `ready` | `state` is `refining` or `ready`, and no unanswered questions |
| `sent` | `state: sent` |

`R` keeps its current rule (refused while questions are open) and remains the
way to move a `raw` capability to `ready` by hand. `U` is unchanged.

## The send gate

`s` (in the tree and in the list) refuses, with a message naming the count,
when the capability is `raw` or has any unanswered open question. Every
handoff is gated alike. Sending an already-sent capability stays allowed
(it replaces `sent_to`, as today) as long as the gate holds.

## Open questions are happy-path only

Two changes to what the agents are told:

1. **During refinement.** The rule is added to `FORMAT_REMINDER` in
   `conversation.rs`, which is appended to every turn, including the seed, so
   it reaches existing setups whose seeded `*.polish.md` templates predate it.
   The bundled defaults get the same sentence for new setups. The sentence:
   *Open questions are only what you need to know to build the main flow:
   who, what, where, and what success looks like. Do not ask about edge cases
   or failure modes; those are handled after hand-off.*
2. **At hand-off.** A new preamble in `send.rs`, applied to every handoff:
   *The happy path is settled: every open question below has its answer. Edge
   cases are now yours to find. List the ones you see, ask about those that
   change what gets built, and record each decision as you go.*

## Delivery progress

Shown only once a capability is sent. The stages from Sent to Merged are
cumulative: the bar shows the furthest of them with evidence, and earlier ones
count as reached even when their own signal is missing (an agent that never
wrote a plan but opened a PR is past planning). **Verified is not part of that
rule**: ticked criteria fill only the Verified segment, by their share, and
never count as evidence for Building, PR open or Merged (a criterion ticked at
send time, or while building, must not jump the bar).

| Stage | Label while here | Evidence | Weight |
|---|---|---|---|
| Sent | `speccing`, or `charting · N decisions` | `state: sent` | 1 |
| Specced | `planning` | a `spec:` or `issue:` line under `## Artifacts` | 1 |
| Planned | `building 0/N` | a `plan:` line under `## Artifacts` | 1 |
| Building | `building k/N` | tasks finished in the linked plan's ledger | 3, filled by k/N |
| PR open | `PR #n open` | a `pr:` line under `## Artifacts`, state `OPEN` | 1 |
| Merged | `verifying j/M` | that PR's state `MERGED` | 1 |
| Verified | `verifying j/M` | acceptance criteria ticked, j of M | 2, filled by j/M |

Weights sum to 10, one bar cell each; partial cells may use eighth-block
characters. When every acceptance criterion is ticked the bar is replaced by
`✓ complete`, regardless of other stages, including before a merge
(decision 9). A sent capability with no acceptance criteria never shows
complete; its label says `no criteria` once it reaches Merged.

The owner ticks acceptance criteria (agreed in the brainstorm: "Verified is
ticked by a person"). No handoff prompt asks an agent to tick them.

### Several plans, several PRs, and handoffs that file instead of build

Calls made after the builder check (Claude's, for the owner to check):

- **Plans are summed.** A capability may link several `plan:` lines (the
  owner's live sent capability links four). N and k are summed over every
  linked plan and the label reads `building k/N` overall. A plan with no
  tasks and no checkboxes still counts as Planned and adds nothing to N. When
  N is 0 across all plans the label reads `building` and the Building segment
  stays empty until a PR appears. A plan that cannot be read makes the label
  `building ?` but does not hide the other plans' counts.
- **PRs.** Every `pr:` line is checked. A PR closed without merging is ignored.
  PR open is reached when at least one remaining PR is open or merged. Merged
  is reached when every remaining PR is merged. Labels: `PR #n open` for one,
  `PRs 1/3 merged` for several. One `gh` call per distinct PR per poll.
- **Issues count as specs.** `issue-report` and `enhancement-spec` end by
  filing an issue, recorded as an `issue:` line; in the skoolscout workflow
  that issue is the spec of record, so it is evidence for Specced. Work done
  later on that issue moves the bar the usual way, through `plan:` and `pr:`
  lines appended by whoever does it.
- **Maps in progress.** A sent capability with `decision:` lines but no spec
  or issue yet reads `charting · N decisions` while at the Sent stage.

### Signals and where they come from

- **Artifacts lines.** Already parsed by `store::artifacts`. The artifacts
  preamble in `send.rs` gains the `pr:` kind: *`- pr: #<n>` when you open a
  pull request.*
- **Plan tasks.** For each linked plan, N is the number of `### Task <n>`
  headings in the plan file. k is the number of distinct `Task <n>: complete` lines in that plan's
  superpowers ledger: a `progress.md` under `.superpowers/sdd/*/` whose first
  line is `# SDD ledger — plan: <path>` with `<path>` naming the plan. If no
  ledger is found, k is the number of ticked `- [x]` steps over all steps in
  the plan file, and N is the number of steps. If the plan file cannot be read,
  the stage shows `building ?`.
- **Worktrees.** Specs, plans and ledgers are usually written in a worktree
  (`work start` puts it under `.claude/worktrees/<area>`) and reach the main
  checkout only when merged. Relative artifact paths and ledger lookups search
  the capability's `repo` first, then every path from
  `git worktree list --porcelain` run in that repo. This also fixes reading a
  linked spec or plan from the Artifacts pane before it is merged.
- **Pull requests.** `gh pr view <n> --json state` run in the repo. Polled at
  most once a minute per capability while the list shows, and on `r`. A `pr:`
  target may be `#n` or a PR URL. If `gh` fails the label shows `PR #n` with
  no state, and the stage counts as PR open.
- **Needs you.** `herdr agent list` (JSON), polled every 3 seconds while the
  list shows. The agent whose `name` equals `sent_to.agent` and whose
  `agent_status` is `blocked` sets the flag. An agent no longer listed shows a
  dim `closed` in the flag's place. If `herdr` is unavailable the flag column
  is empty.
- **Caching.** Progress is computed per capability and cached until `r`, a
  timer tick for its source, or an edit to that capability. File reads for
  plans and ledgers happen at most once per refresh.

## Error handling

Every external source fails to "unknown", never to an error screen:

| Failure | Shows |
|---|---|
| `repo` missing or not a directory | bar from file signals only (sent, verified) |
| plan path linked but unreadable | `building ?` |
| `gh` missing, unauthenticated or offline | `PR #n` without state |
| `herdr` missing or its JSON unparseable | no flag |
| `git worktree list` fails | main checkout only |

## What is left to the builder

- Module layout (a `progress.rs` and a `list` renderer are the obvious split).
- Threading for the pollers, as long as the UI thread never waits on them.
- Exact column widths, truncation and colours, within the sketch above, and
  how the row degrades below 80 columns.
- The `last-used.toml` keys for the sort order and the view.
- Whether `g` on a capability whose agent is gone shows a message or does
  nothing.

## Testing

TDD as in the rest of the crate. At minimum:

- status derivation for each row of the status table;
- the send gate refusing `raw` and `?N`, in both views;
- each progress stage from fixture files, including the cumulative rule, the
  ledger and checkbox fallbacks, a plan in a worktree only, and `✓ complete`;
- `FORMAT_REMINDER` and the hand-off preamble containing the new sentences;
- `gh` and `herdr` failures degrading as in the error table, using the
  existing fake-binary fixtures pattern (`tests/fixtures/fake-herdr.sh`);
- the toggle keeping the selection, and the sort orders' ordering;
- a render snapshot of the list at 80 and 120 columns.
