# intents: capture, refine and send intents to agents

**Date:** 2026-10-24
**Status:** approved by the owner 2026-10-24
**Owner:** Edwin Cruz (jefelabs)
**Home:** `jefelabs/herdr-plugins`, at `tools/intents/`, crate and binary `herdr-intents`.
Runs as a slot in the toolbar (`toolbar/`, see `2026-10-24-herdr-toolbar-design.md`)
and also standalone in any terminal.

## Intent and the why

The owner has three places where work is tracked (GitHub issues, Jira, wayfinder
maps) and a planning pipeline that starts from a spec. What is missing is the
step before any of those: a place where a half-formed intent lives while it is
still the owner's. A user's complaint in their own words with a screenshot, an
enhancement idea with a rough user story, a refactor nobody has sized yet.
These get captured in a line, refined over days, arranged in groups of groups,
and only when ripe are they *sent*: handed to an agent that files the issue,
grills the owner, or charts a map.

Taskwarrior was tried for this slot and rejected: it tracks tasks, which
GitHub issues and Jira already do. An intent is not a task. Once an intent is
sent it leaves this tool and lives in the tracker, so nothing is tracked twice.

## Decisions made in the brainstorm

The owner's decisions, as decided.

1. **Intent management, not a notebook and not a task list.** Capture,
   refine, organise, send. Issues and enhancements are two *kinds* of intent
   with templates that help the refining; screenshots are attachments.
2. **Groups of groups.** A tree of groups, any depth, each holding intents.
3. **Agent polish in the editor.** While editing an intent, one key sends the
   whole intent, or the selected passage, to an agent and gets a sharpened
   version back in the kind's template. The owner accepts, edits or discards
   it. The owner's text is never replaced silently.
4. **Polish runs through `agy -p` by default, or any other configured CLI in
   print mode.** The command is configuration, not code: an argv template
   with placeholders. **agy, Claude Code, Codex and GitHub Copilot are
   supported out of the box** as named providers (decided 2026-10-24), and any
   other CLI can be added in config. Providers are also the choices for which
   agent `send` starts.
4a. **JSON print output.** Providers run in their JSON/JSONL print mode and
   the tool reads the reply text from the documented event field, never from
   a raw terminal dump. Inside that reply text the agent writes prose plus a
   fenced `proposal` block and a `questions` list (see Polish); those blocks,
   not a JSON schema, are what `apply` reads, because a schema cannot wrap a
   streaming conversation. A reply with no `proposal` block cannot be applied
   (`a` is disabled with a hint), so chat text never lands in an intent.
4b. **Refinement is a continuous conversation, not a one-shot rewrite**
   (decided 2026-10-24). Each intent carries a conversation with the polish
   provider. The owner asks, the agent answers, the owner asks again; at any
   point the owner applies the agent's latest proposal to the intent text. The
   conversation resumes by id in later sessions, and **streaming is required**
   so replies appear as they are written.
5. **Send means: start an agent.** Sending an intent starts a new agent in a
   fresh Herdr workspace with the intent as its opening prompt, chooses the
   handoff by kind, and records where it went.

## Calls made by the designer (overturnable)

- **Data is plain Markdown in a directory tree** that mirrors the groups:
  `<root>/<group>/<subgroup>/<intent-slug>.md`, attachments beside it in
  `<intent-slug>.assets/`. YAML front matter holds the metadata. Git-friendly,
  greppable, editable in any editor, readable by `glow` in another slot.
- **Root location.** Default `~/intents`; configurable; a workspace may point
  at its own root (an `intents/` folder in the repo) through the toolbar's
  per-worktree config, via `env = { INTENTS_ROOT = "..." }`.
- **Kinds and templates.** Built in: `issue` (observed, expected, steps,
  evidence), `enhancement` (as a `<who>`, I want `<what>`, so that `<why>`,
  plus acceptance notes), `idea` (free form). Templates are Markdown files in
  the config dir, so the owner can edit or add kinds.
- **States.** `raw`, `refining`, `ready`, `sent`. `ready` is set by the owner;
  polish never changes state. `sent` is set by the tool with a `sent_to`
  record (handoff name, workspace id, date).
