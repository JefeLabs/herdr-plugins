# herdr-intents

Capture intents, refine them in a streaming conversation with an agent CLI, organise them in groups of groups, and send them to a new Herdr agent with the right handoff. Intents are plain Markdown files with YAML front matter, so git, `grep` and any editor work on them.

An intent is not a task. Once it is sent it lives in the tracker the agent files it in; nothing is tracked twice.

## Install

Rust toolchain required. Herdr 0.9.0 or later for `send` and the toolbar slot; everything else works standalone in any terminal.

```sh
cargo build --release -p herdr-intents
herdr plugin link "$PWD/tools/intents"
```

On first run the tool seeds `intents.toml` and the kind templates in its config dir:

```sh
herdr plugin config-dir jefelabs.intents      # ~/.config/herdr/plugins/config/jefelabs.intents
```

Standalone runs use the same directory, so the toolbar and a plain terminal always see one config and one root (`~/intents` by default). `INTENTS_CONFIG_DIR` overrides it.

## In the toolbar

Replace the Todo slot in the toolbar's `tools.toml`:

```toml
[[tool]]
id      = "intents"
name    = "Intents"
icon    = "✎"
command = ["/absolute/path/to/herdr-plugins/target/release/herdr-intents"]
```

Capture from any pane with a popup, bound in your Herdr `config.toml`:

```toml
[[keys.command]]
key = "prefix+a"
type = "plugin_action"
command = "jefelabs.intents.add"
```

## Keys

Browse: `j`/`k` move, `h`/`l` collapse/expand a group, `Enter`/`e` edit, `a` add an intent in the selected group, `n` new group, `m` move to a group (type its path), `K` cycle kind, `R` ready, `U` back to refining, `p` refine with the agent, `s` send, `A` attach a file (type its path), `o` open the first attachment, `r` rescan, `q` quit.

Editor: type, `Shift+arrows` select, `Ctrl-s` save, `Ctrl-p` refine (on the selection if any), `Ctrl-e` open in `$EDITOR`, `Esc` back.

Refine (chat): type and `Enter` to send the next message, `Esc` cancels a running turn or leaves, `a` shows a diff of the latest proposal, `d` dismisses it. In the diff, `Enter` applies, `Esc` goes back.

Send: pick a handoff, then on the confirm screen `Tab` cycles the provider, `j`/`k` scroll the prompt, `Enter` sends, `Esc` cancels.

## Refining with an agent

Refinement is a conversation, not a one-shot rewrite. The first turn is seeded by the kind's polish template (`templates/<kind>.polish.md`), the intent text and, when opened on a selection, a focus line. Replies stream in as they are written. Every reply is asked to end with a fenced ```` ```proposal ```` block holding the complete rewritten intent and a `questions:` list; `a` applies the latest proposal after a diff and merges the questions into `## Open questions`. A reply without a proposal cannot be applied, so chat text never lands in an intent. Your text changes only when you apply.

The conversation id is stored per provider in the intent's front matter and resumed on later turns, and a transcript is appended to `<intent>.chat.md`.

### Providers

| Provider | Polish | Resume | Status |
|---|---|---|---|
| `agy` (default) | `agy -p=… --output-format stream-json` | `--conversation <id>` | verified |
| `claude` | `claude -p … --output-format stream-json --include-partial-messages --session-id <uuid>` | `--resume <id>` | verified |
| `codex` | `codex exec --json …` | `codex exec resume <id> …` | unverified (account returned 402) |
| `copilot` | `copilot -p … --output-format json` | `--session-id <id>` | unverified (policy denied) |

Any other CLI goes in `intents.toml` as a custom provider. The tool never passes `--allow-all*`, `--dangerously-skip-permissions` or similar flags: polish runs with whatever permissions the CLI grants by default, and nothing more.

```toml
polish_provider = "agy"

[providers.mine]
polish = ["my-cli", "--prompt", "{prompt}"]
resume = ["my-cli", "--resume", "{conv}", "--prompt", "{prompt}"]
output = "text"
send_kind = "claude"
```

## Sending

`s` lists the handoffs that apply to the intent's kind. Built in, all starting a Claude Code agent because they invoke Claude Code skills:

| Handoff | Kinds | What the agent does |
|---|---|---|
| `issue-report` | issue | the repo's `issue-report` skill, confirms the root cause before filing |
| `enhancement-spec` | enhancement | the repo's `enhancement-spec` skill, shows the draft before posting |
| `grill` | any | `/grill-me` on the intent |
| `wayfinder` | any | `/wayfinder` with the intent as the destination |

Send runs `herdr workspace create` on the intent's repo (always the main checkout, never a worktree), `herdr agent start` with the handoff's provider, and `herdr agent prompt` with the rendered prompt. In repos that use workbrokr (`helmsmith.config.ts` present) the building handoffs get a `work start` instruction prefixed; planning handoffs do not. If a step fails the intent is left as it was and the message names the step.

## Files

```
~/intents/
  Inbox/
    2026-10-24-pending-label.md
    2026-10-24-pending-label.assets/screen.png
    2026-10-24-pending-label.chat.md
  skoolscout/applications/…
```

Front matter: `title, kind, state (raw|refining|ready|sent), created, updated, repo, tags, polished_at, sent_to, conversations`. Unknown fields you add by hand are preserved. If two toolbars edit the same intent, the second save asks before overwriting.

## Limits (v1)

- Text only inside the toolbar: attachments are listed and `o` opens them externally.
- The editor is deliberately small; `Ctrl-e` hands the file to `$EDITOR` for anything heavy.
- Codex and Copilot event shapes follow their documentation and are unverified on the author's machine.
