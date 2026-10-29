# Evidence capture for bugs

Date: 2026-10-29. Tool: `tools/intents` (herdr-intents). Status: design, not built.

## Why

A bug captured by conversation is words only. The owner wants the agent to reproduce the bug in a real browser and keep a screenshot or video of it. After the fix merges, the same steps run again to show the bug is gone. The before/after pair travels with the intent, and the issue filed from it shows them.

## The flow

Owner decisions are marked (owner); the rest are my calls.

1. **Ask after the questions** (owner). On a bug (kind `issue`), once the last interview question is answered or skipped, the add view asks: `Capture evidence? s screenshot · v video · Enter skip`. The write-up then goes ahead as today.
2. **Capture runs in the background** (owner). The conversation does not wait. The intent can be saved while capture runs. Its Evidence section shows `capturing…` until the result arrives.
3. **A local stack of its own** (owner: "leverage our local setup", "intent's own area"; the stack rules below follow the reviewer's reading of workbrokr, approved 2026-10-29):
   - the tool starts a task effort in the intent's repo (`repo:`): `work start --area <area> --task evidence-<n> --type fix --mode fullstack`, with its own session name (`WORK_SESSION=capture-<stem>`) so it never touches your efforts;
   - the area is the intent's own when that area can run a full stack: the area in the repo's `helmsmith.config.ts` whose name ends the intent's group path (`skoolscout/discover-schools/org/team` → `org/team`) and that has a backend module;
   - otherwise (no matching area, a `platform/*` area without a module, a scope conflict with an open effort, or a group not yet known from the popup or the card view), it uses the default area the capture guide names, since every stack serves every page;
   - the stack serves every area, so one video can cross areas (owner: "a video may span across multiple areas").
4. **Seed the stack** (owner, 2026-10-29). Before the agent starts, the tool runs the capture guide's base seed, the e2e stories that create demo-org, MARK and the family personas (`make e2e-story STORY=<id>` in the task worktree). Anything specific to the bug, such as an application or a family, the agent creates through the UI as part of its recorded steps, so the "after" redoes it the same way on a fresh slot.
5. **Sign in with the dev persona picker** (owner): `https://demo-org.skoolscout.com.local:<frontend port>/auth/sign-in`, Command+U, the persona whose role sees the bug.
   - Codes: email 123456, SMS 654321 (owner), whichever channel the screen says it sent to.
   - The agent may sign out and in again on camera to show a second role.
   - No credentials are written anywhere: the picker already holds the e2e cast's.
6. **The agent reproduces it.** It reads the repo's capture guide and runs agent-browser:
   - a fixed window size;
   - for `v`, `record start` … `record stop` to an `.mp4`;
   - for `s`, a `.png` of the moment the bug shows.

   It reports `reproduced: <one line>` or `couldn't reproduce: <what it tried>`, and saves the steps it took for the "after".
7. **After the fix** (owner, from the original note). When every pull request of the sent intent has merged (the moment reconcile already uses), the tool runs the same steps again: same form, same window size, on a fresh, freshly seeded task effort synced from the merged develop.
   - A bug's hand-off files an issue but opens no PR, so the tool also finds PRs that mention the issue number (`gh pr list --search`), alongside the `pr:` lines and the existing spec and plan search. You can still add one by hand in Pull requests. Evidence then shows the before/after pair. An "after" that still shows the bug is marked ✗.
8. **Where the files live** (owner):
   - **Until the intent is sent and its issue is created,** captures stay in a scratch folder, the intent's agent folder under `.polish/`.
   - **Once an `issue:` line appears under its Artifacts,** they move into the intent's attachments folder (`<stem>.assets/`) and stay there. They are never committed to a repo.
   - **Trashing the intent** moves them along with it.
   - **The issue's screenshot** (owner, 2026-10-29): the `issue-report` skill requires a PNG committed next to the `.issues/` file, and the capture screenshot is used as that PNG. Videos stay attachments that you paste into the issue (⌘V).
9. **Capture again** (my call). On a bug intent, a key in the detail pane starts a capture, or re-runs a failed one. Stack starts do fail here (old slot database, no free slot, sync conflict), and one shot would leave a bug with no evidence.

## The Evidence section

A new detail-pane section, beside Artifacts:
- one row per capture: before or after, screenshot or video, and the one-line result;
- **Enter or a click** opens the file in Chrome;
- **`c`** copies the file itself: on macOS, a pasteboard file URL. The owner tested that ⌘V into a GitHub comment uploads the `.mp4`. Over SSH, where there is no pasteboard, `c` copies the path instead;
- **`Y`** copies the path.

## Goals

- A bug can carry visible proof without the owner leaving the add flow.
- The same steps prove the fix after merge, with the result shown next to the original.
- The agent cannot do anything but drive the browser.

## Non-goals

- Features, refactors and ideas get no evidence prompt.
- Intents stay in `~/intents` (owner: "keeping it off ~ is good for now"). Moving them into the space's meta repo (`.helmsmith/`) is separate, and may not be needed now that videos open from Evidence.
- No upload to GitHub by the tool: the owner pastes (⌘V) when the agent filing the issue asks.
- No deployed environments. Capture runs against the local stack only.

## Anti-goals

- The capture agent gets a shell, edits code, or runs anything but `agent-browser`.
- A credential is written to a file, a prompt or a log.
- Capture blocks the conversation or the save.
- A capture's stack is left running and squatting a slot after capture finishes or fails.
- An "after" is compared at a different window size or on a different form from the "before".

## Constraints

- **The tool runs `work`, not the agent** (my call, recorded for the owner to overturn).
  - `work` is the only way to start a stack in this setup. Slot 0 is the owner's: never `SLOT=0`, never in the main checkout.
  - The tool reads the frontend port from `work env --area <area> --json`.
- **Capture never pushes a branch and never joins.**
  - Its task effort is synced from develop on start: the "before" sees the bug as it stands there, and the "after" sees the merged fix.
  - It ends with a plain `work finish --area <area> --task evidence-<n>`, which pushes nothing because capture makes no commits. It never uses `--into-area` (that switches and fast-forwards area branches) or `--join` (that browses someone's half-built branch).
  - A start that fails (HEALTH_TIMEOUT, NO_FREE_SLOT, SYNC_CONFLICT) ends the capture as `capture failed: <reason>`, ready to try again.
- **The capture agent's tools are `agent-browser` alone** (owner's original note: "allowed only agent-browser commands"). With claude that means `--allowedTools "Bash(agent-browser:*)"` and no other tool.
  - My call: capture uses claude whatever `m` chose for polish, because only claude takes a per-command allow-list.
  - If claude is missing, the prompt says so and offers to skip.
- **Each repo carries a capture guide:** `.helmsmith/capture.md` in the repo (my call: `.helmsmith` is already the helmsmith folder). It covers:
  - the default area to use when the intent's own area can't run a stack;
  - the base seed: the e2e stories the tool runs before the agent starts;
  - how to reach the app: host and port from the slot;
  - how to sign in: Command+U, which persona for which role, the dev codes;
  - what to avoid: never run an e2e story with `mark-signup`, since it re-creates demo-org and logs the session out.

  The tool hands the whole guide to the agent.
- **Formats:** `.mp4` for video and `.png` for screenshots (my call; agent-browser records both).
- **The saved steps are the agent-browser commands the agent ran,** kept beside the capture. The "after" agent gets them as its instructions, along with what the "before" showed.
- **Stack lifetime:** a capture's own task effort is finished as soon as the capture ends or fails, so no slot stays leased. A joined effort is left as it was.
- **Tests** use a fake `work` (which records its arguments and `WORK_SESSION`, so a test can assert capture never passes `--join` or `--into-area` and always finishes what it started) and a fake `agent-browser` (shell fixtures, like the existing fake `herdr` and fake provider), run with `</dev/null`.

## In skoolscout-com

These are small additions in the other repo, for the owner to approve with the plan:
- `.helmsmith/capture.md`, the capture guide above.
- The `issue-report` skill's Evidence section lists the intent's capture attachments. It uses the capture screenshot as the issue's committed PNG, and tells the owner to paste the video (⌘V) when the issue is created on GitHub.

## Left to the builder

- Where in the code capture lives: probably a `capture` module beside `interview`, with the app glue in `app.rs`.
- The wording of the capture prompt and how the guide is passed to the agent.
- How the Evidence rows look at narrow widths.
- How ✗ is decided for an "after": the agent's own `reproduced`/`fixed` line, plus agent-browser's `diff screenshot` against the "before" for screenshots.