- **Each polish reply is asked to carry a proposal and questions.** The
  conversation prompt instructs the agent to end every turn with a fenced
  `proposal` block holding the **full rewritten intent body**, and a
  `questions` list. A conversation is always about the whole intent: opening
  it on a selection only adds "focus on this passage" to the seed message, the
  proposal is still the full body, and `apply` always replaces the full body
  after showing a diff. This avoids stale selection offsets across sessions
  and never pastes a full rewrite into a passage. Questions go to `## Open
  questions`; an intent with open questions is marked as not ready.
- **One conversation per intent per provider**, stored by id in front matter
  (`conversations: { agy: "3d0a…", claude: "…" }`). Switching provider
  mid-intent starts a fresh conversation with the intent text as context.
  The transcript is also appended to `<intent-slug>.chat.md` so it survives
  the provider's own session storage and is readable without the tool.
- **Handoffs are configuration too.** Each is a name, the kinds it applies
  to, the **provider** whose agent `send` starts (the four built-ins all say
  `claude`, because `/grill-me`, `/wayfinder`, `issue-report` and
  `enhancement-spec` are Claude Code skills that other CLIs cannot resolve),
  and a prompt template. The confirm screen lets the owner change the provider
  for this send; `send_provider` in config is only the fallback for handoffs
  that name none. Built-in defaults:
  - `issue-report` for `issue`: prompt asks the agent to use the repo's
    `issue-report` skill with the intent text and attachment paths.
  - `enhancement-spec` for `enhancement`: prompt asks for the `enhancement-spec`
    skill.
  - `grill` for anything: prompt opens `/grill-me` on the intent.
  - `wayfinder` for anything: prompt opens `/wayfinder` with the intent as the
    destination.
  Sending shows the applicable handoffs and the owner picks one.
