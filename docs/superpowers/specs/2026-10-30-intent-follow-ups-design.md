# Follow-ups: refinement requests after an intent is done

Status: draft for the owner's review. Decisions marked **(owner)** are the owner's; **(my call)** are mine and open to correction.

## Why

Refinement requests arrive after work is technically complete: the intent shipped, every criterion is proven, the card reads `✓ complete`, and then someone wants it changed. The tool has no place for that today. Editing the original's criteria after the fact would reopen it, and mix the new work's pull requests, reconcile and check with the old ones.

The owner wants each refinement to be its own piece of work, visible from the original. Once the refinement ships, the original should still describe what the app does.

## What it does

### Creating a follow-up

- **Shape (owner, agreed: "standalone, linked"):** a refinement is a standalone intent, a follow-up, linked both ways to the intent it refines (the original).
- **How it is made (my call, agreed with "looks good"):** a key on a sent intent, `F`, opens the add-by-conversation view already linked to it:
  - in the tree, on a card, and in the detail pane;
  - the agent gets the original's title, description and acceptance criteria as context, as data;
  - the kind question, the restatement, the questions and the write-up work as for any intent.
- **Where it is filed (my call):** the follow-up goes in the original's group, as refining, and is never sent by the tool.
- **Which intents (my call):** `F` works on any sent intent, `✓ complete` or not; refinements usually come after complete. On an unsent intent it is refused: you can still edit that one.
- **A follow-up's follow-up (my call, from the builder check):** `F` on a follow-up links the new one to the first original, the intent that describes the feature. Every refinement then folds into that one place, and the follow-up the owner started from is named in the new one's description.

### On the original

- **Section (owner, agreed):** a **Follow-ups** section in the detail pane lists each follow-up with its state (`○` not sent, `→` sent, `✓` complete) and title. Enter goes to it. It joins the accordion and the Tab order after Missing items / Bugs.
- **Card (owner):** while any follow-up is open, the original's card reads `✓ complete · 1 follow-up` (`· 2 follow-ups`…). Once all are complete it reads `✓ complete` again.
- **Not a blocker (owner):** a follow-up never takes away the original's `✓`; what the original promised was delivered. A missing item does block it, since it is a gap in the original's own criteria. That difference is why the two are separate sections.

### When a follow-up ships, the original is updated to match (owner)

- **When (my call, agreed with "looks good"):** once the follow-up reads `✓ complete` (its own check proved the new behaviour), not merely when its pull requests merge.
- **The original is finished first (my call, from the builder check):** a fold waits until the original has no open pull requests and its own reconcile has been applied for its merged set. A later regular reconcile of the original, which builds criteria only from the original's own spec and pull requests, would otherwise drop what was folded in.
- **How:** a fold, a reconcile of the original with a fold prompt.
  - The agent gets the original's body, plus the follow-up's description, acceptance criteria and merged pull requests (titles and bodies).
  - It changes the original's acceptance criteria to describe the app as it now behaves, touching only what the follow-up changed.
  - It copies a criterion that still holds word for word, checkbox included. New and changed criteria start unticked. Description, Preconditions and Artifacts are kept, as the reconcile rule already says.
- **Applied like a reconcile:**
  - the previous version is kept, and `u` puts it back. For a fold, `u` also means "don't fold this one" (owner, "go with your recommendations"): that is recorded on the follow-up, so it never folds again on its own. To redo it, the owner edits the original's criteria by hand;
  - the original is marked unread (`*`);
  - the changed criteria show as new (`+`) until acknowledged (`Y`);
  - a section or artifact line the rewrite dropped is restored, as reconcile already does.
- **Then the original re-proves itself (my call, agreed):** a completion check runs on the original for the criteria the fold added or changed, if the repo has a capture guide. When they pass, the original reads `✓ complete` again, now describing the current behaviour. Without a guide, they wait for the owner's ticks.
- **A failure after a fold files nothing (owner, "go with your recommendations"):** the follow-up's own check already proved the new behaviour, so a fold-written criterion that fails most likely means the fold described it wrongly, not that the app is broken. It stays unticked, marked "the fold may describe this wrongly", for the owner to fix or tick. No missing item is filed, and the original's `✓` is not blocked by an item.
- **Several follow-ups (my call, agreed):** each folds in when it completes, one fold at a time per original, in the order they complete. Each is recorded, so it folds once.
- **The follow-up's own ticks never move to the original:** the original's changed criteria are proven by the original's own check (or the owner).

### What "complete" means for the original

It is unchanged from the completion check: every criterion ticked, every pull request merged, no missing item open, every criterion acknowledged. Open follow-ups only add the `· N follow-up(s)` note.

## Goals

- A refinement after completion has a place: its own intent, linked, visible from the original.
- The original's `✓` stays honest. It shows done while refinements are worked on, and its criteria describe the shipped app once they land.
- No new machinery where existing parts fit: the add conversation, reconcile (with its undo and its `*`), completion check, and the linked-section pattern of Missing items.

## Non-goals

- Turning a refinement into new criteria on the original before it ships.
- Folding a follow-up that never completes, or one that is deleted. Deleting a follow-up just removes its link.
- Merging the follow-up's pull requests or plans into the original's progress bar.
- Follow-ups of follow-ups get no special handling: a follow-up is an intent, so `F` works on it too.

## Anti-goals

- A fold that rewords or drops criteria the follow-up did not touch. The prompt says to copy them word for word, and the existing restore of dropped sections and artifact lines applies.
- A fold that leaves the original reading `✓ complete` on criteria nobody proved. Changed and new criteria start unticked and unread, and the check or the owner proves them.
- A follow-up folded twice, or two folds of one original at once.
- Confusing the two kinds of extra work: missing items (gaps that block) and follow-ups (new wants that don't).

## Constraints

- **Naming:** the reconcile already speaks of "follow-up issues", the `issue:` lines agents file, and is told never to turn them into criteria. The new follow-ups are intents. The fold uses its own prompt, and the existing reconcile prompt stays as it is.
- **Links survive moves:** they use file stems, as missing-item links do.
- **Pull requests stay with their intent (my call, from the builder check):** pull requests linked to or found for a follow-up are left out of the original's set. They must not drop the original from `✓` to "PR open", nor start the original's own reconcile and check. When both intents claim one, the intent with the `pr:` line owns it.
- **Front matter:** new fields are skipped when empty, so existing intent files are unchanged.
- **Separate from a regular reconcile:** a fold is not one, and the original's `reconciled_for` (its own merged set) is unchanged by a fold. The completion check after a fold is started by the fold, not by `check_due`.
- **Repo rules:** no push or release without the owner's word; tests run with `</dev/null`.

## Left to the builder

- Field names for the links and for "folded".
- The fold prompt's exact wording, within the rules above.
- How the add view carries the original as context.
- The Follow-ups section's empty text and its help line.
- Whether the fold's check starts right after the fold or on the next poll.
