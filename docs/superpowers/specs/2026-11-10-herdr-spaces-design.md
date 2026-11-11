# herdr-spaces: define and manage spaces from the toolbar

Status: approved by the owner on 2026-11-10. **(owner)** marks the owner's decisions. **(owner, accepted)** marks a proposal of mine that the owner accepted in the 2026-11-04 to 2026-11-10 design session. **(my call)** marks a choice of mine that is open to correction.

This is piece 2 of the space setup listed in `2026-10-31-space-name-from-meta-repo-design.md` ("The Spaces view: New space, a setup and repair checklist, and teammates joining"), built as its own tool instead of a view inside Intents **(owner)**.

## Why

The owner had to edit a space's config by hand to add repos **(owner)**. A space today is three files that nothing edits for you:

- `<meta>/.helmsmith/space.toml`: the space's repos, `repos = [{ path = "../api", git = "git@…" }, …]`, with paths relative to the meta repo. It's shared through git.
- `<meta>/.helmsmith/run.md`: how to run the app. Its `db:` line is the agent runtime context the Database tool uses.
- `<intents config dir>/spaces.toml`: each person's pointer from a space's name to their clone of its meta repo (`[spaces.<name>] meta = "~/…"`). It's written automatically the first time Intents starts in a meta repo workspace. The config dir is `herdr plugin config-dir jefelabs.intents`, normally `~/.config/herdr/plugins/config/jefelabs.intents`.

The owner works on a Mac and on a Coder workspace (Linux, seen through Chrome). A space has to come up on both without hand-editing.

## What a space is

A space has four parts **(owner)**:

| Part | What it is | Where it's saved |
|---|---|---|
| **Meta repo** | The designated repo that holds the space's context (`.helmsmith/`) **(owner)**. The space is named after it, as today: the repo name in its `origin` URL. | Its folder: this machine (`spaces.toml`, `meta =`) |
| **Repos** | The space's list of repos **(owner)** | Shared: `space.toml`, `repos` |
| **Workspace path** | The folder the space's Herdr workspace opens in. New terminals in the space start there **(owner)**. Managed by the tool **(owner)**. | This machine: `spaces.toml`, new key `workspace =` **(my call)**, because the path differs between the Mac and Coder |
| **Database** | On or off. On when the space's app needs a database **(owner)**. | Shared: `space.toml`, new key `database = true/false` **(my call)** |

One more setting comes from the PR decision below: **Changes**, `push` or `pr`, saved as a new shared key `changes =` in `space.toml` **(my call)**.

`spaces.toml` also gets `git =`, the meta repo's GitHub address as it was when recorded. That's what the wrong-repo check compares against **(my call)**.

**The workspace path is only where things start (owner, accepted).** It isn't where the repo list comes from. On the owner's Mac, `~/Development/Workspaces/skoolscout` also holds jefelabs-com, tourneyseason-com and about 30 other folders. Two spaces may share one workspace path.

- **Default:** the meta repo's parent folder **(my call)**.
- **Used for:** where a new or cloned meta repo goes, where new or cloned repos go, and the folder `o` (open) gives a new Herdr workspace.

**The database switch doesn't touch `db:` (owner).** "The agent runtime context is needed. What we can maybe control in the tool is if database is enabled." The `db:` line in `run.md` stays as it is.

- **On:** the toolbar shows the Database tool. It uses `db:` to connect to the active agent's database, as today.
- **On, but `run.md` has no `db:`:** the Database tool opens LazySql's own connection list, and the Settings tab says `db:` is missing **(my call)**.
- **Off:** the Database tool is hidden on the bar in that space **(owner)**.
- **No `database` key** (a hand-written file): on when `run.md` has `db:`, off otherwise, so existing spaces behave as they do today **(my call)**.

## Goals