- **Where the agent starts.** `repo` in front matter always records the
  **main checkout** (the git common dir's top level), never a worktree: when
  an intent is created inside a toolbar whose workspace is a worktree, the
  tool resolves `git rev-parse --git-common-dir` and stores its parent. An
  intent is refined over days and the worktree may be gone by then, and under
  the toolbar design a worktree belongs to one agent. `send` runs
  `herdr workspace create` on that main checkout, then `herdr agent start`
  with the handoff's provider, then `herdr agent prompt`. The tool never runs
  `herdr worktree create` or `work` itself.
- **workbrokr repos.** Detected by `helmsmith.config.ts` at the repo root (a
  per-repo `workbrokr = true|false` in config overrides). For *building*
  handoffs (`issue-report`, `enhancement-spec`) the prompt is prefixed with:
  "This repo uses workbrokr. Before touching code, run `work start --area
  <area> --type <fix|feature> --mode <fullstack|frontend>`; take the area from
  the intent's tags or group, and ask me if it is unclear." For *planning*
  handoffs (`grill`, `wayfinder`) nothing is prefixed: planning needs no
  stack, and leasing a slot for a conversation is waste.
- **Attachments.** Any file dropped onto the intent (path pasted or typed) is
  copied into the assets folder. In the toolbar (text-only v1) attachments are
  listed by name and size; `o` opens one with the system opener (`open` on
  macOS). Inline images wait for the toolbar's image passthrough.
- **Capture from anywhere.** `herdr-intents add "<title>"` from a shell or a
  Herdr action creates a raw intent in an `Inbox` group, so capture does not
  require the UI. Standard input becomes the body when present.

## Goals

- Capture an intent in under five seconds: one key, a title, done.
- Refine without friction: the editor is the intent's Markdown, polish is one
  key, the result is a side-by-side accept/discard, never a modal essay.
- Organise by moving intents and groups with keys; the directory tree follows.
- Send in two keys: pick a handoff, confirm. The agent appears in a new Herdr
  workspace within seconds and the intent shows where it went.
- Work standalone in a plain terminal with the same files, so nothing depends
  on the toolbar being open.

## Non-goals

- Tracking after send. Status of the issue or map lives in the tracker.
- Replacing the editor. Editing is a plain text area with a few conveniences;
  for heavy editing `e` opens the file in `$EDITOR` and reloads on exit.
- Sync, sharing, or a server. Files on disk; use git if you want history.
- Inline images in v1 (toolbar limit).
- Running the *sent* agent's work inside this tool. The refinement
  conversation (decision 4b) lives here; the implementation agent started by
  `send` lives in its own Herdr workspace.

## Anti-goals

- Polish that rewrites without consent, or loses the owner's wording.
- A send that fires an agent at the wrong repo or with a truncated intent.
- Hidden state: everything visible in the UI must be in the files.
- A polish turn that blocks the UI; it streams in the background and can be
  cancelled.
- A conversation that is lost when the provider forgets its session; the
  transcript file is the record.

## Constraints

- Rust, ratatui 0.30, crossterm 0.29, same workspace as the toolbar; may share
  a small crate with it later (`herdr-tui-common`) if duplication appears,
  not before.
- `agy -p` facts, probed 2026-10-24 (agy 1.4.2): the prompt attaches to the
  flag, `-p='…'`; `--output-format json` returns
  `{"status":"SUCCESS","response":"…", …}`; `--json-schema <schema>` enforces
  structured output; `--print-timeout 60s`; `--agent <name>` and `--model`
  select an agent or model; `--disable-slash-commands` keeps the prompt literal.
  `agy agent` listed no agents on this machine, so the default uses no
  `--agent`.
- Herdr 0.9.5 CLI for send: `herdr workspace create`, `herdr agent start
  <name> --kind <kind> --pane <id>`, `herdr agent prompt`. Exact flags verified
  by the builder against `herdr --skill`.
- Inside the toolbar the tool receives `HERDR_*` and `HERDR_TOOLBAR_TOOL_ID`;
  standalone it may have none of them, and `send` then explains that Herdr is
  required.
- Attachments can be large; copy, never embed.

## How it works

### Layout

```
+------------------------------+------------------------------------------+
| Inbox (3)                    | # Family sees "Pending" on skipped folder|
| skoolscout                   | kind: issue   state: refining   ? 2      |
|   applications               |                                          |
|     ▸ Pending label on skip… | ## Observed                              |
|     ▸ Wizard resumes decided | A parent who skipped the report-card…    |
|   communications             | ## Expected                              |
| herdr-plugins                | …                                        |
|   toolbar                    | ## Attachments                           |
|     ▸ mouse support          | skipped-folder.png (212 KB)              |
|                              | ## Open questions                        |
|                              | - Which org saw this?                    |
+------------------------------+------------------------------------------+
 a add  n new group  m move  e edit  p polish  s send  ? help
```

Left: the group tree with intents as leaves, counts per group, a mark for
intents with open questions. Right: the selected intent rendered; `e` or
`Enter` switches the right pane to an editor on the same text.

### Editor

A plain multi-line text area over the intent's Markdown body (front matter is
edited through fields, not raw). Keys: normal typing, arrows, Home/End,
PageUp/Down; `Ctrl-s` saves; `Esc` leaves (prompting if unsaved); `Ctrl-p`
polishes the selection or, with none, the whole body; `Ctrl-e` opens `$EDITOR`.
Selection is `Shift+arrows`.

### Providers

A provider is a name plus two argv templates (polish, and the agent kind or
command `send` starts) and how to read its output. Built-in defaults, all four
CLIs present on the owner's machine on 2026-10-24; the builder verifies each
flag set against `<cli> --help` and records the exact reply field:

| Provider | Polish command | Reply | Notes |
|---|---|---|---|
| `agy` (default) | `agy -p={prompt} --output-format stream-json --print-timeout 90s --disable-slash-commands` | text events; final `response` (verified in json mode) | `--agent`, `--model` optional |
| `claude` | `claude -p {prompt} --output-format stream-json --include-partial-messages` | text deltas; final `result` (verify field) | `--model` optional; resumes must run from the same working directory |
| `codex` | `codex exec {prompt} --json -o {outfile}` | last message file `{outfile}` (`--output-last-message`, verify) | `--json` emits JSONL events; the last-message file is the reliable reply |
| `copilot` | `copilot -p {prompt} --output-format json` | JSONL events; reply in the final assistant event (verify field) | `--help` calls `--allow-all-tools` required for non-interactive mode. If copilot refuses `-p` without granting tools, copilot is **send-only** (an agent `send` can start) and is removed from the polish provider list; the tool never grants tools for polish. Builder verifies. |

All four stream (checked 2026-10-24): agy and claude with
`--output-format stream-json` (claude adds `--include-partial-messages` for
token chunks), codex with `exec --json`, copilot with `--output-format json`
(already JSONL). Streaming is how v1 runs every turn (decision 4b); the
provider table carries a `stream` argv and the event field that holds text.

Rules for every provider: polish runs with no tool permissions and no
repository access beyond the prompt text; a provider's "allow all" style flags
are never set by the tool. There is no schema enforcement: the fenced blocks in the reply are the
contract for every provider, which keeps streaming readable as prose.

```toml
[polish]
provider = "agy"               # agy | claude | codex | copilot | <custom name>

[providers.custom-llm]         # any other CLI
polish  = ["my-cli", "--prompt", "{prompt}"]
output  = "text"
send_kind = "claude"           # which `herdr agent start --kind` to use when sending via this provider
```

### Resuming a conversation (per provider, checked 2026-10-24)

| Provider | New conversation | Continue by id | Where the id comes from |
|---|---|---|---|
| `agy` | `agy -p=… --output-format stream-json` | `agy --conversation <id> -p=…` | `conversation_id` in the JSON result |
| `claude` | `claude -p … --output-format stream-json --session-id <uuid>` (tool picks the uuid) | `claude --resume <id> -p …` | the uuid the tool chose |
| `codex` | `codex exec --json …` | `codex exec resume <id> <prompt> --json` | session id in the JSONL events |
| `copilot` | `copilot -p … --output-format json` | `copilot --session-id <id> -p …` | session id in the JSONL events |

### Polish (conversation)

1. `Ctrl-p` in the editor, or `p` on a selected intent, opens the
   conversation pane for that intent: transcript above, an input line below,
   the intent's current text in a side column. The first turn is seeded by the
   kind's polish template: role, the kind's structure, the intent text, the
   selection if any, and the instruction to end every reply with a fenced
   `proposal` block and a `questions` list.
2. The tool runs the provider in streaming mode on a background thread,
   parses events as they arrive, and appends text to the transcript while it
   streams. `Esc` cancels the running turn (kills the subprocess).
3. The owner types the next message; the tool resumes by the stored id. The
   transcript persists to `<intent-slug>.chat.md`.
4. `a` shows a diff of the current body against the latest `proposal`;
   `Enter` applies it to the whole body, appends `questions` to `## Open
   questions`, and records `polished_at`. `d` dismisses the proposal but keeps
   the conversation. With no proposal in the latest reply `a` is disabled.
   The owner's text is never changed except by this step.
5. Providers that cannot stream or resume still work with degraded behaviour:
   a spinner instead of streaming, a fresh conversation each turn with the
   transcript re-sent as context.

### Send

1. `s` lists handoffs whose kinds include this intent's kind, plus the
   any-kind ones. Pick one.
2. Confirm screen shows: handoff, target repo directory, agent kind, the
   rendered prompt (scrollable). `Enter` sends, `Esc` cancels.
3. Tool runs the Herdr commands, writes `state: sent` and `sent_to`, and shows
   the new workspace and agent name. If any Herdr call fails the intent stays
   `ready` and the error shows.

### Files

```
~/intents/
  Inbox/
    2026-10-24-pending-label-on-skipped-folder.md
    2026-10-24-pending-label-on-skipped-folder.assets/
      skipped-folder.png
  skoolscout/
    applications/
      wizard-resumes-decided-application.md
```

```markdown
---
title: Pending label on skipped folder
kind: issue
state: refining
created: 2026-10-24T18:02:11-04:00
updated: 2026-10-24T19:40:03-04:00
repo: /Users/edwincruz/Development/Workspaces/skoolscout/skoolscout-com
tags: [applications, family]
polished_at: 2026-10-24T19:38:50-04:00
sent_to: null
---
## Observed
…
## Expected
…
## Steps
…
## Evidence
- skipped-folder.png
## Open questions
- Which org saw this?
```

Groups are directories. Moving an intent or group moves files. Slugs come
from the title; renaming rewrites the slug only when the owner asks.

### Config

`$HERDR_PLUGIN_CONFIG_DIR/intents.toml` inside the toolbar, else
`~/.config/herdr-intents/intents.toml`:

```toml
root = "~/intents"
editor = "$EDITOR"
send_provider = "claude"       # fallback for handoffs that name no provider

[polish]
provider = "agy"               # built-ins: agy, claude, codex, copilot; see Providers

[[handoff]]
name   = "issue-report"
kinds  = ["issue"]
provider = "claude"
prompt = "Use the issue-report skill on this finding. Do not file anything until you have confirmed the root cause with me.\n\n{intent}\n\nAttachments:\n{attachments}"

[[handoff]]
name   = "enhancement-spec"
kinds  = ["enhancement"]
provider = "claude"
prompt = "Use the enhancement-spec skill to write this up as a GitHub issue. Show me the draft before posting.\n\n{intent}"

[[handoff]]
name   = "grill"
kinds  = ["*"]
provider = "claude"
prompt = "/grill-me\n\n{intent}"

[[handoff]]
name   = "wayfinder"
kinds  = ["*"]
provider = "claude"
prompt = "/wayfinder\n\nDestination:\n{intent}"
```

Templates: `$CONFIG/templates/<kind>.md` for the body skeleton and
`$CONFIG/templates/<kind>.polish.md` for the polish prompt; built-in defaults
are written there on first run so the owner can edit them.

### Packaging: its own Herdr plugin

`tools/intents/` is a Herdr plugin of its own (`id = "jefelabs.intents"`)
with its own manifest, so it has its own config dir, a `[[build]]`, a
`[[panes]]` popup for quick capture and `[[actions]]`:

```toml
id = "jefelabs.intents"
name = "Intents"
version = "0.1.0"
min_herdr_version = "0.9.0"
platforms = ["macos", "linux"]

[[build]]
command = ["cargo", "build", "--release", "-p", "herdr-intents"]

[[panes]]
id = "add"
title = "New intent"
placement = "popup"
width = "60%"
height = 12
command = ["../target/release/herdr-intents", "add", "--popup"]

[[panes]]
id = "intents"
title = "Intents"
placement = "split"
command = ["../target/release/herdr-intents"]

[[actions]]
id = "add"
title = "Intents: capture"
contexts = ["workspace"]
command = ["../target/release/herdr-intents", "add", "--popup"]
```

Config lives in this plugin's config dir (`herdr plugin config-dir
jefelabs.intents`), and standalone runs read the same directory (resolved as
`~/.config/herdr/plugins/config/jefelabs.intents`, overridable with
`INTENTS_CONFIG_DIR`), so toolbar and standalone always see one config and
one root. The toolbar slot just runs the binary by absolute path:

```toml
[[tool]]
id      = "intents"
name    = "Intents"
icon    = "✎"
command = ["/Users/edwincruz/Development/Workspaces/jefelabs/herdr-plugins/target/release/herdr-intents"]
```

Replaces the Todo slot. `prefix+a` bound to `jefelabs.intents.add` captures
from any pane through the popup.

## Left to the builder

- Text area implementation (`tui-textarea` or hand-rolled) and selection
  model.
- Front matter parsing crate and slug rules.
- How the polish subprocess is supervised (thread + channel, like the
  toolbar's pty reader) and cancelled.
- Exact Herdr commands for send, verified against `herdr --skill`, and how
  the new workspace is named.
- Keymap details beyond the ones named here; help screen.
- Test strategy. Suggested minimum: unit tests for front matter round trip,
  slug and move, template rendering, polish result parsing (json and text), a
  fake polish command (shell script) and a fake `herdr` for send, ratatui
  TestBackend snapshots for the tree and the split view.

## Open items for the owner

None. Approved 2026-10-24 with the defaults: name `intents` / `herdr-intents`,
icon `✎`, root `~/intents`, "jsonp" read as JSON print-mode output.

Settled after the builder check (designer's calls, see the sections named):
`repo` is the main checkout; workbrokr detection and prompt prefix; each
built-in handoff names `claude` as provider; fenced blocks, not a JSON schema,
are the reply contract; proposals are always the full body; the tool is its
own Herdr plugin sharing one config dir; copilot becomes send-only if it
cannot run `-p` without granting tools.
