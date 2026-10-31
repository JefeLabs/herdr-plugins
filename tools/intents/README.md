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

Standalone runs use the same directory, so the toolbar and a plain terminal always see one config and one root (`~/.helmsmith/intents/spaces` by default). `INTENTS_CONFIG_DIR` overrides the config dir and `INTENTS_ROOT` the root, so a workspace can point at its own `intents/` folder through the toolbar's per-worktree config (`env = { INTENTS_ROOT = "..." }`).

**Moved from `~/intents`.** The first start after the update moves the folder once and leaves `~/intents` as a link to it, so running checks and agents sent before the move keep working; delete the link when nothing uses it. A custom `root =` or `INTENTS_ROOT` is never moved. If both folders already hold intents, the tool uses the new one and names both, so you can move what is left by hand.

## One workspace per project

Inside Herdr the tree is scoped to the workspace: a workspace named `skoolscout` sees only the root's top-level `skoolscout` group, with its sub-groups as the tree, and nothing from other projects' groups. Name a workspace after its project's group (`herdr workspace rename <id> skoolscout`) and keep one workspace per project. The name matches the group's folder whatever its case. A workspace whose group does not exist yet shows it empty, and the first capture there creates it. Quick captures, `a` with nothing selected and an empty list all add at the top of the workspace's group. Paths are unchanged: areas and `M` still use paths from the root (`skoolscout/discover-schools/org/candidates`). The name is read from Herdr when the tool starts, so restart the tool after renaming a workspace. Outside Herdr the whole tree shows. `INTENTS_SCOPE=<group>` chooses the group yourself, and `INTENTS_SCOPE=` (empty) shows the whole tree inside Herdr too.

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

The popup opens the same conversation as `a` in the tool, described next. It closes once the intent is saved, or when you cancel.

## Adding an intent

Adding is a conversation with the agent chosen by `m`, in its own view: the tree and the cards stay out of the way until you save or cancel.

1. **What is it?** `b` bug, `r` refactor, `f` feature, `i` idea. The type decides what the agent asks about: for a bug, what happens, what you expected, where and who sees it; for a refactor, which code, what hurts and what must not change; for a feature, who, what they can do, where and what success looks like; for an idea, what sparked it, who it is for and what would make it worth doing.
2. **Say it in your own words.** Type or paste whatever you have: an observation, a report someone sent, a half-formed idea. Typos are fine.
3. **The agent restates it first.** A one-line summary and a polished paragraph, then "Is that right?". `Enter` on an empty line accepts; type a correction to get a new restatement.
4. **Up to five questions, one at a time,** each built on your last answer and only about the main flow (who, what, where, what success looks like). The header counts them (`question 2 of up to 5`). `Enter` on an empty line skips one; `Ctrl-d` stops the questions and writes it up now; the agent may stop early on its own.
5. **The write-up.** The agent writes the title and the intent: Description, Preconditions, Acceptance criteria and Open questions. Criteria for the intent's own group come first; each other group the work touches gets a `### <group path>` heading with its own criteria, and those groups become the intent's areas, so it also shows as a `↳` row under each. Below the write-up, `save as:` shows the type (`Tab` changes it, and the agent says when it reads as another type), `in:` the group it is filed in, and `also:` the other areas. Type to ask for changes ("file it under skoolscout/admin/team-invites"); `Enter` on an empty line saves.

Where it is filed: pressing `a` on a group in the tree files it there unless you ask otherwise. From the card view or the popup the group is not known, so the agent proposes one from your existing groups, asking you if it cannot tell. A group the agent names that does not exist is ignored; the intent then goes where `a` would put it without a conversation (the selected card's group, or the top of the workspace's group).

Nothing is written until you save: `Esc` at any point cancels and leaves no file behind.

When the agent fails (an error, no reply, a write-up with nothing to save) or you stop it with `Esc` while it works, your words stay on screen with the reason. `Enter` tries the same step again; `m` picks another agent and model and comes back to retry with it; `Ctrl-s` saves what you have so far as a raw intent, titled by the first line you typed, with your answers under Open questions, to polish later with `p`; `Esc` discards. The saved intent starts as refining, with any skipped question under Open questions, and keeps the conversation, so a later `p` carries on with the same agent where the interview left off.

## Shape of an intent

Every intent body has four sections, seeded by the kind's template and kept by the agent's proposals:

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

The right-hand side shows the Description under a header, then six sections (Preconditions, Acceptance criteria, Demo Videos, Open questions, Pull requests, Artifacts) as an accordion: one is open and fills the rest of the pane, the others show only their title line with its count (`▸` closed, `▾` open). The Description takes what it needs, up to a third of the height, and scrolls. The Description pane and the agent's replies in the chat render Markdown: headings bold, bullets as dots, bold, italic and code spans styled, code blocks dimmed; the editor shows the raw text. Preconditions and acceptance criteria render their inline Markdown the same way (bold, italic, struck through, code reversed, link text underlined), wrapped with the styles carried across lines; an item that starts like a heading, list item or quote is shown as written. `Tab` and `Shift-Tab` move focus between the tree, Description, Preconditions, Acceptance criteria, Demo Videos, Open questions, Pull requests and Artifacts; `Esc` returns to the tree. A section opens when it gets focus (by `Tab` or a jump key such as `Q`, `C` or `V`) and stays open while focus is on the tree or the Description, including as you move to another intent; Acceptance criteria is open to start. In Description, `j`/`k` scroll and `Enter` edits. In Acceptance criteria, `j`/`k` choose and `Enter` or space toggles the checkbox, staying in the pane so several can be ticked. In Open questions, `j`/`k` choose and `Enter` opens an answer box right under the question; type (or paste), `Enter` saves, `Esc` cancels. Answers are stored as an indented `→` line under the question, answered questions no longer count as open (the `?N` mark counts only unanswered ones), and the agent is told to keep them with their answers rather than re-ask. A polish polishes everything you wrote, answers included: it fixes spelling, grammar and unclear wording in the description, preconditions, criteria, questions and `→` answers while keeping their meaning and every decision in them (`Ctrl-a` shows the diff before you apply). A reconcile never does this, since it must keep criteria word for word. The polish templates are copied into the config folder once; a copy you never edited is brought up to the current default when the tool starts, and an edited one is never overwritten, so this rule is also sent with every turn rather than relying on them. `Q` jumps to the questions; `C` jumps to the criteria and, pressed again, to the preconditions. In any of the three list panes `a` adds an item there by hand; a polish drafts all three lists for you. Preconditions are assumptions, not checks: when an intent is sent, the receiving agent is told to verify each one against the code before building anything and to ask about those it cannot settle.

