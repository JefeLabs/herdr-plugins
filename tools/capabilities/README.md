# herdr-capabilities

Capture capabilities, refine them in a streaming conversation with an agent CLI, organise them in groups of groups, and send them to a new Herdr agent with the right handoff. Capabilities are plain Markdown files with YAML front matter, so git, `grep` and any editor work on them.

An capability is not a task. Once it is sent it lives in the tracker the agent files it in; nothing is tracked twice.

## Install

Rust toolchain required. Herdr 0.9.0 or later for `send` and the toolbar slot; everything else works standalone in any terminal.

```sh
cargo build --release -p herdr-capabilities
herdr plugin link "$PWD/tools/capabilities"
```

On first run the tool seeds `capabilities.toml` and the kind templates in its config dir:

```sh
herdr plugin config-dir jefelabs.capabilities      # ~/.config/herdr/plugins/config/jefelabs.capabilities
```

Standalone runs use the same directory, so the toolbar and a plain terminal always see one config and one root (`~/capabilities` by default). `CAPABILITIES_CONFIG_DIR` overrides the config dir and `CAPABILITIES_ROOT` the root, so a workspace can point at its own `capabilities/` folder through the toolbar's per-worktree config (`env = { CAPABILITIES_ROOT = "..." }`).

## In the toolbar

Replace the Todo slot in the toolbar's `tools.toml`:

```toml
[[tool]]
id      = "capabilities"
name    = "Capabilities"
icon    = "✎"
command = ["/absolute/path/to/herdr-plugins/target/release/herdr-capabilities"]
```

Capture from any pane with a popup, bound in your Herdr `config.toml`:

```toml
[[keys.command]]
key = "prefix+a"
type = "plugin_action"
command = "jefelabs.capabilities.add"
```

## Shape of an capability

Every capability body has four sections, seeded by the kind's template and kept by the agent's proposals:

```markdown
## Description
What this is, in the author's words (for an issue: observed, expected, steps).

## Preconditions
- [ ] what must already be true for this to be worth building
- [x] one already confirmed

## Acceptance criteria
- [ ] a check a reviewer could verify
- [x] one already met

## Open questions
- Which org saw this?
  → IYM, on Oct 20
- Since when?

## Artifacts
- spec: docs/superpowers/specs/2026-10-25-skipped-folder-design.md
- issue: #5499
```

The right-hand side shows the three parts as separate panes under a header. The Description pane and the agent's replies in the chat render Markdown: headings bold, bullets as dots, bold, italic and code spans styled, code blocks dimmed; the editor shows the raw text. `Tab` and `Shift-Tab` move focus between the tree, Description, Preconditions, Acceptance criteria and Open questions; `Esc` returns to the tree. In Description, `j`/`k` scroll and `Enter` edits. In Acceptance criteria, `j`/`k` choose and `Enter` or space toggles the checkbox, staying in the pane so several can be ticked. In Open questions, `j`/`k` choose and `Enter` opens an answer box right under the question; type (or paste), `Enter` saves, `Esc` cancels. Answers are stored as an indented `→` line under the question, answered questions no longer count as open (the `?N` mark counts only unanswered ones), and the agent is told to keep them with their answers rather than re-ask. `Q` jumps to the questions; `C` jumps to the criteria and, pressed again, to the preconditions. In any of the three list panes `a` adds an item there by hand; a polish drafts all three lists for you. Preconditions are assumptions, not checks: when a capability is sent, the receiving agent is told to verify each one against the code before building anything and to ask about those it cannot settle.

The Artifacts pane (`V`) lists what the agents produced for the capability: specs, plans, ADRs, decision records and tickets, one `- kind: target` line each under `## Artifacts`. Every handoff prompt asks the receiving agent to append such a line for each file or issue it creates. `Enter` reads a Markdown file right here (`o` opens it outside, as do tickets and URLs), `a` adds a line by hand. App-demos are artifacts too: `- demo: <story-id> (criteria: 1, 4)` links a story from the repo's `app-demos/stories/*.yaml` and names the acceptance criteria it shows. `Enter` on it reads the story here (title, actor, host, the stories it runs after, and its steps with their on-video captions), `o` plays the newest recorded video from `app-demos/videos/<story>/` or, without one, opens the demos site at the story (`make demos-open` in app-demos), and each criterion a linked demo shows gets `▶` beside its checkbox. Hand-offs ask agents to add these lines for the stories they write, and stories whose titles match the capability show up as found, so `L` links one. The pane also scans the capability's repo (`docs/superpowers/specs`, `docs/superpowers/plans`, `docs/adr`) for files whose name or first heading shares words with the capability's title and lists them dimmed as *found*, with a `?` after the kind; `Enter` reads them the same way and `L` links one for good. The scan runs once per selected capability and again after `r`.

