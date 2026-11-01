# A space is named after its meta repo

Status: draft for the owner's review. Decisions marked **(owner)** are the owner's. **(owner, Qn)** marks a recommendation from the 2026-10-31 design session that the owner accepted ("yes to the rest"). **(my call)** marks a choice that is mine and open to correction.

This is the first of four pieces of space setup **(owner accepted the split)**:
1. **This piece:** the space's name comes from its meta repo, and skoolscout moves to its new name.
2. The Spaces view: New space, a setup and repair checklist, and teammates joining.
3. "The runbook owns the app" (`2026-10-31-runbook-owns-the-app-design.md`).
4. An agent drafts the runbook, and secrets.

## Why

The owner wants a space's name to be its meta repo's name: "space name = meta-repo makes it more intuitive". Then there is no name to type, and nothing to keep in sync.

Today a space is named by `space = "…"` in the meta repo's `.helmsmith/intents/config.toml`, and falls back to the repo's folder name. Skoolscout uses the override: the repo is `skoolscout-com`, but the space is `skoolscout` (skoolscout-com #6012).

The folder name is not reliable on its own either. A teammate who runs `git clone … skoolscout` would get a different space. The name has to be the same on every clone, so it comes from the git remote **(owner, Q28)**.

The owner also wants existing intents moved to the new name automatically: "yes, move it automatically" **(owner)**.

## Goals

- **One name per project, the same on every clone,** taken from the meta repo with nothing to set.
- **Skoolscout keeps working through the rename.** Its intents, its pointer, its Herdr workspace, and the agents already working on sent intents all keep working.
- **No intent is lost or shown twice,** before, during or after the move.

## Non-goals

- **The Spaces view, setup and repair:** piece 2. That includes listing and tidying stray top-level folders **(owner, Q35)**.
- **Refusing two projects that share a repo name** (`org-a/web` and `org-b/web`): piece 2 (Q14). Today's folder-name rule mixes them the same way, so this piece makes nothing worse.
- **Sharing intents with teammates:** a later project **(owner, Q29)**.
- **Rewriting area references outside the moved space,** for example in stray groups **(owner, Q31, Q35)**.

## Anti-goals

- **Touching text that only looks like a space reference.** That includes `repo:` lines (`…/Workspaces/skoolscout/skoolscout-com`), GitHub slugs (`skoolscout/jefelabs-com#1856`), chat transcripts (`*.chat.md`) and reconcile backups.
- **Moving a space while a check, capture or recording runs in it (owner, Q33).**
- **A group that shows twice** because the tree follows the link the move leaves behind **(owner, Q32)**.
- **Partly moved state that stays broken.** A move cut off partway must finish on a later start, not leave a space half-renamed.

## What it does

### The name

- **The space's name is the repo name in the meta repo's `origin` URL (owner, Q28).** That is the last path segment with `.git` removed. `git@github.com:skoolscout/skoolscout-com.git`, `https://github.com/skoolscout/skoolscout-com` and `ssh://git@github.com/skoolscout/skoolscout-com.git` all give `skoolscout-com`.
  - **The case is kept as the URL writes it (my call).** Every space lookup already ignores case, so this changes nothing for matching, and the folder keeps the case people see on GitHub.
  - It is read from the **main checkout** of the repo. A worktree has the same `origin` anyway.
- **Without an `origin`, the name is the meta repo's folder name (owner, Q28).** That is also the fallback when the remote's name isn't usable as one folder name under `scope::group_name`, for example a repo called `.github` **(my call)**. If neither works, the start-up notice says why, as today.
- **Both marks name a space (my call).** A repo with only `.helmsmith/space.toml` now names its space too, the same way as one with `.helmsmith/intents/`. Today only the intents folder names a space, which the 2026-10-31 session flagged as inconsistent. In practice, `intents_space` and the open-space hook now accept any repo for which `is_meta` is true.
- **`config.toml` is no longer read (owner, Q7).** A `space = "…"` line is ignored. The folder `.helmsmith/intents/` is still a mark, and the file can stay as a place for comments. A `config.toml` that doesn't parse no longer matters, because nothing reads it **(my call)**.
  - The one-time notice that the line is ignored is part of the move's notice (below). Anyone with a stale line and no pointer has never used that space, so there's nothing to tell them **(my call)**.

### Until the move is done

**A meta repo's scope is the key of the pointer that already points at it (my call, from the builder check).** If no pointer points at it, the scope is the name worked out above. Before the move, skoolscout-com's scope therefore stays `skoolscout`. This matters because the move can wait for a run, and checks start on their own after a merge. Without this rule, the gap would:
- show `skoolscout-com` empty;
- record a second pointer (`remember_space_meta`);
- let a capture create a real `<root>/skoolscout-com/`, which would then block the move at every start.

With it, the space shows its intents and captures land with them until the move runs. After the move, the pointer's key is the new name, so the same rule gives the new name. Pointer paths are compared after `canonicalize`.

**An Intents already running on the old name follows the link (my call, from the builder check).** When `Group::scoped` finds no group for its scope, but `<root>/<scope>` is a link to a group in the same root, it shows that group. An Intents started before the move therefore keeps showing the intents rather than an empty group.

### The Herdr workspace

- **open-space renames the workspace to the new name.** Herdr's automatic name is the folder name, so for most clones the rename is to the same name. The rename still happens because it pins the name, so the workspace keeps it after a `cd` elsewhere, as #43 does today **(my call)**. A clone in a folder with a different name (`skoolscout/`) is renamed to `skoolscout-com`.
- **One workspace per space counts old names too (owner, Q34).** A workspace already named `skoolscout` (pinned by #43/#44) counts as skoolscout-com's workspace, so the hook doesn't open a second one.
- **A workspace named with an old name is renamed to the new one (my call, from the builder check).** The "someone named it" guard treats a name listed in `old_names` as one the hook chose. So a restored `skoolscout` workspace on the meta repo becomes `skoolscout-com`, and there is one name per project.

### The move

The move runs when Intents starts (the TUI and `add`/popup, the modes that go through `with_config`). It runs after `storage::settle_root` and before the scope is resolved **(owner, Q33)**.

**What triggers it:** a pointer `[spaces.X]` in `spaces.toml` whose `meta` folder exists, and whose name worked out as above (Y) differs from X, ignoring case **(my call)**. The trigger is the pointer, not `config.toml`, so the move still happens once the `space = "…"` line is gone. Skoolscout is the only such space today: X = `skoolscout`, Y = `skoolscout-com`.

**It moves nothing, says why, and tries again at the next start, when:**
- **A run is going in X (owner, Q33).** That is any intent under `<root>/X` whose evidence folder has a live capture (`capture::is_running`), or the space's run lock (`capture::space_lock_path(meta)`) held by a live run. Notice: `skoolscout will become skoolscout-com once its check or recording ends`. "Waits" means trying at the next start, never blocking start-up **(my call)**.
- **`<root>/X` and `<root>/Y` are both real folders.** Notice: `skoolscout and skoolscout-com both hold intents: move one by hand`, as `settle_root` does for `~/intents` **(my call)**. If `<root>/X` is missing or is already the link, a `<root>/Y` that exists means an earlier move got past step 1, and the move resumes **(my call, from the builder check)**.
- **`[spaces.Y]` already exists and points at a different meta repo:** a notice naming both paths **(my call)**. If it points at the **same** meta repo, which happens when an old build ran without the `space` line, the two entries merge. The `Y` entry's settings win, and settings only the `X` entry has are kept **(my call)**.

**What it does, in this order.** Each step can safely run again, and the pointer is changed last, so a move cut off partway finishes at the next start **(my call)**:
1. **Rename the folder** `<root>/X` to `<root>/Y` **(owner, Q31)**. This is skipped when `<root>/X` is missing (a pointer without intents yet) or is already the link from step 3.
2. **Rewrite area references, in `<root>/Y` only (owner, Q31).** In each `.md` file except `*.chat.md` and the reconcile backups:
   - each `areas:` list item in the front matter that is `X` or starts with `X/` becomes `Y…`;
   - each body line that starts `### X/` becomes `### Y/`.

   "Each `.md` file" means the files the tree loads as intents: dot folders and `<stem>.assets` folders are skipped, as `Store::scan` skips them **(my call, while building)**. Prefixes match ignoring case. Only those lines change. The file is edited as text, never re-serialised, so key order and formatting stay as they are **(my call)**.
   - **A rewritten file gets a new `updated` (my call).** An Intents already open in another workspace may still hold the file as it was before the move. If that Intents saves it, it gets the existing "file changed on disk" conflict instead of silently writing the old area names back.
3. **Leave a link** `<root>/X → Y`, relative so it survives the root being moved **(owner, Q32)**. Agents already sent hold absolute paths under `…/spaces/skoolscout/…`, and they keep writing to the right files.
4. **Rename the pointer key** `[spaces.X]` to `[spaces.Y]`, keeping `meta`, `storage` and `url`, and record `old_names = ["X"]` **(owner, Q31, Q34; the key's name is my call)**.

**Only one Intents moves (my call).** Each workspace's toolbar runs its own Intents, so several can start together. The move takes an exclusive lock file beside `spaces.toml` (create-new). One that can't take it skips the move without a notice and carries on with the pointers as it reads them.

**It says so once.** The start that does the move shows: `moved space skoolscout to skoolscout-com, its meta repo's name; skoolscout still finds it`. If the meta repo's `config.toml` still has a `space` line, it adds: `; space = "…" in <path> is no longer read: remove it` **(owner, Q7, Q33)**.

**Any space, not only skoolscout:** the same rule applies to any pointer whose key isn't its meta repo's name **(owner, Q31)**.

### The link never shows as a group (owner, Q32)

A top-level entry of the intents root that is a symlink to a folder directly inside the same root is skipped everywhere the root's groups are listed. That covers `Store::scan`, `Store::scope_group`, the whole-tree view, and the "names it" listing for blocked storage. A link that points anywhere else, such as a person's own link to a synced folder, still shows as today **(my call)**.

### Old names still work (owner, Q34)

A space name that comes from outside is mapped to the current name through `old_names`:
- **the scope:** from `INTENTS_SCOPE`, or from the Herdr workspace's name;
- **open-space's "one workspace per space" check;**
- **the first segment of a typed "move to group" destination;**
- **pointer lookups** (`meta_for`, `storage_for`), so `[spaces.skoolscout-com]` answers for `skoolscout`.

The scope is mapped before `Group::scoped`, so a workspace still named `skoolscout` shows skoolscout-com's real group, not the link.

### Left as they are (owner, Q35)

The empty `skoolz/` group and the stray group named after an intent title are left alone. The Spaces view in piece 2 lists them for tidying.

### skoolscout-com (owner, Q7)

A skoolscout-com PR, made the way that repo makes PRs, removes the `space = "skoolscout"` line. `config.toml` keeps a comment saying that the folder marks the meta repo and the name comes from `origin`.

## Constraints

- **Order: the skoolscout-com PR merges after the new Intents is built on your machine.** An old build reading a `config.toml` without the line names the space `skoolscout-com` and shows an empty group, while the intents still sit in `skoolscout/`. It would also record a second pointer. The merge rule above repairs that case, but it shouldn't be needed.
- **The hooks' manifest doesn't change,** so rebuilding the release binary is enough and no re-link is needed. A re-link is only needed if the manifest changes after all.
- **Rust, test-first, in `tools/intents`,** as #42 to #44 were built.

## Rollout

1. herdr-plugins PR. Build the release binary on the owner's machine. The next Intents start moves `skoolscout` and shows the notice once.
2. Check the result on the owner's data:
   - the folder is moved;
   - the two intents' areas and headings are rewritten, and nothing else changed (diff against a copy taken first);
   - the link is in place and doesn't show in the tree;
   - `spaces.toml` has `[spaces.skoolscout-com]` with `old_names`;
   - a workspace or `INTENTS_SCOPE` named `skoolscout` shows skoolscout-com.
3. skoolscout-com PR removing the `space` line.

## Testing

- **The name:** `origin` in ssh, https and `ssh://` form, with and without `.git`; case kept; no `origin`; a remote name that isn't a folder name; a meta repo with only `space.toml`; a `space = "…"` line ignored; a `config.toml` that doesn't parse ignored.
- **The move:**
  - the happy path;
  - a live capture in the space, and a held run lock (nothing moves, and the notice says why);
  - `<root>/Y` already exists;
  - `[spaces.Y]` with the same meta (the entries merge) and with a different meta (refused);
  - a move cut off after each of steps 1 to 3, then finished at the next start;
  - two starts at once (only one moves);
  - a pointer without a group folder;
  - the exact rewrite: `areas:` items and `### ` headings change, while `repo:` lines, GitHub slugs, other headings, `*.chat.md` and backups are byte-for-byte unchanged, and `updated` changes only in rewritten files.
- **The link:** skipped in the scan, the whole tree and `scope_group`; a link pointing outside the root still shows.
- **Old names:** the scope from `INTENTS_SCOPE` and from the Herdr label; open-space with an existing `skoolscout` workspace; a typed move destination; `meta_for`/`storage_for` by old name.
- **Existing tests to update:**
  - `tests/space.rs`: `the_intents_config_names_the_space`, `without_a_config_file_the_space_is_named_after_the_repo`, `a_broken_intents_config_says_where`;
  - `app.rs`: `an_intents_folder_marks_the_meta_repo_of_the_space_it_names`;
  - `open_space.rs`: the `meta_repo` fixture and the tests that assert `workspace rename w3 skoolscout`;
  - `scope.rs`: `a_workspace_in_a_meta_repo_sees_that_repos_space`.
- **Docs:** README §"Which space" and §"Naming the space in the meta repo" (the `shop` / `shop-meta` example), §"Opening a meta repo opens its space", and the module docs in `scope.rs`, `space.rs` and `open_space.rs`.

## Left to the builder

- Helper names, and where the old-name mapping hooks in, provided every entry point listed under "Old names still work" goes through it.
- The exact notice wording, keeping the meaning above.
- How the move's lock file is named and cleaned up.
- Whether area lookups (`root.join(area)` in the "affected by" index) should also resolve through the link. With the rewrite they shouldn't need to.
