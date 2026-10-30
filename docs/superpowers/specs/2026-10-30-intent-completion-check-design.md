# Completion check for sent intents

Status: draft for the owner's review. Decisions marked **(owner)** are the owner's; **(my call)** are mine and open to correction.

## Why

A sent intent reads `✓ complete` only when every acceptance criterion is ticked. Ticks come only from the owner, one at a time in the Criteria section. The reconcile after merge is told never to tick ("ticks are the owner's sign-off"). So once the work ships, a card sits at "verifying 0/5" until the owner checks each criterion by hand, even when the owner knows it is done.

The owner wants the tool to check instead: confirm that every pull request has merged and that every acceptance criterion holds in the running app. It should tick what it confirmed, keep proof, and turn what fails into new work.

## What it does

### The check

- **What it confirms (owner):** that all of the intent's pull requests have merged, and that each acceptance criterion holds.
- **How it confirms a criterion (owner):** an agent drives the app with agent-browser. For criteria the UI cannot show, such as "an email is sent", it reads the stack's logs (owner: "an email is sent can be captured in logs").
- **PR status (my call):** the tool reads it from `gh`, as progress already does. No agent is needed for this.
- **The stack:** it runs on evidence capture's machinery:
  - a fresh task effort from `work` (areas: the intent's own, then the guide's `default_area`), so it gets the merged code;
  - the guide's seed stories;
  - a detached `herdr-intents` runner that holds a lock and always runs `work finish`;
  - the same confined agent: `claude --restricted --permission-mode dontAsk --tools ""` with only the `capture-mcp` tools.
- **One more tool, `logs` (my call on shape):**
  - it searches the check stack's own log files for a pattern and returns the matching lines;
  - it is read-only and bounded (a line and byte cap);
  - it reads only files the repo's capture guide lists, relative to the stack's worktree, so it cannot read any other path;
  - it returns only lines written after the agent started (my call, from the builder check). The seed stories send emails into the same `.logs/backend.log`, so an "email is sent" criterion could otherwise be confirmed by the seed's own output;
  - there is still no shell.
- **Results per criterion:** criteria are numbered from 1 in order across their area groups, as demos number them. The agent answers each with one of:
  - `confirmed: <what it saw>`;
  - `fails: <what it saw instead>` (built, but wrong);
  - `missing: <what is not there>` (not built);
  - `can't check: <why>` (neither the app nor the logs can show it, e.g. "the migration is idempotent").
- **No tick without proof (my call):** a `confirmed` counts only when the run kept proof for that criterion, either a screenshot or the log lines the `logs` tool returned. Otherwise it is recorded as `can't check: no proof kept`. This guards the anti-goal of a false `✓ complete`.

### When it runs (owner: both)

- **On a key:** `X` (my call; `V` already opens Artifacts). It works in the tree, on a card, and in the Criteria section, on the selected sent intent. It is refused while that intent's check or capture is running, the way `E` is.
- **Automatically:** once every PR has merged and the reconcile for that merged set has been applied, the check runs once per merged set. Its own front-matter record works like `reconciled_for`. The reconcile goes first because it may reword or add criteria, and the check should test what the owner will read.
- **For a bug with a reproduced "before" (my call):** the automatic check also records the "after" in the same run, so a merged bug starts one stack, not two.
- **Concurrency (my call):** automatic checks run one at a time per repo; the others wait their turn. `work` would refuse overlapping efforts in one area anyway. A restart picks up what is still due, through the per-set record.
- **The day it ships (owner, "go with your recommendations"):** on upgrade, every sent intent whose PRs had already all merged is recorded as checked for its current merged set. So only merges after the install start an automatic check; there is no backlog of stacks, and intents already `✓ complete` stay that way. `X` still checks any older intent on demand.

### Ticks

- **Ticking (owner, via "agent confirms all ACs"):** a check ticks the criteria it confirmed.
- **Never yours (my call):** a check never unticks a tick the owner made. It may untick only a tick a check made, when a later check of that criterion fails. If an owner-ticked criterion fails, it shows `✗` beside the tick and still creates a missing item.
- **Recording who ticked (my call):** each tick a check made is recorded as such, with time and proof, so "who ticked this" is never a guess. Criterion wording is never changed.
- **Your untick wins (my call):** when the owner unticks a tick a check made, that is recorded too. Later checks still show their result beside the criterion, but never tick it again; only the owner can.

### Missing items / Bugs (owner)