## Keys

Browse: `j`/`k` move, `h`/`l` collapse/expand a group (sub-groups start collapsed and open one level at a time), `Enter`/`e` edit, `a` add an capability in the selected group, `n` new group, `m` move the selected capability or group (type the target group path), `K` cycle kind, `R` ready (refused while any question is unanswered), `U` back to refining, `p` refine with the agent, `Tab` move between panes, `t` rename (the title is meant to be a capability phrase: who can do what), `Q` jump to questions, `C` jump to criteria, `s` send, `A` attach a file (type or paste its path), `o` open the first attachment, `r` rescan, `q` quit. A file that fails to parse shows as a `!` row with its error. With the mouse, a click selects a row in the tree, a click on the selected group opens or closes it, and the wheel moves the selection.

Editor: type, `Shift+arrows` select, `Ctrl-s` save, `Ctrl-p` refine (on the selection if any), `Ctrl-e` open in `$EDITOR`, `Esc` back.

`p` (or `Ctrl-p` in the editor) first asks which agent: a provider list, then that provider's models if it has any, with your last choice preselected, so `Enter` `Enter` repeats it. The choice is saved in `last-used.toml` in the config dir.

Refine (chat): type and `Enter` to send the next message, `Esc` cancels a running turn or leaves, `Ctrl-a` shows a diff of the latest proposal, `Ctrl-d` dismisses it, `PgUp`/`PgDn` scroll the transcript. Plain letters are always typed, so a message may start with "a" or "d". In the diff, only `Enter` applies; `Esc` goes back; `PgUp`/`PgDn` scroll.

Send: pick a handoff, then on the confirm screen `Tab` cycles the provider, `j`/`k` scroll the prompt, `Enter` sends, `Esc` cancels.

## Refining with an agent

Refinement is a conversation, not a one-shot rewrite. The first turn is seeded by the kind's polish template (`templates/<kind>.polish.md`), the capability text and, when opened on a selection, a focus line. Replies stream in as they are written. Every reply is asked to start with a `capability:` line, a phrase naming who can do what, which becomes the title when you apply, and to end with a fenced ```` ```proposal ```` block holding the complete rewritten capability and a `questions:` list; `a` applies the latest proposal after a diff and merges the questions into `## Open questions`. A reply without a proposal cannot be applied, so chat text never lands in an capability. Your text changes only when you apply.

The conversation id is stored per provider in the capability's front matter and resumed on later turns, and a transcript is appended to `<capability>.chat.md`.

### Providers

| Provider | Polish | Resume | Status |
|---|---|---|---|
| `agy` (default) | `agy -p=… --output-format stream-json --sandbox` | `--conversation <id>` | verified; models include `gemini-4-argon-high/medium/low` |
| `claude` | `claude -p … --output-format stream-json --include-partial-messages --session-id <uuid>` | `--resume <id>` | verified |
| `codex` | `codex exec --json …` | `codex exec resume <id> …` | unverified (account returned 402) |
| `copilot` | `copilot -p … --output-format json` | `--session-id <id>` | unverified (policy denied) |

Any other CLI goes in `capabilities.toml` as a custom provider. Polish runs with no tools and no repository in reach: the built-in argv use `--permission-mode plan --tools "" --strict-mcp-config` (claude), `--sandbox` (agy; its `--mode plan` is ignored while slash expansion is off), `--sandbox read-only` (codex) and `--deny-tool '*'` (copilot), and every turn runs in a neutral directory under the root (`<root>/.polish/<capability>`), never in your checkout. The tool never passes `--allow-all*` or `--dangerously-skip-permissions`.

```toml
polish_provider = "agy"

# tune a built-in without restating it
[providers.agy]
model  = "gemini-4-argon-high"          # or extra = ["--model", "…"]; the picker still lets you change it
models = ["gemini-4-argon-high", "gemini-4-argon-medium", "claude-opus-5-5-high"]   # what the picker offers

[providers.mine]
polish = ["my-cli", "--prompt", "{prompt}"]
resume = ["my-cli", "--resume", "{conv}", "--prompt", "{prompt}"]
output = "text"
send_kind = "claude"
```

## Sending

`s` refuses while the capability is still `raw` or has an unanswered open question, for every handoff: open questions are the happy-path details an agent needs, and the refinement conversation is told to ask only those. Edge cases are the receiving agent's to find; every hand-off prompt says so. Otherwise `s` lists the handoffs that apply to the capability's kind. Built in, all starting a Claude Code agent because they invoke Claude Code skills:

| Handoff | Kinds | What the agent does |
|---|---|---|
| `issue-report` | issue | the repo's `issue-report` skill, confirms the root cause before filing |
| `enhancement-spec` | enhancement | the repo's `enhancement-spec` skill, shows the draft before posting |
| `grill` | any | `/grill-me` on the capability |
| `wayfinder` | any | `/wayfinder` with the capability as the destination |

