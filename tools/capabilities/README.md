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

The Artifacts pane (`V`) lists what the agents produced for the capability: specs, plans, ADRs, decision records and tickets, one `- kind: target` line each under `## Artifacts`. Every handoff prompt asks the receiving agent to append such a line for each file or issue it creates. `Enter` reads a Markdown file right here (`o` opens it outside, as do tickets and URLs), `a` adds a line by hand. The pane also scans the capability's repo (`docs/superpowers/specs`, `docs/superpowers/plans`, `docs/adr`) for files whose name or first heading shares words with the capability's title and lists them dimmed as *found*, with a `?` after the kind; `Enter` reads them the same way and `L` links one for good. The scan runs once per selected capability and again after `r`.

## Keys

Browse: `j`/`k` move, `h`/`l` collapse/expand a group (sub-groups start collapsed and open one level at a time), `Enter`/`e` edit, `a` add an capability in the selected group, `n` new group, `m` move the selected capability or group (type the target group path), `K` cycle kind, `R` ready (refused while any question is unanswered), `U` back to refining, `p` refine with the agent, `Tab` move between panes, `t` rename (the title is meant to be a capability phrase: who can do what), `Q` jump to questions, `C` jump to criteria, `s` send, `A` attach a file (type or paste its path), `o` open the first attachment, `r` rescan, `q` quit. A file that fails to parse shows as a `!` row with its error.

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

## List view

`v` flips the browser between the group tree and one full-screen list of every capability, across all groups (collapsed or not). The selection carries across both ways, and the layout and sort order are remembered.

```
Capabilities · 7 · sorted by Updated          v tree · S sort · s send · g agent
────────────────────────────────────────────────────────────────────────────────
 Notify primary contact to schedule a meeting
 Oct 25 08:10 │ sent   │ ! needs you │ ███████░░░ PR #42 open
 skoolscout/org │ slot 3

 The primary contact gets no prompt to book the intro meeting.
────────────────────────────────────────────────────────────────────────────────
 Families can skip a folder without seeing Pending
 Oct 24 18:02 │ ?2     │ —           │ —
 skoolscout/org

 When a parent uses Skip for now on the report-card folder, the folder still
 shows up labelled Pending.…
```

Each capability is a card that opens with a horizontal rule: the title in bold on its own row; then the timestamp, the capability's status, the agent's status and the implementation progress; then where it lives (its group, and once sent the agent's workbrokr slot, read from the nearest `.slot` file above the agent's working directory); then, after a blank line, up to two wrapped lines of the description.

Before sending, the status reads `raw`, `?N` (N questions unanswered) or `ready`. Once sent, a ten-cell bar shows how far the feature has got, from evidence that already exists:

| Stage | Label | Evidence |
|---|---|---|
| Sent | `speccing` or `charting · N decisions` | `state: sent` |
| Specced | `planning` | a `spec:` or `issue:` line under Artifacts |
| Planned / Building | `building k/N` | `plan:` lines; tasks counted against each plan's superpowers ledger (`.superpowers/sdd/*/progress.md`), in the main checkout or any worktree, summed over plans |
| PR open | `PR #n open`, `PRs m/t merged` | `pr:` lines, state from `gh pr view` |
| Merged | `verifying j/M` | every remaining PR merged (closed ones are ignored) |
| Complete | `✓ complete` | every acceptance criterion ticked, by you |

Ticked criteria fill only their own share of the bar; they never stand in for building or merging. The agent field shows the live Herdr state of the agent the capability was sent to: `working`, `idle`, `done`, `! needs you` when it is waiting on you, `closed` when its pane is gone, `…` until Herdr has answered, and `—` before sending.

Keys in the list: `j`/`k` (and `PgUp`/`PgDn`) move, `Enter` opens the capability in the tree, `v` back to the tree, `S` cycles the sort order (Updated, Progress with the least advanced first, Captured, Title), `s` send (same gate), `g` focus the sent agent, `r` rescan, `q` quit. Agent states refresh every few seconds while the list shows; plans and PR states on entry, on `r` and about once a minute. `herdr`, `gh` and git run on background threads, and any of them failing only blanks its column. `gh` is found through `GH_BIN_PATH`, else on `PATH`.

## Files

```
~/capabilities/
  Inbox/
    2026-10-24-pending-label.md
    2026-10-24-pending-label.assets/screen.png
    2026-10-24-pending-label.chat.md
  skoolscout/applications/…
```

Front matter: `title, kind, state (raw|refining|ready|sent), created, updated, repo, tags, polished_at, sent_to, conversations`. Hand edits are read leniently: unknown fields are preserved, a missing title falls back to the file name, dates may be `YYYY-MM-DD`, tags may be a scalar, states are case-insensitive, CRLF is fine. Every field change (state, kind, sent) reloads the file first, so two toolbars never clobber each other's text; a save or an apply over a file that changed since you opened it asks before overwriting.

`send` needs `repo:` set to the main checkout; captures record it automatically (Herdr's `workspace_cwd` inside the toolbar, else the current directory). Without it the tool refuses to send rather than guess.

## Limits (v1)

- Text only inside the toolbar: attachments are listed and `o` opens them externally.
- The editor is deliberately small; `Ctrl-e` hands the file to `$EDITOR` for anything heavy.
- Codex and Copilot event shapes follow their documentation and are unverified on the author's machine.
