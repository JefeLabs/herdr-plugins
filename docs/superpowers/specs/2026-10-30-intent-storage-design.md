# Where intents are stored

Status: draft for the owner's review. Decisions marked **(owner)** are the owner's; **(my call)** are mine and open to correction.

## Why

Intents live in `~/intents/<workspace>/`, a folder that belongs to no project and to no tool family. The owner wants storage to be a setting with three kinds, so a space can later keep its intents with the project or on a shared server **(owner)**:
- **profile:** the person's own machine;
- **meta:** the meta repo, shared through git;
- **remote:** a REST API backed by a database.

Only profile is built now. Meta and remote are placeholders **(owner: "leave place holder for remote and meta options")**.

## What it does

### Profile storage: `~/.helmsmith/intents/spaces/<space>/` (owner)

- **The default for every space.** `~/.helmsmith/intents/spaces/` becomes the tool's default root, replacing `~/intents` **(my call)**.
  - Each space is the top-level folder named like the Herdr workspace, exactly as each workspace's group is today. Scoping, groups, `INTENTS_SCOPE` and the tree work unchanged.
  - The agents' `.polish/` and `.trash/` stay at the root, as today: `~/.helmsmith/intents/spaces/.polish/` and `.../.trash/` **(my call)**.
- **The meta storage path stays consistent:** a later meta option would use `<meta repo>/.helmsmith/intents/` **(owner: "let keep intents consistent")**.
- **Overrides keep working.** `INTENTS_ROOT` and a `root =` other than the old default still win. A `root = "~/intents"` line counts as "the default", as it does today, so the shipped `intents.toml` and the owner's both follow the move without being edited **(my call)**.

### Moving what's there (my call)

- **When:** on the first start after the update, if `~/.helmsmith/intents/spaces/` does not exist and `~/intents/` does, the tool moves `~/intents/` there in one rename. The tool says so once.
  - The move covers everything: `skoolscout/`, `skoolz/`, the stray "Users can run…" folder, `.polish/` and `.trash/`.
  - The intents' `.assets/`, evidence and check records move with them, so nothing is lost.
- **A link at the old path (my call, from the builder check).** After the rename, `~/intents` becomes a link to the new folder. These all write to the old absolute paths and keep working through the link:
  - a running capture or check, whose order file holds the old paths;
  - another copy of the tool still open in another Herdr workspace;
  - a coding agent that was sent an intent before the move and appends `## Artifacts` lines to its old path.

  The link can be deleted once nothing uses the old path. If the link can't be made, the message says so.
- **When it doesn't move:**
  - If `~/.helmsmith/intents/spaces/` already exists, the tool moves nothing. A `~/intents` that is a link counts as already moved.
  - If both folders hold intents, it names both and uses the new one.
  - If the rename fails (another disk, a permission), the tool keeps using `~/intents` and says why.
- **Conversations started before the move (my call, from the builder check).** Claude remembers a conversation by the folder it ran in, and `.polish/` moved, so those polish and add conversations can't resume. A turn that fails to resume is started once more as a fresh turn. A fresh turn's prompt already carries the intent's text and the conversation so far. This also covers any other resume that fails.

### The setting: `storage` per space (owner: "config based")

In each person's `spaces.toml`, beside the meta pointer from part 1 **(my call)**:

```toml
[spaces.skoolscout]
meta = "~/Development/Workspaces/skoolscout/skoolscout-com"
storage = "profile"      # the default; "meta" and "remote" are placeholders
```

- **Values:**
  - `profile`, or no line at all, uses the profile folder;
  - `meta` and `remote` are accepted words. The space then shows no intents, and the tool shows "meta storage is not built yet" or "remote storage is not built yet" in place of the tree. It never falls back to profile, so nothing is written where the person didn't ask for it **(my call)**.
  - Any other word is an error that names the file.
- **The whole-tree view (my call, from the builder check).** Outside Herdr, or with `INTENTS_SCOPE=""`, a top-level group whose space is set to `meta` or `remote` is left out, and the message names it. Nothing is added to it, moved into it or checked automatically there.
- **Placeholder fields (my call):** `remote` may already carry `url = "…"`. It is read and kept, unused. No token field exists yet; a token will never go in a shared file.

## Goals

- Every intent lives under `~/.helmsmith/intents/spaces/<space>/` by default, with today's intents moved there without loss.
- A space's storage is one setting, ready for meta and remote later.

## Non-goals

- Building meta or remote storage.
- The REST API, its contract or its server.
- Moving videos or attachments anywhere else.

## Anti-goals

- Losing or duplicating an intent during the move.
- Writing intents somewhere other than the storage a space names.

## Constraints

- Tests use temporary homes (`HOME`, `INTENTS_ROOT` unset) and never touch the real `~/intents` or `~/.helmsmith`.
- No push, merge or release without the owner's word.

## Left to the builder

- The exact message wording.
- Whether the move runs in `Config::load` or at start-up beside the old `jefelabs.capabilities` migration.