The Artifacts pane (`V`) lists what the agents produced for the intent: specs, plans, ADRs, decision records and tickets, one `- kind: target` line each under `## Artifacts`. `pr:` and `demo:` lines live in the same place but are listed in their own sections, Pull requests and Demo Videos. A relative path is read from the intent's `repo:` first (which may be a worktree or a subfolder), then from the repo's main checkout and each of its worktrees; with no repo, from beside the intent file. `~` and absolute paths are read as written. Every handoff prompt asks the receiving agent to append such a line for each file or issue it creates. `Enter` reads a Markdown file right here (`o` opens it outside, as do tickets and URLs), `a` adds a line by hand. App-demos are artifacts too, listed in the Demo Videos section: `- demo: <story-id> (criteria: 1, 4)` links a story from the repo's `app-demos/stories/*.yaml` and names the acceptance criteria it shows. `Enter` on it reads the story here (title, actor, host, the stories it runs after, and its steps with their on-video captions), `o` plays the newest recorded video from `app-demos/videos/<story>/`; without one it opens the demos site at the story while that runs (`make demos-open` in app-demos), and otherwise says how to record it (`make demos STORY=<story>`), and each criterion a linked demo shows gets `▶` beside its checkbox. Hand-offs ask agents to add these lines for the stories they write, and stories that match the intent the same way (two shared words, one of them in the story id) show up as found, so `L` links one. Words match on their stem: plurals, -ing and -ed, and a final e fold away (contact and contacts, skip and skipped, schedule and scheduling), but a word is not matched by every longer word that starts with it (specific is not specification), and a word the title repeats counts once. The pane also scans the intent's repo (`docs/superpowers/specs`, `docs/superpowers/plans`, `docs/adr`) for files that share at least two words with the intent's title, at least one of them in the file name (the first heading counts for the rest), and lists them dimmed as *found*, with a `?` after the kind; `Enter` reads them the same way and `L` links one for good. The scan runs once per selected intent and again after `r`.

## Keys

The Pull requests section lists each `pr:` line with its state from `gh` (`merged`, `open`, `closed`, or `…` until `gh` has answered) and its title, and counts them in its title (`Pull requests (6/7 merged)`). Pull requests the repo search found by the intent's spec and plan names, but no `pr:` line names, follow dimmed as found (`merged? #5840 …`); `L` links one, and it keeps the state and title the search gave until `gh` is asked about it. `Enter` or `o` opens a pull request in the browser. In Pull requests and Demo Videos, `a` adds a line of that kind without typing it (`#123`, or `story-id (criteria: 1, 4)`).