- **What gets created:** each `fails` or `missing` criterion becomes a new intent in the original's group:
  - `fails` creates a bug;
  - `missing` creates a feature (my call on the mapping; the owner's name for both is "Missing item/Bug").
- **What the new intent holds:**
  - a title from the gap;
  - a description naming the original intent and criterion, what was expected, and what the check saw;
  - one acceptance criterion: the original criterion's text, so its own check tests the same thing;
  - the proof in its attachments;
  - a link back to the original.
- **Not sent (my call):** it is created as refining. The owner reviews and sends it like any intent.
- **On the original (owner):** a new **Missing items / Bugs** section comes right after Pull requests. It lists each item with its state (refining, sent, `✓ complete`) and the criterion it came from; Enter jumps to the item. It joins the Tab order and the accordion like the other sections.
- **No duplicates (my call):** while an item is open, a later check that fails the same criterion adds its proof to that item instead of creating another.
- **Overruling a wrong result (owner, "go with your recommendations"):**
  - deleting a missing item (`D`, to the trash as today) dismisses that gap. The original stops counting it, and later checks do not file it again for that criterion;
  - ticking the item's criterion yourself closes it, even while it is unsent, and that triggers the original's re-check as usual;
  - if a criterion fails again after its item was completed, the check files a new item rather than reopening the completed one.
- **`can't check` creates nothing:** it stays for the owner to tick.
- **Links survive moves:** `M` keeps file names, so the link follows the intent wherever it is moved.

### Re-checking the original (owner)

- **The trigger:** a missing item reaches `✓ complete`, whether its own check passed or the owner ticked it.
- **What runs:** the original re-checks only the criterion that item came from, on a fresh stack.
- **If it passes:** it is ticked. If that was the last open thing, the original reads `✓ complete`.
- **If it fails again:** a new item is filed (owner, under "Overruling a wrong result" above), and the completed one stays as it was.

### What "complete" means now

Every acceptance criterion is ticked, every pull request has merged, no missing item is open, and every criterion is acknowledged.

- **A missing item is open** until it reads complete, its criterion is ticked by you, or it is deleted.
- **Acknowledged (owner, "go with your recommendations"):** a check may tick criteria the reconcile added or reworded, but the intent reads complete only once every criterion is acknowledged too. So a criterion an agent wrote and an agent confirmed never completes an intent unread. Until then the card reads e.g. `✓ 5/5 · 2 new to read`, and `Y` acknowledges as today.

### Where the results show (owner: "that split works")

- **In the Criteria section:** each criterion shows its latest result beside the checkbox: `✓` confirmed, `✗` fails or missing, `?` can't check. It also shows the one-line finding and the date of the check, the way `▶` marks a demo today. `o` on a criterion opens its proof.
- **In the Evidence section:** the proof files from the latest check, one row per criterion, next to a bug's before/after. The existing keys work on them: Enter opens in Chrome, `c` copies the file, `Y` copies the path. Log proof is a short text file of the matching lines.
- **Not in Demo Videos:** that section stays for the app-demos stories the owner curates.
- **On cards:** while a check runs, the card reads `checking 2/5`, then the usual labels. The card also counts open missing items.

## Goals

- Done work shows as done without ticking every criterion by hand.
- Every tick the tool makes has proof one key away.
- Gaps become work: each failing criterion is an intent, linked both ways, and the original completes itself once they are fixed.

## Non-goals

- Deployed tiers. Checks run on a local stack only, as captures do.
- Reviewing code quality, writing tests, or judging the PRs' content.
- Sending missing items, or changing the wording of any criterion.
- A screenshot diff between runs.

## Anti-goals

- A `✓ complete` that is not true. Hence no tick without proof, and never unticking the owner's ticks.
- An agent that can do more than drive the browser and read the listed logs.
- A new missing item for the same gap on every re-check.
- A check reading another slot's logs or seeding another slot.
- A check left running, or a stack left leased, when the tool quits or the intent is deleted. The runner is detached and finishes its effort, and trash or discard stop it, as with captures.

## Constraints

- **Capture's runner and confinement:** reuse them. A check is a detached run with its own lock, records it writes itself, and `work finish` whatever happens. It needs a `claude` with `--restricted` and `dontAsk`, and fails closed before any stack starts if the installed one lacks them.
- **The capture guide (`.helmsmith/capture.md`):** it gains `logs:`, a list of log files relative to the stack's worktree. No guide means no check: the card stays at "verifying" and the owner ticks by hand.
- **One guide for both (my call):** the guide's body describes the stack (who is seeded, how to sign in, what to leave alone). The tool's prompt alone sets how the agent reports, whether that is a capture's `reproduced:` or a check's per-criterion lines. The skoolscout-com guide is reworded to match: its bug-only opening and its `couldn't reproduce: needs <name>` line become neutral ("say it needs <name>, who is not seeded on this stack"). That change goes on this session's own unpushed `chore/platform/tooling--capture-guide` branch, and is pushed only on the owner's word.
- **In skoolscout-com:**
  - the guide lists `logs: [.logs/backend.log]`, which is where a slot stack's backend writes. Locally, email output runs in TERMINAL mode, which logs each rendered email (From, To, Subject, Body), so an "email is sent" criterion can be confirmed from that log;
  - this rides on the capture-guide branch (`chore/platform/tooling--capture-guide`, unpushed).
- **Criterion identity:** one criterion's results stay attached to it across checks by its wording, the same key `acknowledged:` uses. A reworded criterion is a new one.

## Left to the builder

- **Data formats:**
  - where per-criterion results and proof live (e.g. `<stem>.assets/checks/`);
  - the front-matter fields for "checked for this merged set" and for the missing-item links;
  - how the agent's per-criterion lines are parsed.
- **Shape of the extra tools:**
  - `logs` (pattern syntax, caps);
  - a per-criterion `screenshot`, so proof lands under the right number.
- **The check's time limit,** as long as it grows with the number of criteria.
- **Card wording,** apart from `checking 2/5` and the `✓ ✗ ?` marks.