Send runs `herdr workspace create` on the capability's repo (always the main checkout, never a worktree), `herdr agent start` with the handoff's provider, and `herdr agent prompt` with the rendered prompt. In repos that use workbrokr (`helmsmith.config.ts` present) the building handoffs get a `work start` instruction prefixed; planning handoffs do not. If a step fails the capability is left as it was and the message names the step.

## Affected areas

A capability lives in one group but may affect others. List them in its front matter, or press `+` (in the tree or the list) and type group paths separated by commas; an unknown group is refused, an empty list clears them:

```yaml
areas:
  - skoolscout/discover-schools/org/candidates
  - skoolscout/discover-schools/family/schedule-meeting
```

The tree then shows the capability as a dim `↳` row under each affected group; that row acts on the real capability, and a group's count reads `own +affecting` (e.g. `(3 +2)`). In the list, the path row under the title reads home · other areas, and a hand-off tells the agent the other areas, since a change that spans them may need one work area each.

Areas are also suggested from the work itself: a pull request found for the capability whose branch follows workbrokr's `<type>/<area>--<task>` shape names an area (`feature/org/candidates--…` gives `org/candidates`), and when exactly one group's path ends with it, the card shows `suggested: …`. `+` opens with suggestions already added, so Enter confirms them; nothing is added without that.

## New acceptance criteria