Browse: `j`/`k` move, `h`/`l` collapse/expand a group (sub-groups start collapsed and open one level at a time), `Enter`/`e` edit, `a` add an intent by conversation, filed in the selected group (see Adding an intent), `n` new group, `M` move the selected intent or group (type the target group path), `K` cycle kind (bug, refactor, feature, idea), `R` ready (refused while any question is unanswered), `U` back to refining, `p` polish with the agent, `m` choose the agent and model, `Tab` move between panes, `t` rename (a bug's title says what goes wrong and when, a feature's who can do what, an idea's the idea itself), `Q` jump to questions, `C` jump to criteria, `s` send, `A` attach a file (type or paste its path), `o` open the first attachment, `D` delete an intent that has not been sent (after a y/n question; see below), `E` capture a bug again (see Evidence for bugs), `X` check a sent intent's acceptance criteria (see Completion check), `F` file a follow-up of a sent intent (see Follow-ups), `r` rescan, `q` quit. A file that fails to parse shows as a `!` row with its error.

`D` deletes the selected intent after asking. Nothing is lost for good: the intent's file, its conversation (`.chat.md`), its attachments and its agent folder move to `.trash/<date-time>-<name>/` in the intents folder, with `from.txt` naming the group it came from; move the files back to restore it. An intent that has been sent is refused, since an agent is working from it. With the mouse, a click selects a row in the tree, a click on the selected group opens or closes it, and the wheel moves the selection.

Editor: type, `Shift+arrows` select, `Ctrl-s` save, `Ctrl-p` polish (on the selection if any), `Ctrl-e` open in `$EDITOR`, `Esc` back.

`p` (or `Ctrl-p` in the editor) polishes at once with the agent and model you chose last. `m` changes them: a provider list, then that provider's models if it has any (for agy the model names carry the thinking level: high, medium, low), with the current choice preselected. The choice is saved in `last-used.toml` in the config dir and used for every polish, and for reconciles, until you change it; before the first `m`, polish uses `polish_provider` from the config.

Refine (chat): type and `Enter` to send the next message, `Esc` cancels a running turn or leaves, `Ctrl-a` shows a diff of the latest proposal, `Ctrl-d` dismisses it, `PgUp`/`PgDn` scroll the transcript. Plain letters are always typed, so a message may start with "a" or "d". In the diff, only `Enter` applies; `Esc` goes back; `PgUp`/`PgDn` scroll.

Send: pick a handoff, then on the confirm screen `Tab` cycles the provider, `j`/`k` scroll the prompt, `Enter` sends, `Esc` cancels.

## Refining with an agent

Refinement is a conversation, not a one-shot rewrite. The first turn is seeded by the kind's polish template (`templates/<kind>.polish.md`), the intent text and, when opened on a selection, a focus line. Replies stream in as they are written. Every reply is asked to start with a line saying what the intent is, judged from what you wrote rather than from the kind it is filed under: `bug: <what goes wrong> when <condition>`, `refactor: <what changes> in <where>`, `feature: <who> can <what>` or `idea: <the idea>`. When the agent cannot tell, it keeps the filed kind and asks "Is this a bug, a refactor, a feature, or an idea?" among its questions. When you apply, the phrase becomes the title and the line sets the kind (bug is stored as `issue`, feature as `enhancement`, refactor and idea as themselves; the screen shows bug, refactor, feature or idea; an older `capability:` line still reads as a feature), so a bug captured as an idea is handed off as a bug; `K` still changes the kind by hand. Every reply is also asked to end with a fenced ```` ```proposal ```` block holding the complete rewritten intent and a `questions:` list; `a` applies the latest proposal after a diff and merges the questions into `## Open questions`. A reply without a proposal cannot be applied, so chat text never lands in an intent. Your text changes only when you apply.

The conversation id is stored per provider in the intent's front matter and resumed on later turns, and a transcript is appended to `<intent>.chat.md`.

### Providers

| Provider | Polish | Resume | Status |
|---|---|---|---|
| `agy` (default) | `agy -p=… --output-format stream-json --sandbox --print-timeout 0s` | `--conversation <id>` | verified; models include `gemini-4-argon-high/medium/low` |
| `claude` | `claude -p … --output-format stream-json --include-partial-messages --session-id <uuid>` | `--resume <id>` | verified |
| `codex` | `codex exec --json …` | `codex exec resume <id> …` | unverified (account returned 402) |
| `copilot` | `copilot -p … --output-format json` | `--session-id <id>` | unverified (policy denied) |

agy gets `--print-timeout 0s` so it waits for a long turn to finish: with a print timeout it gives up mid-turn and then never exits. A turn ends when the provider reports its result (agy's and claude's `result`, codex's `turn.completed`, copilot's `session.idle`), not when the process exits: agy can stay running after its result, so the tool stops any provider still running at that point (SIGTERM to its process group, then SIGKILL). Any other CLI goes in `intents.toml` as a custom provider. Polish runs with no tools and no repository in reach: the built-in argv use `--permission-mode plan --tools "" --strict-mcp-config` (claude; the MCP flag is left off when your organization manages MCP servers on the machine, `/etc/claude-code/managed-mcp.json` or `/Library/Application Support/ClaudeCode/managed-mcp.json`, since claude refuses it then; `strict_mcp = true` or `false` under `[providers.claude]` forces it either way), `--sandbox` (agy; its `--mode plan` is ignored while slash expansion is off), `--sandbox read-only` (codex) and `--deny-tool '*'` (copilot), and every turn runs in a neutral directory under the root (`<root>/.polish/<intent>`), never in your checkout. The tool never passes `--allow-all*` or `--dangerously-skip-permissions`.

```toml
polish_provider = "agy"

# tune a built-in without restating it
[providers.agy]
model  = "gemini-4-argon-high"          # or extra = ["--model", "…"]; `m` still lets you change it
models = ["gemini-4-argon-high", "gemini-4-argon-medium", "claude-opus-5-5-high"]   # what `m` offers

[providers.mine]
polish = ["my-cli", "--prompt", "{prompt}"]
resume = ["my-cli", "--resume", "{conv}", "--prompt", "{prompt}"]
output = "text"
send_kind = "claude"
```

## Sending

`s` refuses while the intent is still `raw` or has an unanswered open question, for every handoff, and the check runs again on the file as it is when you pick the handoff and when you confirm, so a question added in the meantime still stops the send: open questions are the happy-path details an agent needs, and the refinement conversation is told to ask only those. Edge cases are the receiving agent's to find; every hand-off prompt says so. Otherwise `s` lists the handoffs that apply to the intent's kind. Built in, all starting a Claude Code agent because they invoke Claude Code skills:

| Handoff | Kinds | What the agent does |
|---|---|---|
| `issue-report` | issue | the repo's `issue-report` skill, confirms the root cause before filing |
| `enhancement-spec` | enhancement | the repo's `enhancement-spec` skill, shows the draft before posting |
| `refactor-spec` | refactor | the repo's `enhancement-spec` skill, filed as an issue with the `refactor` label, shows the draft before posting |
| `grill` | any | `/grill-me` on the intent |
| `wayfinder` | any | `/wayfinder` with the intent as the destination |

Send opens a new tab in the Herdr workspace the tool runs in (`herdr tab create --workspace <current> --no-focus`, labelled with the intent's title) on the intent's repo (always the main checkout, never a worktree), then runs `herdr agent start` with the handoff's provider in that tab and `herdr agent prompt` with the rendered prompt. Your focus stays where it is; `g` focuses the agent. Outside Herdr (no `HERDR_WORKSPACE_ID` and no plugin context) it creates a new workspace instead. If the agent fails to start, the message names the tab (or workspace) left open. In repos that use workbrokr (`helmsmith.config.ts` present) the building handoffs get a `work start` instruction prefixed; planning handoffs do not. If a step fails the intent is left as it was and the message names the step.

## Affected areas

An intent lives in one group but may affect others. List them in its front matter, or press `+` (in the tree or the list) and type group paths separated by commas; an unknown group is refused, an empty list clears them:

```yaml
areas:
  - skoolscout/discover-schools/org/candidates
  - skoolscout/discover-schools/family/schedule-meeting
```

The tree then shows the intent as a dim `↳` row under each affected group; that row acts on the real intent, and a group's count reads `own +affecting` (e.g. `(3 +2)`). In the list, the path row under the title reads home · other areas, and a hand-off tells the agent the other areas, since a change that spans them may need one work area each.

Areas are also suggested from the work itself: a pull request found for the intent whose branch follows workbrokr's `<type>/<area>--<task>` shape names an area (`feature/org/candidates--…` gives `org/candidates`), and when exactly one group's path ends with it, the card shows `suggested: …`. `+` opens with suggestions already added, so Enter confirms them; nothing is added without that.

## Criteria grouped by area

Acceptance criteria can be grouped under `### <area path>` sub-headings, one per affected area. The criteria pane draws each sub-heading as a bold header with its own count (`skoolscout/discover-schools/org/candidates (1/3)`), with ungrouped criteria (the intent's own group) first; selection and scrolling still move between criteria. Grouping happens in two places: refinement groups the criteria when the intent lists areas (`+` sets them), and a reconcile groups them by the intent's areas plus the areas its pull requests suggest. Grouping keeps every criterion's wording, so nothing turns new.

## New acceptance criteria

Criteria someone else adds or rewords (an agent editing the file, a reconcile) are new until you acknowledge them. The criteria pane draws them bold with `+` and counts them in its title (`Acceptance criteria (2/10 · 7 new)`); `y` acknowledges the selected one, `Y` all of them. While any are unacknowledged the intent carries `*` before its title, in the tree and on its card, so the last `y` clears it. Your own criteria never count as new: everything present when you send, the criteria of a refinement proposal you apply, and criteria you add with `a` or type in the editor are recorded as seen (`acknowledged:` in the front matter holds a key per criterion's wording, so rewording one makes it new again while ticking it does not). For an intent sent before this record existed, criteria found in its refinement transcript count as seen; with no transcript, all do.

## Reconcile after merge

When every pull request of a sent intent has merged (one closed without merging is ignored), the tool reconciles it with what shipped: in the background it reads the spec, the plan names, the merged pull requests' titles and descriptions and the `issue:` follow-ups, and asks your refinement agent (the last one used) for a rewrite. The rewrite corrects the acceptance criteria to match the shipped behaviour (grouped under `### <area path>` when the work spans areas), names any dropped criterion under Open questions, keeps Description, Preconditions and Artifacts, and leaves follow-ups out of the criteria. Ticks stay yours: after a reconcile a criterion is ticked only if one with the same wording was ticked before, whatever the agent marked, so new and reworded criteria wait for you to verify them. The agent is asked to copy a criterion that still holds word for word and to put new detail about it in an indented sub-bullet, so your ticks carry over. The groups its criteria end up grouped under (`### <area path>` headings that name an existing group other than its own) are added to `areas:`, so the intent also shows as a `↳` row in each of them.

The rewrite is applied straight away; the previous version is kept beside the file as `<name>.before-reconcile.bak` (older ones as `<name>.before-reconcile.1.bak` up to `.4.bak`), and `u` puts it back, one reconcile per press. Whatever the rewrite leaves out of the old version is put back: a `## ` section it dropped (a Decisions list, say) returns where it was, and an artifact line it dropped is added again. A demo line it kept as it was is renumbered to point at the same criteria by their wording, and a criterion that is gone drops out of it. The intent is marked unread: its card and tree row show `*` before the title until you open it (select it in the tree, or `Enter` on its card). Each merged set is reconciled once (`reconciled_for` in the front matter), so a later merge reconciles again but a restart does not. If a reconcile fails (the agent returns no proposal, say), the reason stays on the status line while that intent is selected, and `r` retries it. A reconcile that runs longer than 10 minutes is stopped and reported the same way, and quitting the tool stops any reconcile still running. `R` and `U` leave a sent intent sent, since its progress tracks the work from then on. Pull request states are checked about once a minute in either view (tree or list), so a merge reconciles wherever you are; an intent whose merge happened while the tool was closed is reconciled shortly after it starts.

## Spaces and the run guide

Evidence capture, the completion check and (next) demo videos run your app in a browser. How to run it belongs to the project, which is often several repos, so it lives in the project's **meta repo**: one of its repos, or an extra one made for the purpose, with the others beside it as sibling folders. The meta repo's `.helmsmith/` folder holds two shared files, committed so teammates get the same.

`.helmsmith/space.toml` lists the project's repos, relative to the meta repo. A listed repo that is missing stops a run with the `git clone` to bring it in; nothing is ever cloned for you.

```toml
repos = [
  { path = ".",      git = "git@github.com:acme/shop-meta.git" },
  { path = "../api", git = "git@github.com:acme/shop-api.git" },
  { path = "../web", git = "git@github.com:acme/shop-web.git" },
]
```

`.helmsmith/run.md` says how to run the app (the older name `capture.md` is still read): front matter for the tool, then instructions for the agent.

```markdown
---
start: docker compose up -d db && npm --prefix ../api run dev & npm --prefix ../web run dev
url: http://localhost:{port}         # {port}: a free port the tool picks and passes as PORT
ready: /health                       # polled until the app answers (default: the url itself)
ready_timeout: 600                   # seconds (the default); also the limit for each seed
seed: ["npm --prefix ../api run seed:demo"]
stop: docker compose down            # optional
stop_timeout: 60                     # seconds `stop` may take (the default)
logs: [../api/logs/app.log]          # what a completion check may search
splash:                              # how a story video's cards look (optional)
  logo: brand/logo.png               # png, svg or jpeg in the meta repo, at most 256 KB
  background: "#142b57"              # colors: #rgb or #rrggbb only
  accent: "#59b7ff"
  text: "#fafafa"
---
Sign in as demo@acme.test (password in the seed). Leave the billing pages alone.
```

Every command runs with `sh -c` from the meta repo. `start` runs in its own process group for the whole run: it may stay in the foreground, or exit 0 leaving the app running (`docker compose up -d`); one that fails first ends the run at once with its last lines. The tool waits until the app answers, runs each `seed` (each must succeed within `ready_timeout`), and at the end runs `stop` (cut off after `stop_timeout`) and ends `start`'s whole group, also when the run is cancelled or fails. An app a crashed run left behind is ended by that intent's next run, and only if it is still the very process that run started. Without `{port}` in `url`, runs in a space take turns (a second is refused with the address in use), and a fixed address that already answers before `start` is refused too: the tool never seeds, drives or stops an app it did not start. A check that could not run (its app never answered, a seed failed) says why in the Evidence section. Log paths must stay inside the space's folder: no `..` out of it, no symlinks. The app runs from whatever your repos have checked out; the tool never switches branches.

**A story video's look.** `splash:` gives the cards and captions of every story video the project's logo and colors. The logo is read from the meta repo (a relative path, nothing outside it, no symlink out of it) and goes to the page as image data, never as markup; a color must be `#rgb` or `#rrggbb`, since it goes into the overlay's style. Anything the tool can't use is skipped, and each story's `run.log` says what and why. Without the block, the cards are dark with white text and no logo.

**Which space.** The space is the Herdr workspace (and its group under `~/.helmsmith/intents/spaces/`). Your own config keeps where each space's meta repo is, in `spaces.toml` beside `intents.toml`:

```toml
[spaces.shop]
meta = "~/code/shop-meta"
storage = "profile"      # where the space's intents live; the default
```

It is filled in for you the first time the tool starts in a workspace whose folder has `.helmsmith/space.toml`.

**Where a space's intents live.** `storage = "profile"` (the default, also with no line) keeps them in your own folder, `~/.helmsmith/intents/spaces/<space>/`. `meta` (the meta repo's `.helmsmith/intents/`, shared through git) and `remote` (a REST API, with `url = "…"`) are placeholders for now: the space then shows no intents, says `meta storage is not built yet` (or `remote …`), and refuses to add one, rather than writing them anywhere else. Outside a space, the whole tree leaves such a space's group out and names it. Any other word is an error that names `spaces.toml`. The run guide is looked for in the space's meta repo, then in the intent's own repo; with neither, the tool says where to add one.

**Waiting for the pull.** In a project run this way, the automatic check after a merge waits until your checkout has the merged code: every merged pull request's merge commit (from `gh`) must be in the `HEAD` of the clone it merged into, which is the intent's repo for `#12`, or for `owner/repo#12` the space repo whose git URL in `space.toml` names that repo. A pull request in a repo the space does not list does not hold the check back. Until you pull, the card reads `· waiting for a pull` and the merge is not counted as checked. `X` checks whatever is checked out.

**Skoolscout's way.** A guide with `default_area` in place of `start` keeps skoolscout's own steps: a `work` slot, e2e seed stories and logs in the slot's worktree, as the next sections describe.

## Stories and demo videos

A feature or a bug carries the user stories its demo videos will show, in a `## Stories` section after the acceptance criteria. The detail view lists them in the same order, Acceptance criteria then Stories, since a story names the criteria it shows by number:

```markdown
## Stories
### Mark reinstates Maya's withdrawn application
- persona: Mark, a tenant admin
- shows: 1, 3
- starting data: Maya has a submitted application, then withdrawn
1. Open Maya's application from Applications
2. Choose Reinstate and confirm
3. See the application back under Submitted
```

`shows:` names acceptance criteria by number, counted from 1 across groups (a reconcile or fold that renumbers criteria renumbers it too). Polish drafts the section: while open questions remain, each story has only its name, persona and `shows:`, with `drafts wait for the open questions`; once every question has its answer it adds the starting data and the steps. Answering the last open question with `Q` opens the chat on that drafting turn. The section goes to the coding agent with the rest of the intent, so it builds knowing what will be shown. Refactors and ideas have no stories.

**The run guide names the personas.** Its instructions get a `## Stories` section; the guide's rules win over a story:

```markdown
## Stories
### Personas
- Mark: tenant admin, signs in as mark@example.test
- Pat: a parent, signs in as pat@example.test
### Setup may create
- applications and notes, never people
```

Drafting picks personas from that list (`unknown` when none fits). With no such section stories are still drafted, but none is recorded: the row says `the run guide lists no personas for stories`.

**When videos are recorded.** Once the completion check passes (no criterion failing or missing, counting every check so far; "can't check" does not hold it back), the same run records each story with no kept video, and records again any story showing a criterion that check looked at. Each story gets its own confined agent and browser session, which signs in, sets up the starting data through the app, rehearses the steps once unrecorded, sets up fresh data, then calls `take_start`. The video opens on a splash, drawn before the recording starts so that it is also the video's first frame, the cover Slack, Messages and Finder show: the guide's logo, the intent's title, the story and its persona, each criterion the story covers in the intent's own words (as plain text: Markdown's code ticks, bold and links are left out, and a story named as a slug, `mark-reinstates-maya`, reads `Mark Reinstates Maya`), and a footer with the intent's id, its version and when it was recorded (`k7q2 · v3 · recorded 2026-10-31 10:42`). The splash stays 4 seconds plus 2 per criterion, at most 12. The take then shows each criterion's own text as a caption when the agent says it is visible, and ends on a card listing what it showed (`✓ 1. …`), with the same footer. A video is kept only when every criterion the story shows got its caption; a re-take that fails keeps the earlier video. Nothing about signing in is ever on video. `r` in the Stories section records one story at any time (see below).

Each story's folder is `<stem>.assets/stories/<n>-<name>/`: `video.mp4`, `steps.txt` (the take's browser steps), `captions.json`, `story.json` (how the last take ended) and `run.log` (the agent's last word and any tool errors: read it when a take fails). A take records into `take.mp4` and becomes `video.mp4` only when it ends with `take_stop` and showed every criterion. Recording uses agent-browser's video, which needs ffmpeg with libx264 on `PATH` (`agent-browser doctor` checks); the pointer is drawn on the video.

**The Stories & Videos section** lists each story with its video; a kept video's row names the criteria it shows (`▶ 1. Mark Reinstates Maya · video (criteria: 1, 2, 7)`), and those criteria get `▶` after their checkbox in Acceptance criteria, as an app-demo's do. The criteria come from the take's own record, so editing a story's `shows:` later can't claim what its video doesn't show. A story named as a slug reads as words.

**Keys in the Stories & Videos section** (Tab to it): `d` drafts the stories when there are none and re-evaluates them when there are: in the background, the agent reads everything known about the intent (every section of it, decisions included; the specs, plans and ADRs it links, only files inside its repo and each capped; its pull requests; the latest completion check; a bug's captured evidence; the end of its polish conversation), keeps the stories that still fit, fixes or drops the ones that don't, and adds stories for the criteria nothing shows. Only the `## Stories` section is rewritten; the one it replaces is kept as `before-draft.md` in the stories folder. For an idea or a refactor it says to change the kind with `K` first. `o` or Enter plays the video, `c` copies the file (it pastes into Slack or Messages on a Mac; elsewhere the path), `Y` copies its path, `O` shows it in Finder, `r` records it now (again, if it has a video), whatever the last check says: when that check hasn't passed, the status line names it (`recording story 2 (the last check (08:57) hasn't passed): …`) without waiting for it. An intent has one run at a time, so while one is going (a recording, a check or a bug capture), `r` queues the story instead: a recording records it next and a check after itself, on the app they already have, and a capture leaves it to a run of its own once it ends. `r` on a queued story takes it off the queue. A queue a run never reached (it crashed, say) starts its own run once nothing else of that intent runs, and a run that can't start its app says why on each queued row. Different intents' runs go side by side, each on its own app. Each row reads `<n>. <name> · video`, `recording…`, `queued`, `criterion 3 didn't show`, `couldn't record: <why>` (when the agent never started the take, `<why>` is its own one-line reply, such as `Reinstate can't reach Accepted on this stack…`; the full reply is in the story's `run.log`), `drafts wait for the open questions` or `not recorded yet`.

## Evidence for bugs

A bug can carry proof: a screenshot or a video of it happening on a local stack, recorded by an agent, and the same steps again once the fix has merged. It needs a run guide (see Spaces and the run guide; with skoolscout's work mode, also `work` (workbrokr)), a `claude` recent enough to have `--restricted` and `--permission-mode dontAsk` (an older one is refused before any stack starts), and `agent-browser` (`npm i -g agent-browser` or `brew install agent-browser`, then `agent-browser install` for its browser).

**The question.** After the last question about a bug, before the write-up, the add view asks `Capture evidence? s screenshot · v video · Enter skip`. It asks once, and only for bugs. When the repo has no capture guide, or `claude` is not installed, the line says why instead and `Enter` continues.

**In the background.** Each capture runs in its own process (`herdr-intents capture-run`), so the write-up goes ahead, you can save, and closing the popup or quitting the tool leaves it running to its end. The intent's Evidence section shows how far it got (`starting a stack in org/social`, `seeding: kathia-signup`, `the agent is reproducing it`), then the result. In order, a capture in skoolscout's work mode (any other project runs its guide's commands instead, as above):

1. starts its own task effort, `work start --area <area> --task evidence-<n> --type fix --mode fullstack` with `WORK_SESSION=capture-<name>`, trying the area the intent's group ends with (its last two path segments), then the guide's `default_area`. It never joins an effort or merges into one;
2. runs the guide's seed stories on its own slot (`make -C <worktree>/app-test-e2e-runner e2e-story STORY=<story> FAST=1`, with `SLOT` from `work env`);
3. runs `claude` with the guide, the report and the app's address, and no shell: `--restricted --permission-mode dontAsk --tools ""`, with one MCP server (`herdr-intents capture-mcp`). Its tools are `browser` plus `record_start`/`record_stop` (video) or `screenshot`. `browser` takes one agent-browser command from a fixed list (open, snapshot, click, fill, type, press, select, scroll, wait, get, find and the like) with only the flags each needs; `open` goes only to the stack's own address, and `eval`, `wait --fn`, `read`, `tab`, `connect` and every global flag (`--session`, `--profile`, `--executable-path`…) are refused. Everything runs in the capture's own agent-browser session, closed when the agent ends, and the files go only to the evidence folder. The report reaches the agent as data between markers, never as instructions. An agent still running after 20 minutes is stopped;
4. ends with `work finish` for its effort whatever happened. When `work finish` refuses (a dirty worktree, say), the capture's row says so with the command to run.

Deleting the intent (`D`) or cancelling the conversation (`Esc`) stops its capture, which still finishes its effort. A capture whose process ended early (a crash, a restart) shows as `! … interrupted`; an interrupted "after" runs again.

The agent's last line is `reproduced: …` or `couldn't reproduce: …`; a capture without one counts as failed. Captures are 1280×800.

**The capture guide (work mode).** A repo opts in with `.helmsmith/run.md` (or the older `capture.md`): front matter, then the instructions the agent gets (how to sign in, as whom, what to leave alone). No guide, no capture. In skoolscout's work mode the front matter is:

```markdown
---
default_area: org/social     # where to start a stack when the intent's group names no area that starts
seed: [kathia-signup]        # e2e stories run before the agent starts
url: https://demo-org.skoolscout.com.local:{port}   # {port}: the stack's frontend port
---
Sign in at /auth/sign-in: press Meta+U and pick the person…
```

Keep passwords out of it: the agent signs in the way a person does on a local stack.

**The Evidence section.** It follows Artifacts in every intent's detail pane (`Tab` reaches it). One row per capture: `before` or `after`, `video` or `screenshot`, and the agent's line, marked `…` running, `✓` reproduced or fixed, `✗` still broken, `·` couldn't reproduce, `!` failed or interrupted; `⚠` adds anything to act on, such as a `work finish` that failed. `Enter` opens the file in Chrome (on Linux, with `xdg-open`). `c` copies the file itself: on macOS, ⌘V into a GitHub comment uploads it; over SSH, or off macOS, it copies the path. `Y` copies the path. `E` (here, in the tree or on a card) captures the selected bug again: a "before" until one reproduced it, then an "after". It refuses while a capture of that intent is running.

**After the fix.** When every pull request of a sent bug has merged and its "before" reproduced the bug, the completion check that follows the reconcile (see Completion check) also runs the same steps on its fresh stack: an "after", once per merged set of pull requests (a bug with no acceptance criteria gets the after on its own). Its row reads `✓ after · … · fixed: …` or `✗ after · … · still broken: …`. Besides `pr:` lines and the spec and plan names, pull requests are found through the intent's `issue:` lines: the ones GitHub links as closing the issue ("Fixes #n").

**Where the files are.** Until the bug has an issue, its captures sit in its agent folder, `.polish/<name>/evidence/`: `before.mp4` or `before.png`, and `before.json` with the result and the `browser` calls the agent made (the same for `after`). Once an `issue:` line names the filed issue, they move into the intent's attachments, `<name>.assets/evidence/`. A hand-off lists the captures wherever they are, so the agent filing the issue can use the screenshot as the issue's picture and ask you to paste the video. `D` takes them to the trash with the intent.

## Completion check

A sent intent reads `✓ complete` when every acceptance criterion is ticked, every pull request has merged, no missing item is open and you have read every criterion. A completion check does the ticking for you: an agent tries each criterion in the running app, on a fresh local stack, and ticks the ones it can prove.

**When it runs.** `X` (in the tree, on a card, or in any section of the intent) checks the selected sent intent now. While a check runs, its card and the intent's header say what it is doing: `checking 2/9 · seeding: kathia-signup`, `· the agent is checking the criteria`, then `· recording story 2: …` for the stories that follow; a story recorded with `r` reads `recording · …`. When a check you started with `X` ends, the status line says how: it passed, what holds it back, or why it couldn't run (the card then reads `· check couldn't run`).

**Programs it runs.** A check runs `work`, `make`, `claude` and `agent-browser`. Herdr starts the tool without your shell's setup, so programs installed with `npm i -g` under nvm (`work` among them) are not on its `PATH`: the check puts nvm's default Node (`~/.nvm/alias/default`, or `NVM_DIR`) first on the `PATH` of everything it runs. `WORK_BIN_PATH`, `MAKE_BIN_PATH`, `CLAUDE_BIN_PATH` and `AGENT_BROWSER_BIN_PATH` still name a program exactly. A check also runs by itself once every pull request has merged and the reconcile for that set has been applied, once per merged set and one check at a time per repo. The reconcile goes first because it may reword the criteria. The first time the tool starts with this feature, every merged set already reconciled counts as checked, so there is no backlog of stacks.

**How it checks.** It uses evidence capture's machinery: its own process, its own `work` stack seeded by the guide, and the same agent with no shell. Its tools are:
- `browser`;
- a `screenshot` per criterion;
- when the guide lists `logs:` (paths under the stack's worktree, such as `.logs/backend.log`), a `logs` tool that searches those files. It returns only what was logged after the agent started, so the seed's own output proves nothing. A log path that is a symlink or leads out of the worktree is refused.

The agent ends with one line per criterion:
- `<n>: confirmed: …`;
- `<n>: fails: …` (built, but wrong);
- `<n>: missing: …` (not built);
- `<n>: can't check: …`.

A `confirmed` counts only with proof kept in that run (the criterion's screenshot or log lines); otherwise it reads `can't check: no proof kept`. For a criterion that fails or is missing the agent keeps proof of what it saw instead, which goes to the missing item. A check's agent may run 10 minutes plus 3 per criterion (5 more when it records a bug's after). A run whose agent gives no answers at all (it timed out, or claude failed) applies nothing, so the check runs again later.

**Ticks.**
- A check ticks the criteria it confirmed and records that it did.
- It never unticks one you ticked. If one of yours fails, it shows `✗` beside your tick and still files a missing item.
- It unticks only its own ticks, when a later check fails them.
- When you untick a check's tick, no check ticks that criterion again.

**Missing items / Bugs.** Each criterion that fails or is missing becomes a new intent in the same group:
- a bug for `fails`, a feature for `missing`;
- created as refining, with the criterion as its one criterion and the proof in its attachments;
- listed in the original's **Missing items / Bugs** section, after Pull requests: `✓` closed, `→` sent, `○` not yet. Enter goes to it.

A later check that fails the same criterion adds its proof to the open item instead of filing another; one that fails after the item was completed files a new one. Deleting a missing item that is still open (`D`) dismisses that gap: later checks do not file it again. Deleting a done one only removes it, and a later failure of its criterion is filed anew. Once an item closes (its criterion ticked, by its own check or by you), the original re-checks just that criterion, once.

**Where the results show.**
- **Acceptance criteria:** the latest result sits beside each checkbox (`✓` confirmed, `✗` fails or missing, `?` can't check). The status line gives the selected criterion's finding and date, and `o` opens its proof.
- **Evidence:** one row per criterion (`check · 3 · ✓ …`), with Enter, `c` and `Y` as for captures.
- **The card:** reads `checking 2/5` while a check runs. It reads `✓ 5/5 · 2 new to read` when every criterion is ticked but some are unread (`Y` reads them), and adds `· 1 missing` while items are open.

Results live in `<name>.assets/checks/` (`latest.json` and the proof files).

## Follow-ups

A refinement asked for after an intent shipped is a follow-up: its own intent, linked both ways to the one it refines (the original). It never reopens the original, and the original keeps its `✓` while the follow-up is worked on.

**Filing one.** `F` on a sent intent (in the tree, on a card, or in the detail pane) opens the add conversation already linked to it. The agent gets the original's title, description and criteria as context. The follow-up is filed in the original's group, as refining, and you send it as usual. `F` on a follow-up links the new one to the first original, the intent that describes the feature, so every refinement lands in one place. On an intent that has not been sent, `F` is refused: edit that one instead.

**On the original.**
- A **Follow-ups** section in the detail pane, after Missing items / Bugs, lists each one: `✓` complete, `→` sent, `○` not yet. Enter goes to it.
- The card reads `✓ complete · 1 follow-up` while any is open, and `✓ complete` again once all are complete.
- A follow-up never takes the original's `✓` away. A missing item does, since it is a gap in the original's own criteria.
- Pull requests stay with their intent. One linked to or found for a follow-up is left out of the original's, so it never sets the original back to "PR open" or starts the original's reconcile or check.

**The fold.** Once a follow-up reads `✓ complete`, it is folded into the original: a reconcile of the original that changes its acceptance criteria to describe the app as it now behaves.
- It waits until the original is finished: its pull-request searches have answered, no pull request is open, its own reconcile is applied and its own check has run.
- One fold runs at a time per original, and each follow-up folds once.
- The agent gets the original, the follow-up's description and criteria, and its merged pull requests. It changes only what the follow-up changed and copies every other criterion word for word. New and changed criteria start unticked.
- As with any reconcile, the original is marked unread (`*`), changed criteria show as new until read (`Y`), and a fold that fails stays in the status line until `r` retries it. The original's own merged set is untouched.

**After the fold.** With a capture guide in the repo, a completion check runs on just the criteria the fold added or changed: at once, or as soon as no other check or capture of the original is running (they are kept on the original until checked). When they pass, the original reads `✓ complete` again. A criterion that fails files nothing: the follow-up's own check already proved the app, so it reads `can't check: the fold may describe this wrongly: …` and stays unticked for you to fix or tick. Without a guide, the changed criteria wait for your ticks.

**`u`** after a fold puts back the version before it and declines that follow-up: it never folds in again on its own. To make the change after all, edit the original's criteria by hand.

## List view

`v` flips the browser between the group tree and one full-screen list of every intent, across all groups (collapsed or not). The tool always opens on that list (the card view), at its top card; the selection carries across both ways, and the sort order and filter are remembered.

```
Intents · 7 · sorted by Updated
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

Each intent is a card that opens with a horizontal rule: the title in bold, with its path in the tree under it; then the timestamp, the intent's status, the agent's status, how long ago it was sent and the implementation progress; then, once the agent is working in a worktree, that worktree's path (relative to the repo) and its workbrokr slot, read from the nearest `.slot` file above the agent's working directory; then, after a blank line, up to two wrapped lines of the description.

Before sending, the status reads `raw`, `?N` (N questions unanswered) or `ready`; once sent it reads `→ sent`. The same marks run through the tree: `○` raw, `◐` refining, `●` ready, `→` sent, `✓` only when every acceptance criterion is ticked (complete), and `*` before the title when a reconcile updated the intent and you have not read it yet. Once sent, a bar with one block per stage (sent, specced, planned, building, PR open, merged, verified) shows how far the feature has got, from evidence that already exists. A filled block means that stage is done; the verified block fills by its share of ticked criteria. Done cells are solid (`█`), cells in review dark shade (`▓`), cells still to do light shade (`░`); the kinds are carried by the glyphs because Herdr draws block characters in the default colour. Blocks touch with no gap cells: a done block ends in a slightly narrower cell (`▉`) whose thin sliver marks where it meets the next, and elsewhere the change of shade divides blocks (two to-do blocks side by side read as one light run). The label names the stage you are in. On a narrow card the label is cut with `…`; when there is no room for it the bar shows alone, and when there is no room for the bar the label shows alone, so the bar is never cut.

A `- plan:` line with no path (no `/` and no `.md`), such as `- plan: Slice 2: data`, names a slice the spec plans for later. Each one adds an empty one-cell block to the building stage and counts in the share like an average planned slice, the label says how many are left (`building 14/15 · 4 to plan`), and while any are left the intent stays at building even when the planned slices' pull requests are open or merged. Replace the line with the plan's path once it is written. The building stage holds one block per linked plan, sized from the plan itself: S (up to 3 tasks or 8 files named), M (up to 6 or 20), L (up to 10 or 40), XL beyond, taking the larger of the two; S is one cell wide, M two, L three, XL four, so the bar's length shows how big the effort is. A plan naming no tasks and no files gets no block. Each plan block fills from its own ledger while one exists, is full once a pull request mentioning the plan's file name is merged, and shows `▓` while that pull request is open. For example, two L plans merged and an M plan in review:

```
▉▉▉██▉██▉▓▓▉░░░   PRs 3/4 merged
```

The stages and their evidence:

| Stage | Label | Evidence |
|---|---|---|
| Sent | `speccing` or `charting · N decisions` | `state: sent` |
| Specced | `planning` | a `spec:` or `issue:` line under Artifacts |
| Planned / Building | `building k/N`, or `building` | `plan:` lines; `### Task` headings counted against each plan's superpowers ledger (`.superpowers/sdd/*/progress.md`), in the main checkout or any worktree, summed over plans. The plan executor deletes its ledger when a plan finishes, so without a ledger the label shows no count rather than a guess |
| PR open | `PR #n open`, `PRs m/t merged` | `pr:` lines, plus pull requests whose title or description mentions the intent's spec or plan file names (`gh pr list --search`, on entry, on `r` and about every ten minutes); state from `gh` |
| Merged | `verifying j/M` | every remaining PR merged (closed ones are ignored) |
| Complete | `✓ complete` | every acceptance criterion ticked, by you |

Ticked criteria fill only their own share of the bar; they never stand in for building or merging. The agent field shows the live Herdr state of the agent the intent was sent to: `working`, `idle`, `done`, `! needs you` when it is waiting on you, `closed` when its pane is gone, `…` until Herdr has answered, and `—` before sending.

Keys in the list: `j`/`k` move, `PgUp`/`PgDn` move a screen of cards (less one, so the last card you saw stays in view), `Enter` opens the intent in the tree (`v` from the tree opens the list on the tree's intent, the first card under a selected group, or the first card), `a` adds an intent by conversation (the agent proposes its group) and selects its new card, `v` back to the tree, `S` cycles the sort order (Updated, Progress with the least advanced first, Longest running with running agents sent longest ago first, Captured with the newest first, Created Asc with the oldest first, Title), `f` cycles the filter (All, In progress for everything not yet complete, Completed for sent intents with every acceptance criterion ticked; the header then reads `showing Completed (1 of 7)`), `s` send (same gate), `g` focus the sent agent, `F` file a follow-up of a sent intent, `D` delete an unsent intent, `r` rescan, `q` quit. A click selects a card and a click on the selected card opens it in the tree; the wheel moves the selection. Agent states refresh every few seconds while the list shows; plans and PR states on start, on `r` and about once a minute in either view. `herdr`, `gh` and git run on background threads, and any of them failing only blanks its column. `gh` is found through `GH_BIN_PATH`, else on `PATH`.

## Files

```
~/.helmsmith/intents/spaces/
  2026-10-24-pending-label.md            ← a quick capture, at the top until you move it (M)
  2026-10-24-pending-label.assets/screen.png
  2026-10-24-pending-label.chat.md
  2026-10-24-pending-label.assets/evidence/before.mp4   ← a bug's capture, once its issue is filed
  .polish/<name>/evidence/                ← captures until then
  skoolscout/applications/…
```

There is no default group: an intent added by conversation is filed where the agent and you settle it (see Adding an intent), and a non-interactive capture (`herdr-intents add "title"`) lands at the top of the workspace's group (the top of the tree outside Herdr).

Front matter: `title, kind, state (raw|refining|ready|sent), created, updated, repo, tags, polished_at, sent_to, conversations`, plus `id`, `version` (with `version_of`), `areas` and, for an intent added by conversation, `polish_dir` (the folder under `.polish/` its agent conversation runs in). `id` is 4 lowercase letters or digits, unique under the root: given when the intent is created, or before the first check or recording of an older one (a copied file's twin gets its own). `version` counts changes to the intent's text from 1: the description, criteria, stories, decisions and the rest, but not the Artifacts section, ticked checkboxes, trailing spaces or front matter, since those don't change what a video proves. `version_of` is the fingerprint it was counted at, so an edit made in your editor counts the next time the tool writes the file or a check or recording starts. Hand edits are read leniently: unknown fields are preserved, a missing title falls back to the file name, dates may be `YYYY-MM-DD`, tags may be a scalar, states are case-insensitive, CRLF is fine. Every field change (state, kind, sent) reloads the file first, so two toolbars never clobber each other's text; a save or an apply over a file that changed since you opened it asks before overwriting.

`send` needs `repo:` set to the main checkout; captures record it automatically (Herdr's `workspace_cwd` inside the toolbar, else the current directory). Without it the tool refuses to send rather than guess.

## Limits (v1)

- Text only inside the toolbar: attachments are listed and `o` opens them externally.
- The editor is deliberately small; `Ctrl-e` hands the file to `$EDITOR` for anything heavy.
- Codex and Copilot event shapes follow their documentation and are unverified on the author's machine.