- Add and remove a space's repos without editing a file by hand.
- Create a space starting from its meta repo, either an existing repo or a new one **(owner)**: "when creating workspace, most important thing is select a new or existing repo to be the meta repo".
- Set the workspace path and database switch from the screen.
- Shared changes reach the other machine and teammates through git, with no manual commit **(owner)**: "commit and push it automatically".
- A teammate, or the owner on a second machine, can join a space from its GitHub repo and clone the missing repos in a few keys.

## Non-goals

- Editing `run.md`, including `db:` **(owner)**.
- Tidying stray intent groups, such as the empty `skoolz/` and the group named after an intent title. That stays with Intents **(owner)**.
- Merging PRs. The owner merges **(owner, accepted)**.
- Deleting folders or GitHub repos. Removing a repo only takes it off the list **(owner, accepted)**.
- Intents storage and secrets.
- Windows.

## Anti-goals

The owner said what would make them wince: **"pushing to the wrong repo or overwriting my changes" (owner)**. These are hard rules. Every behaviour below gives way to them.

### Never push to the wrong repo (my call, how)

- It pushes only the meta repo shown on screen, only to that repo's `origin`, and never with force.
- If the folder's `origin` doesn't match the `git =` recorded in `spaces.toml` (a re-clone, a fork, a different folder), it stops and asks before any push.
- The screen always shows the repo and branch a change will go to.
- **Two projects with the same repo name** (`org-a/web` and `org-b/web`) would both be called `web`. The second is refused with the reason (2026-10-31 Q14) **(owner)**.
- **A listed repo whose folder holds a different repo** (its `origin` isn't the `git` in `space.toml`) is marked "different repo" and never cloned over or written to **(owner, accepted)**.

### Never overwrite the owner's changes (my call, how)

- A commit contains only `.helmsmith/space.toml` (`git commit --only`). Other files the owner has staged stay staged and out of it.
- If `space.toml` has uncommitted edits the tool didn't make, the tool shows them and stops instead of writing.
- It reads the file just before each write, edits it in place and keeps comments, key order and layout. That means `toml_edit`, not a `toml` round trip. The same goes for `spaces.toml`: today's `remember_meta` rewrites it with `toml::to_string`, so moving that into the shared library (below) moves it to `toml_edit` too.
- Pulling only merges: no rebase, reset or stash. If a merge isn't clean, it's aborted (`git merge --abort`) and the tool stops with the reason.
- It never clones into a folder that already has files.
- It never uses `--no-verify`. If a repo's commit hook refuses a commit, the tool shows the hook's message and stops.

## How changes are saved

**Shared changes are committed and pushed automatically (owner).** Shared changes are a repo added or removed, the database switch and the Changes setting. Changes to this machine's settings go only to `spaces.toml`.

**When the meta repo is also an app repo, changes go by PR (owner).** skoolscout-com is both the meta repo and the app repo, and its main checkout is on `develop`, often with unfinished work.

- **Which spaces use a PR (owner, accepted):** the `changes` setting. A meta repo the tool created new defaults to `push`. An existing repo picked as the meta repo defaults to `pr`. The owner can switch it. A `space.toml` with no `changes` key reads as `pr`, the safer one **(my call)**.
- **Push mode:**
  - It commits on the meta repo checkout's current branch and pushes that branch to its upstream with an explicit refspec.
  - A detached HEAD or a branch with no upstream stops with the reason.
  - If the push is rejected because the other machine pushed first, it fetches, merges and pushes once more. A conflict stops it **(my call)**.
- **PR mode (owner, accepted):**
  - The owner's checkout isn't touched.
  - The tool makes the commit on its own branch, cut from the latest default branch on `origin` (for example `develop`), in a separate worktree it owns. Then it pushes that branch and opens the PR with `gh pr create`.
  - The PR title says what changed.
  - **A new branch for each PR (my call, from the builder check):** `herdr-spaces/<space>-<YYYYMMDD-HHMM>`. A fixed name would need a force push after a squash merge, which is forbidden. The tool never deletes branches; GitHub's "delete head branch" setting or the owner tidies them.
  - **One PR at a time, per person (owner, accepted; per person is my call):**
    - While the person's own PR is open, more changes add commits to it. "Own" means a PR whose head starts with `herdr-spaces/<space>-` and whose author is the `gh` login.
    - The owner's Mac and Coder share one login, so they add to the same PR. Each fetches the branch before adding a commit.
    - A teammate gets a PR of their own and never pushes onto the owner's.
  - **While a PR is open (my call, from the builder check):**
    - Both tabs show the live settings (`origin/<default>`) with the PR's changes on top, each marked `pending · PR #N` with its link.
    - `x` on a pending repo adds a commit to the PR that takes it back out.
    - `c` can clone a pending repo, because cloning only touches this machine.
    - The Settings tab shows a pending database or Changes value with the same mark.
  - **After a merge:** the pending marks go once the default branch has the change, and the next change opens a new PR. The other machine gets the change after the merge.
  - **After a close without merging (my call):**
    - A closed PR counts as a rejection, so its changes are dropped from the screen and never brought back in a later PR.
    - The tool says so once: "PR #N was closed, so its changes were dropped".
- **Pulling when the tool opens:** it runs `git fetch` on the meta repo.
  - **Push mode:** it fast-forwards the checkout when it can (`git merge --ff-only`). Otherwise it shows "behind origin" and leaves the checkout alone **(my call)**.
  - **PR mode:** it only fetches. It reads the shared settings from `origin/<default>:.helmsmith/space.toml` and never changes the checkout. This keeps it out of a human's `develop` checkout while a dev server runs from it **(my call)**.
  - Intents keeps reading `space.toml` from the checkout, so in PR mode a merged change reaches Intents after the owner pulls.
- **skoolscout-com's commit hook:** I read the hook during the builder check but didn't run it.
  - **What it refuses:** `workbrokr hook pre-commit` refuses only two things: a commit on `develop`, `stage` or `prod`, and a write under a commons another effort has locked.
  - **Why that matters here:** `.helmsmith/**` is in the `tooling` commons. A PR-mode commit on `herdr-spaces/<space>-…` should therefore go through, except while another effort holds the tooling lock (4-hour expiry). Push mode on `develop` would always be refused, which is one more reason an existing repo defaults to PR.
  - **When it refuses (my call):** the tool shows the hook's message and keeps the change as `not sent` in its own worktree. `Enter` tries again, and so does the next time the tool opens. It never uses `--no-verify`.
  - **Before building PR mode,** the builder runs the hook once on a scratch branch to confirm this.

## Making and joining spaces

### The meta repo (owner)

Creating a space starts with choosing its meta repo. There are three ways **(owner, accepted)**:

1. **A repo cloned on this machine.** Pick its folder. If `.helmsmith/space.toml` is missing, it's added with no repos and committed under the rules above.
2. **A repo on GitHub, cloned here for you.** Pick it, and the tool clones it into the workspace path. This is how a teammate, or the owner's second machine, joins a space **(owner)**. Today nothing is ever cloned: Intents prints a `git clone` command. Cloning is fine here because the person asks for it **(owner, accepted)**.
3. **A new repo,** created here and on GitHub **(owner, accepted).** This is the only way to get a new meta repo, because a local-only one would never reach the other machine. It would also take its folder's name and be renamed once published.
   - The tool creates the folder in the workspace path, runs `git init`, adds `.helmsmith/space.toml` and makes a first commit.
   - Then it creates the GitHub repo with `gh repo create <owner>/<name> --private --source … --remote origin --push`.
   - **Owner:** the `gh` login or one of its orgs.
   - **Visibility:** private by default.
   - **One confirm screen** lists everything that will be made. Nothing is created before it.
   - If `gh` isn't signed in, or the owner declines the GitHub step, it stops at the local repo and says how to publish later. The space name is then the folder name until it's published, and the existing move logic handles the rename (`old_names`) **(my call)**.

After that, the space's pointer (`meta`, `workspace`, `git`) is recorded in `spaces.toml`.

### The space's other repos (owner)

`a` (add repo) offers the same three ways **(owner)**: "allow me to add additional repos that exists or add new ones".

- **Cloned here:** pick the folder. Its `git` is read from `origin`.
- **On GitHub:** pick it, and it's cloned into the workspace path.
- **New:** the same steps as a new meta repo, with its own confirm screen. A new repo gets git, a README and a first commit, but no `.helmsmith/` **(owner, accepted)**.

Each repo is written to `space.toml` as `{ path = <relative to the meta repo>, git = <url> }`, the existing format, and saved under the rules above.

- **Removing a repo (`x`)** only takes it off the list **(owner, accepted)**.
- **Cloning missing repos (`c`):** a repo listed but missing on this machine is cloned to `meta.join(path)`. That's the same place Intents already looks (`space::missing`), even when it's outside the workspace path.
- **Clone URL form:** the URL is cloned as written in `space.toml`. New entries use `gh`'s `git_protocol` setting (ssh or https) **(my call)**.

### Opening a space (my call)

`o` opens the space:
- If a Herdr workspace with the space's name exists, the tool switches to it.
- Otherwise it runs `herdr workspace create --cwd <workspace path>`, renames the new workspace to the space's name, and loads the space's toolbar or Intents pane, reusing `open_space::load`.

This is needed because today's open-space hook only recognises a workspace whose folder is inside a meta repo's main checkout. A workspace path like `~/Development/Workspaces/skoolscout` isn't in any repo, and it can be shared by several spaces, so the hook can't work out the space from the folder. It's the workspace name that ties it to the space, and Intents already scopes by name.

Herdr's default `new_cwd = "follow"` then starts new panes in the workspace's folder. A pane split from another pane starts in that pane's folder instead, so the workspace path is a starting point, not a lock. The toolbar's Terminal tool keeps opening in the active agent's worktree.

## The screens (owner)

Layout C was picked from three options **(owner)**: one space at a time, with a Repos tab and a Settings tab. It opens on the space of the workspace you're in. Outside a space, it opens on the space picker. The Settings tab and the new-space steps were approved as shown **(owner)**. The mockups are in the session's browser companion. The copies below are the reference, and the data in them is an example.

**Repos tab**

```
skoolscout-com    Repos   Settings                Tab to switch · s spaces
──────────────────────────────────────────────────────────────────────────
   skoolscout-com   ✓   meta     ./skoolscout-com
   skoolscout-ai    ✓            ./skoolscout-ai
   skoolscout-rag   ✗            not cloned here · c to clone

 PR #6101 waiting: add skoolscout-rag
──────────────────────────────────────────────────────────────────────────
a add repo  x remove  c clone missing  Tab settings  s spaces  n new space
```

**Settings tab** (each row says where it's saved)

```
skoolscout-com   Repos   Settings                 Tab to switch · s spaces
──────────────────────────────────────────────────────────────────────────
 Meta repo   ~/Development/Workspaces/skoolscout/skoolscout-com   this machine
             github.com/skoolscout/skoolscout-com · develop
 Workspace   ~/Development/Workspaces/skoolscout                  this machine
             new terminals in this space start here
 Database    on    db: found in .helmsmith/run.md                 shared
 Changes     by PR into develop                                   shared
──────────────────────────────────────────────────────────────────────────
Shared settings change by a PR on skoolscout/skoolscout-com · PR #6101 waiting
Enter change  Tab repos  s spaces  n new space
```

**`n`: new space, four steps**

1. **Meta repo:** cloned on this machine / on GitHub, cloned here for you / new, created here and on GitHub.
2. **Pick it:** a folder, a GitHub repo, or for a new one: name, owner (←→ through the `gh` login and its orgs), private yes.
3. **Settings:**
   - Workspace path, defaulting as above. The repo goes in a folder there.
   - Database, off by default for a new repo. For an existing one it defaults to on when its `run.md` has `db:`.
   - Changes: push for a new repo, PR for an existing one.
4. **Confirm:** it lists every folder, GitHub repo, push and file it will create or change. `Enter` creates, `Esc` goes back. Nothing happens before this step.

After step 4, the new space opens on its Repos tab.

**Repair marks on the Repos tab (owner, accepted):**
- ✓ present.
- ✗ not cloned here (`c` clones).
- "different repo" (the folder's `origin` isn't the listed `git`; never touched).
- On the Settings tab: "meta repo's origin changed" (stops pushes) and "behind origin" (push mode couldn't fast-forward).

Keys follow the other tools: single letters, `Esc` cancels, `?` shows help. The rest of the look (colours, icons, spacing) follows herdr-intents.

## Where it lives and how it's built

- **A new program, `herdr-spaces` (owner).** It goes in `tools/spaces`, a fourth member of the Cargo workspace next to `toolbar`, `tools/intents` and `tools/doc`. It's a Rust terminal app with ratatui, like the others, and works standalone in any terminal. It ships in the same release with the same version (0.11.x line). It isn't a Herdr plugin: no hooks and no plugin actions **(my call)**.
- **A shared library for the space files (my call).**
  - The code that reads and writes `space.toml` and `spaces.toml` moves out of `tools/intents/src/space.rs` into a small library crate both tools use. That covers `Space`, `SpaceRepo`, `read_space`, `missing`, `is_meta`, `space_name`, `git_slug`/`repo_name`, and the pointer functions.
  - This keeps them agreeing on the format and on how the space is named.
  - `herdr-intents` keeps its behaviour, and its tests stay green.
  - `herdr-spaces` finds the pointer file in Intents' config dir (`INTENTS_CONFIG_DIR` overrides it, as today).
- **The toolbar:**
  - A new catalog entry `toolbar/setup/spaces.toml`: `id = "spaces"`, `name = "Spaces"`, `group = "space"`.
  - A line in `toolbar/setup/tools`, ticked by default, with Nerd Font and text icons.
  - A place in `toolbar/setup/order` after `intents`.
- **Hiding the Database tool when the switch is off:** the toolbar can't hide a tool today, so this needs a small toolbar feature **(my call, how)**.
  - `herdr-intents context` adds `"database": true/false` (worked out as above).
  - A tool entry can say it only shows when the database is on, for example `when = "database"`. A missing key means shown.
  - The `db.toml` catalog entry gets it.
- **An existing `tools.toml` is patched in place (owner).**
  - Both of the owner's machines already have one. The installer only writes the file when there's none, or on `herdr-toolbar setup` (which regenerates it).
  - So, as it did for the Capabilities → Intents rename, `install.sh` adds only two things to an existing file:
    - the Spaces entry, if no tool has `id = "spaces"`;
    - `when = "database"` on the `id = "db"` entry, if it doesn't have it.
  - It keeps a `.bak`, prints what it changed, and touches nothing else.
  - It runs once per change, so a second install changes nothing.
  - If the owner later removes the Spaces entry by hand, the next install must not add it back **(my call)**. How it remembers that is left to the builder, for example by recording that the entry was offered.
  - `scripts/test-install.sh` covers each case: a new file, an existing file without the entries, an existing file that already has them, and a hand-removed Spaces entry.
- **Release:**
  - The release archive and `bin_path = "target/release"` must include `herdr-spaces`, so the mise entry installs it.
  - The README gets a section for it.

## Left to the builder

- The library crate's name and module layout, and how `herdr-intents` re-exports from it.
- How a separate worktree for PR mode is made and cleaned up (`git worktree add` in a temp folder, or plumbing), and how the default branch is found.
- How the GitHub repo picker lists repos (`gh repo list` per owner, typing to filter).
- The toolbar field's exact name and how the bar refreshes when the switch changes.
- Icons.
- Exact copy for messages, keeping the rule that every refusal says why and what to do next.