Criteria someone else adds or rewords (an agent editing the file, a reconcile) are new until you acknowledge them. The criteria pane draws them bold with `+` and counts them in its title (`Acceptance criteria (2/10 · 7 new)`); `y` acknowledges the selected one, `Y` all of them. While any are unacknowledged the capability carries `*` before its title, in the tree and on its card, so the last `y` clears it. Your own criteria never count as new: everything present when you send, the criteria of a refinement proposal you apply, and criteria you add with `a` or type in the editor are recorded as seen (`acknowledged:` in the front matter holds a key per criterion's wording, so rewording one makes it new again while ticking it does not). For a capability sent before this record existed, criteria found in its refinement transcript count as seen; with no transcript, all do.

## Reconcile after merge

When every pull request of a sent capability has merged (one closed without merging is ignored), the tool reconciles it with what shipped: in the background it reads the spec, the plan names, the merged pull requests' titles and descriptions and the `issue:` follow-ups, and asks your refinement agent (the last one used) for a rewrite. The rewrite corrects the acceptance criteria to match the shipped behaviour (grouped under `### <area path>` when the work spans areas), names any dropped criterion under Open questions, keeps Description, Preconditions and Artifacts, and leaves follow-ups out of the criteria.

The rewrite is applied straight away; the previous version is kept beside the file as `<name>.before-reconcile.bak`, and `u` puts it back. The capability is marked unread: its card and tree row show `*` before the title until you open it (select it in the tree, or `Enter` on its card). Each merged set is reconciled once (`reconciled_for` in the front matter), so a later merge reconciles again but a restart does not. It runs while the list is open, since that is when pull request states are checked; a capability whose merge happened while the tool was closed is reconciled the next time the list checks.

## List view

`v` flips the browser between the group tree and one full-screen list of every capability, across all groups (collapsed or not). The selection carries across both ways, and the layout, sort order and filter are remembered.

```
Capabilities · 7 · sorted by Updated
────────────────────────────────────────────────────────────────────────────────
 Notify primary contact to schedule a meeting
 skoolscout/org
 Oct 25 08:10 │ → sent │ ! needs you │ 2h ago  │ ▉▉▉██▉▉░░░ PR #42 open
 .claude/worktrees/org/notify │ slot 3

 The primary contact gets no prompt to book the intro meeting.
────────────────────────────────────────────────────────────────────────────────
 Families can skip a folder without seeing Pending
 skoolscout/org
 Oct 24 18:02 │ ?2     │ —           │ —       │ —

 When a parent uses Skip for now on the report-card folder, the folder still
 shows up labelled Pending.…
```

Each capability is a card that opens with a horizontal rule: the title in bold, with its path in the tree under it; then the timestamp, the capability's status, the agent's status, how long ago it was sent and the implementation progress; then, once the agent is working in a worktree, that worktree's path (relative to the repo) and its workbrokr slot, read from the nearest `.slot` file above the agent's working directory; then, after a blank line, up to two wrapped lines of the description.

Before sending, the status reads `raw`, `?N` (N questions unanswered) or `ready`; once sent it reads `→ sent`. The same marks run through the tree: `○` raw, `◐` refining, `●` ready, `→` sent, `✓` only when every acceptance criterion is ticked (complete), and `*` before the title when a reconcile updated the capability and you have not read it yet. Once sent, a bar with one block per stage (sent, specced, planned, building, PR open, merged, verified) shows how far the feature has got, from evidence that already exists. A filled block means that stage is done; the verified block fills by its share of ticked criteria. Done cells are solid (`█`), cells in review dark shade (`▓`), cells still to do light shade (`░`); the kinds are carried by the glyphs because Herdr draws block characters in the default colour. Blocks touch with no gap cells: a done block ends in a slightly narrower cell (`▉`) whose thin sliver marks where it meets the next, and elsewhere the change of shade divides blocks (two to-do blocks side by side read as one light run). The label names the stage you are in.

The building stage holds one block per linked plan, sized from the plan itself: S (up to 3 tasks or 8 files named), M (up to 6 or 20), L (up to 10 or 40), XL beyond, taking the larger of the two; S is one cell wide, M two, L three, XL four, so the bar's length shows how big the effort is. A plan naming no tasks and no files gets no block. Each plan block fills from its own ledger while one exists, is full once a pull request mentioning the plan's file name is merged, and shows `▓` while that pull request is open. For example, two L plans merged and an M plan in review:

```
▉▉▉██▉██▉▓▓▉░░░   PRs 3/4 merged
```

The stages and their evidence:

| Stage | Label | Evidence |
|---|---|---|
| Sent | `speccing` or `charting · N decisions` | `state: sent` |
| Specced | `planning` | a `spec:` or `issue:` line under Artifacts |
| Planned / Building | `building k/N`, or `building` | `plan:` lines; `### Task` headings counted against each plan's superpowers ledger (`.superpowers/sdd/*/progress.md`), in the main checkout or any worktree, summed over plans. The plan executor deletes its ledger when a plan finishes, so without a ledger the label shows no count rather than a guess |
| PR open | `PR #n open`, `PRs m/t merged` | `pr:` lines, plus pull requests whose title or description mentions the capability's spec or plan file names (`gh pr list --search`, on entry, on `r` and about every ten minutes); state from `gh` |
| Merged | `verifying j/M` | every remaining PR merged (closed ones are ignored) |
| Complete | `✓ complete` | every acceptance criterion ticked, by you |

Ticked criteria fill only their own share of the bar; they never stand in for building or merging. The agent field shows the live Herdr state of the agent the capability was sent to: `working`, `idle`, `done`, `! needs you` when it is waiting on you, `closed` when its pane is gone, `…` until Herdr has answered, and `—` before sending.

Keys in the list: `j`/`k` (and `PgUp`/`PgDn`) move, `Enter` opens the capability in the tree, `a` adds a capability to the selected card's group (or at the top level when the list is empty) and selects its new card, `v` back to the tree, `S` cycles the sort order (Updated, Progress with the least advanced first, Longest running with running agents sent longest ago first, Captured with the newest first, Created Asc with the oldest first, Title), `f` cycles the filter (All, In progress for everything not yet complete, Completed for sent capabilities with every acceptance criterion ticked; the header then reads `showing Completed (1 of 7)`), `s` send (same gate), `g` focus the sent agent, `r` rescan, `q` quit. A click selects a card and a click on the selected card opens it in the tree; the wheel moves the selection. Agent states refresh every few seconds while the list shows; plans and PR states on entry, on `r` and about once a minute. `herdr`, `gh` and git run on background threads, and any of them failing only blanks its column. `gh` is found through `GH_BIN_PATH`, else on `PATH`.

## Files

```
~/capabilities/
  2026-10-24-pending-label.md            ← a quick capture, at the top until you move it (m)
  2026-10-24-pending-label.assets/screen.png
  2026-10-24-pending-label.chat.md
  skoolscout/applications/…
```

There is no default group: quick captures (the `add` mode behind the toolbar's capture key) land at the top of the tree, and `a` in the tree adds to the selected group.

Front matter: `title, kind, state (raw|refining|ready|sent), created, updated, repo, tags, polished_at, sent_to, conversations`. Hand edits are read leniently: unknown fields are preserved, a missing title falls back to the file name, dates may be `YYYY-MM-DD`, tags may be a scalar, states are case-insensitive, CRLF is fine. Every field change (state, kind, sent) reloads the file first, so two toolbars never clobber each other's text; a save or an apply over a file that changed since you opened it asks before overwriting.

`send` needs `repo:` set to the main checkout; captures record it automatically (Herdr's `workspace_cwd` inside the toolbar, else the current directory). Without it the tool refuses to send rather than guess.

## Limits (v1)

- Text only inside the toolbar: attachments are listed and `o` opens them externally.
- The editor is deliberately small; `Ctrl-e` hands the file to `$EDITOR` for anything heavy.
- Codex and Copilot event shapes follow their documentation and are unverified on the author's machine.
